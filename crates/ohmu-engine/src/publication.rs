//! Durable staged publication, immutable page snapshots and explicit recovery.
use crate::{
    EngineError, Wiki, content_hash, open_child_directory,
    pages::{catalog, parse_page},
    read_file,
    sources::{ensure_directory, persist_immutable, source_error},
};
use ohmu_core::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Write,
    time::Duration,
};

const MAX_DRAFT: usize = 8 * 1024 * 1024;
const MAX_MANIFEST: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum RecoveryMode {
    Finish,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalChange {
    page_id: PageId,
    before: Option<ContentHash>,
    after: ContentHash,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    prepared: PreparedApply,
    state: PublicationState,
    before_snapshot: ContentHash,
    changes: Vec<JournalChange>,
    conflicts: BTreeMap<PageId, ContentHash>,
}

impl Wiki {
    pub fn stage_file(&self, path: impl AsRef<std::path::Path>) -> Result<Draft, EngineError> {
        let path = path.as_ref();
        let parent = crate::open_directory(
            path.parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| source_error("invalid proposal filename"))?;
        self.stage(serde_json::from_slice(&read_file(
            &parent, name, MAX_DRAFT,
        )?)?)
    }
    pub(crate) fn state_database(&self, create: bool) -> Result<Option<Connection>, EngineError> {
        if create {
            self.check_write_root()?;
        }
        let path = self.root.join(".ohmu/state.sqlite");
        if !create && !path.try_exists()? {
            return Ok(None);
        }
        open_child_directory(&self.directory, ".ohmu")?;
        let access = if create {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let database = Connection::open_with_flags(
            path,
            access | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        database.busy_timeout(Duration::from_secs(2))?;
        let version: u32 = database.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 1 || (!create && version != 1) {
            return Err(source_error(format!(
                "unsupported durable state version: {version}"
            )));
        }
        if create {
            database.pragma_update(None, "journal_mode", "WAL")?;
            database.pragma_update(None, "synchronous", "FULL")?;
            if version == 0 {
                database.execute_batch("BEGIN IMMEDIATE; CREATE TABLE drafts(id TEXT PRIMARY KEY,draft_json TEXT NOT NULL); CREATE TABLE operations(id TEXT PRIMARY KEY,draft_id TEXT NOT NULL,state TEXT NOT NULL,journal_json TEXT NOT NULL); CREATE INDEX operations_draft ON operations(draft_id); PRAGMA user_version=1; COMMIT;")?;
            }
            open_child_directory(&self.directory, ".ohmu")?.sync_all()?;
        }
        Ok(Some(database))
    }

    pub(crate) fn require_recovered(&self) -> Result<(), EngineError> {
        if let Some(database) = self.state_database(false)? {
            let pending:Option<String>=database.query_row("SELECT id FROM operations WHERE state NOT IN ('committed','rolled_back') LIMIT 1",[],|row|row.get(0)).optional()?;
            if let Some(id) = pending {
                return Err(EngineError::RecoveryRequired(id));
            }
        }
        Ok(())
    }

    pub fn verify_citation(&self, citation: &EvidenceCitation) -> Result<(), EngineError> {
        let locator = &citation.locator;
        let record = self.find_source(
            &locator.source_id,
            &locator.revision,
            Some(&locator.artifact),
        )?;
        let document = self.source_document(&record)?;
        if citation.quote.is_empty()
            || citation.quote.len() > 4096
            || locator.start > locator.end
            || document.text.get(locator.start..locator.end) != Some(citation.quote.as_str())
        {
            return Err(source_error(
                "citation quote does not match its immutable UTF-8 source span",
            ));
        }
        let page = document
            .nodes
            .iter()
            .find(|node| {
                node.page.is_some() && node.start <= locator.start && locator.end <= node.end
            })
            .and_then(|node| node.page);
        if page != locator.page {
            return Err(source_error("citation physical page or range mismatch"));
        }
        Ok(())
    }

    /// Stage full replacement bytes. Supplied citations are verified and rendered
    /// into a portable Evidence section before hashing the immutable draft.
    pub fn prepare_stage(&self, mut proposal: ChangeProposal) -> Result<Draft, EngineError> {
        let _guard = self.live_read_guard()?;
        self.require_recovered()?;
        if proposal.summary.trim().is_empty()
            || proposal.summary.len() > 4096
            || proposal.pages.is_empty()
            || proposal.pages.len() > 100
            || serde_json::to_vec(&proposal)?.len() > MAX_DRAFT
        {
            return Err(source_error("invalid proposal size, summary or page count"));
        }
        let mut ids = BTreeSet::new();
        let mut citation_count = 0;
        for change in &mut proposal.pages {
            if !ids.insert(change.page_id.clone()) {
                return Err(source_error("duplicate target page ID"));
            }
            citation_count += change.citations.len();
            if citation_count > 128 {
                return Err(source_error("a changeset supports at most 128 citations"));
            }
            if !change.citations.is_empty() {
                change.content.push_str("\n\n## Evidence\n");
            }
            for citation in &change.citations {
                self.verify_citation(citation)?;
                let loc = &citation.locator;
                change.content.push_str(&format!(
                    "\n- [{} @ {}]({})\n",
                    loc.source_id,
                    loc.revision,
                    loc.evidence_url()
                ));
                for line in citation.quote.lines() {
                    change.content.push_str(&format!("  > {line}\n"));
                }
            }
        }
        if serde_json::to_vec(&proposal)?.len() > MAX_DRAFT {
            return Err(source_error("rendered proposal exceeds 8 MiB"));
        }
        self.validate_proposal(&proposal)?;
        let id = hash(&serde_json::to_vec(&(1, &proposal))?)?;
        let draft = Draft {
            schema_version: 1,
            id,
            proposal,
        };
        Ok(draft)
    }

    pub fn stage(&self, proposal: ChangeProposal) -> Result<Draft, EngineError> {
        let draft = self.prepare_stage(proposal)?;
        self.stage_prepared(draft)
    }

    pub fn stage_prepared(&self, draft: Draft) -> Result<Draft, EngineError> {
        validate_draft_shape(&draft)?;
        let _lock = self.source_store()?;
        let database = self.state_database(true)?.expect("created state");
        let existing: bool = database.query_row(
            "SELECT EXISTS(SELECT 1 FROM drafts WHERE id=?1)",
            [draft.id.as_str()],
            |row| row.get(0),
        )?;
        if existing {
            return self.draft(&draft.id);
        }
        self.validate_proposal(&draft.proposal)?;
        database.execute(
            "INSERT INTO drafts(id,draft_json) VALUES(?1,?2)",
            params![draft.id.as_str(), serde_json::to_string(&draft)?],
        )?;
        self.draft(&draft.id)
    }

    pub fn draft(&self, id: &ContentHash) -> Result<Draft, EngineError> {
        let database = self
            .state_database(false)?
            .ok_or_else(|| source_error("draft not found"))?;
        let value: String = database.query_row(
            "SELECT draft_json FROM drafts WHERE id=?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        let draft: Draft = serde_json::from_str(&value)?;
        if draft.schema_version != 1
            || draft.id != *id
            || hash(&serde_json::to_vec(&(1, &draft.proposal))?)? != *id
        {
            return Err(source_error("draft format or content digest mismatch"));
        }
        validate_draft_shape(&draft)?;
        Ok(draft)
    }

    pub fn drafts(&self) -> Result<Vec<Draft>, EngineError> {
        let Some(database) = self.state_database(false)? else {
            return Ok(Vec::new());
        };
        let mut query = database.prepare("SELECT id FROM drafts ORDER BY id")?;
        let ids = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| self.draft(&ContentHash::new(id).map_err(source_error)?))
            .collect()
    }

    pub fn draft_diff(&self, id: &ContentHash) -> Result<String, EngineError> {
        let _guard = self.live_read_guard()?;
        self.require_recovered()?;
        let draft = self.draft(id)?;
        let pages = self.page_contents()?;
        let mut output = String::new();
        for change in draft.proposal.pages {
            let before = pages.get(&change.page_id).map(String::as_str).unwrap_or("");
            output.push_str(&format!(
                "--- a/pages/{}.md\n+++ b/pages/{}.md\n@@ -1,{} +1,{} @@\n",
                change.page_id,
                change.page_id,
                before.lines().count(),
                change.content.lines().count()
            ));
            for line in before.lines() {
                output.push_str(&format!("-{line}\n"));
            }
            for line in change.content.lines() {
                output.push_str(&format!("+{line}\n"));
            }
        }
        Ok(output)
    }

    fn validate_proposal(&self, proposal: &ChangeProposal) -> Result<(), EngineError> {
        let mut pages = self.page_contents()?;
        for change in &proposal.pages {
            if change.content.len() > MAX_TEXT_BYTES
                || pages
                    .get(&change.page_id)
                    .map(|text| hash(text.as_bytes()))
                    .transpose()?
                    != change.expected_revision
            {
                return Err(EngineError::Conflict);
            }
            for citation in &change.citations {
                self.verify_citation(citation)?;
            }
            pages.insert(change.page_id.clone(), change.content.clone());
        }
        let documents = pages
            .into_iter()
            .map(|(id, content)| parse_page(id, content))
            .collect::<Result<Vec<_>, _>>()?;
        let catalog = catalog(&documents);
        let targets: BTreeSet<_> = proposal
            .pages
            .iter()
            .map(|change| change.page_id.clone())
            .collect();
        if catalog
            .dangling_links
            .iter()
            .any(|link| targets.contains(&link.from))
        {
            return Err(source_error(
                "proposed page has a dangling or ambiguous Wiki link",
            ));
        }
        Ok(())
    }

    /// Pure preparation: freeze the draft, operation attempt and complete live
    /// page generation. It creates no files and grants no execution authority.
    pub fn prepare_apply(&self, id: &ContentHash) -> Result<PreparedApply, EngineError> {
        let _guard = self.live_read_guard()?;
        self.prepare_apply_unlocked(id)
    }

    fn prepare_apply_unlocked(&self, id: &ContentHash) -> Result<PreparedApply, EngineError> {
        self.require_recovered()?;
        let draft = self.draft(id)?;
        let database = self.state_database(false)?.expect("draft state exists");
        if let Some(value)=database.query_row("SELECT journal_json FROM operations WHERE draft_id=?1 AND state='committed' ORDER BY id LIMIT 1",[id.as_str()],|row|row.get::<_,String>(0)).optional()? {return Ok(decode_journal(&value)?.prepared);}
        self.validate_proposal(&draft.proposal)?;
        let count: i64 = database.query_row(
            "SELECT COUNT(*) FROM operations WHERE draft_id=?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        Ok(PreparedApply {
            schema_version: 1,
            draft_id: id.clone(),
            operation_id: hash(&serde_json::to_vec(&(id, count))?)?,
            base_generation: page_generation(&self.page_contents()?)?,
        })
    }

    pub fn apply(&self, id: &ContentHash) -> Result<OperationReport, EngineError> {
        let prepared = self.prepare_apply(id)?;
        self.apply_prepared(&prepared)
    }
    pub fn apply_prepared(&self, prepared: &PreparedApply) -> Result<OperationReport, EngineError> {
        self.apply_with_fault(prepared, None)
    }

    fn apply_with_fault(
        &self,
        prepared: &PreparedApply,
        fault: Option<usize>,
    ) -> Result<OperationReport, EngineError> {
        let _lock = self.source_store_unchecked()?;
        let database = self.state_database(true)?.expect("created state");
        if let Some(value) = database
            .query_row(
                "SELECT journal_json FROM operations WHERE id=?1",
                [prepared.operation_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            let operation = decode_journal(&value)?;
            if operation.prepared != *prepared {
                return Err(EngineError::Conflict);
            }
            self.validate_journal(&operation)?;
            if operation.state == PublicationState::Committed {
                return Ok(report(&operation, true));
            }
            return Err(EngineError::RecoveryRequired(
                prepared.operation_id.to_string(),
            ));
        }
        self.require_recovered()?;
        if self.prepare_apply_unlocked(&prepared.draft_id)? != *prepared {
            return Err(EngineError::Conflict);
        }
        let draft = self.draft(&prepared.draft_id)?;
        let before = self.snapshot_locked()?;
        if hash(&serde_json::to_vec(&before.pages)?)? != prepared.base_generation {
            return Err(EngineError::Conflict);
        }
        let objects = self.objects(true)?;
        let mut changes = Vec::new();
        for change in &draft.proposal.pages {
            let after = hash(change.content.as_bytes())?;
            persist_immutable(&objects, after.hex(), change.content.as_bytes())?;
            changes.push(JournalChange {
                page_id: change.page_id.clone(),
                before: change.expected_revision.clone(),
                after,
            });
        }
        let mut operation = Journal {
            schema_version: 1,
            prepared: prepared.clone(),
            state: PublicationState::Prepared,
            before_snapshot: before.id,
            changes,
            conflicts: BTreeMap::new(),
        };
        save_journal(&database, &operation)?;
        let result = (|| {
            if fault == Some(0) {
                return Err(source_error("injected interruption after prepare"));
            }
            operation.state = PublicationState::Applying;
            save_journal(&database, &operation)?;
            if fault == Some(1) {
                return Err(source_error("injected interruption after execution start"));
            }
            for (index, change) in operation.changes.iter().enumerate() {
                if self.current_hash(&change.page_id)? != change.before {
                    return Err(EngineError::Conflict);
                }
                let bytes = self.read_object(&change.after)?;
                replace_page(
                    &self.pages,
                    &change.page_id,
                    &bytes,
                    &operation.prepared.operation_id,
                )?;
                if fault == Some(index + 2) {
                    return Err(source_error("injected interruption after page replacement"));
                }
            }
            operation.state = PublicationState::Committed;
            save_journal(&database, &operation)?;
            if fault == Some(operation.changes.len() + 2) {
                return Err(source_error("injected lost response after commit"));
            }
            Ok(report(&operation, false))
        })();
        result.map_err(|_: EngineError| {
            EngineError::RecoveryRequired(prepared.operation_id.to_string())
        })
    }

    pub fn operations(&self) -> Result<Vec<OperationReport>, EngineError> {
        let Some(database) = self.state_database(false)? else {
            return Ok(Vec::new());
        };
        let mut query = database.prepare("SELECT journal_json FROM operations ORDER BY id")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(report(&decode_journal(&row?)?, true)))
            .collect()
    }

    pub fn operation(&self, id: &ContentHash) -> Result<serde_json::Value, EngineError> {
        let database = self
            .state_database(false)?
            .ok_or_else(|| source_error("operation not found"))?;
        let value: String = database.query_row(
            "SELECT journal_json FROM operations WHERE id=?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        let journal = decode_journal(&value)?;
        if journal.prepared.operation_id != *id {
            return Err(source_error("operation identity mismatch"));
        }
        self.validate_journal(&journal)?;
        Ok(serde_json::to_value(journal)?)
    }

    /// Reconcile recorded byte hashes before changing any target. Divergent
    /// editor bytes are retained in objects and block recovery without overwrite.
    pub fn recover(
        &self,
        id: &ContentHash,
        mode: RecoveryMode,
    ) -> Result<OperationReport, EngineError> {
        let _lock = self.source_store_unchecked()?;
        let database = self
            .state_database(true)?
            .ok_or_else(|| source_error("operation not found"))?;
        let value: String = database.query_row(
            "SELECT journal_json FROM operations WHERE id=?1",
            [id.as_str()],
            |row| row.get(0),
        )?;
        let mut operation = decode_journal(&value)?;
        if operation.prepared.operation_id != *id {
            return Err(source_error("operation identity mismatch"));
        }
        self.validate_journal(&operation)?;
        if operation.state == PublicationState::Committed
            || operation.state == PublicationState::RolledBack
        {
            return Ok(report(&operation, true));
        }
        let objects = self.objects(false)?;
        if matches!(mode, RecoveryMode::Finish) {
            let draft = self.draft(&operation.prepared.draft_id)?;
            for change in &draft.proposal.pages {
                for citation in &change.citations {
                    self.verify_citation(citation)?;
                }
            }
            let before = self.snapshot(&operation.before_snapshot)?;
            let live = self.page_contents()?;
            let targets: BTreeSet<_> = operation
                .changes
                .iter()
                .map(|change| change.page_id.clone())
                .collect();
            let expected: BTreeMap<_, _> = before
                .pages
                .into_iter()
                .filter(|(id, _)| !targets.contains(id))
                .collect();
            let actual: BTreeMap<_, _> = live
                .iter()
                .filter(|(id, _)| !targets.contains(*id))
                .map(|(id, text)| Ok((id.clone(), hash(text.as_bytes())?)))
                .collect::<Result<_, EngineError>>()?;
            if expected != actual {
                operation.state = PublicationState::Conflict;
                save_journal(&database, &operation)?;
                return Err(EngineError::RecoveryRequired(id.to_string()));
            }
        }
        // Validate every backup/new object before the first recovery write.
        for change in &operation.changes {
            self.read_object(&change.after)?;
            if let Some(before) = &change.before {
                self.read_object(before)?;
            }
            let actual = self.current_hash(&change.page_id)?;
            if actual != change.before && actual.as_ref() != Some(&change.after) {
                if let Some(actual) = actual {
                    let bytes = read_file(
                        &self.pages,
                        &format!("{}.md", change.page_id),
                        MAX_TEXT_BYTES,
                    )?;
                    persist_immutable(&objects, actual.hex(), &bytes)?;
                    operation.conflicts.insert(change.page_id.clone(), actual);
                }
                operation.state = PublicationState::Conflict;
                save_journal(&database, &operation)?;
                return Err(EngineError::RecoveryRequired(id.to_string()));
            }
        }
        for change in &operation.changes {
            let target = match mode {
                RecoveryMode::Finish => Some(&change.after),
                RecoveryMode::Rollback => change.before.as_ref(),
            };
            if self.current_hash(&change.page_id)?.as_ref() == target {
                continue;
            }
            // Check again just before replacement; uncooperative editor races
            // cannot be perfectly excluded by ordinary filesystem rename.
            let actual = self.current_hash(&change.page_id)?;
            if actual != change.before && actual.as_ref() != Some(&change.after) {
                return Err(EngineError::RecoveryRequired(id.to_string()));
            }
            if let Some(hash) = target {
                replace_page(&self.pages, &change.page_id, &self.read_object(hash)?, id)?;
            } else {
                unlinkat(
                    &self.pages,
                    format!("{}.md", change.page_id),
                    AtFlags::empty(),
                )
                .map_err(std::io::Error::from)?;
                self.pages.sync_all()?;
            }
        }
        operation.state = match mode {
            RecoveryMode::Finish => PublicationState::Committed,
            RecoveryMode::Rollback => PublicationState::RolledBack,
        };
        save_journal(&database, &operation)?;
        Ok(report(&operation, false))
    }

    fn current_hash(&self, id: &PageId) -> Result<Option<ContentHash>, EngineError> {
        match read_file(&self.pages, &format!("{id}.md"), MAX_TEXT_BYTES) {
            Ok(bytes) => Ok(Some(hash(&bytes)?)),
            Err(EngineError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn validate_journal(&self, operation: &Journal) -> Result<(), EngineError> {
        let draft = self.draft(&operation.prepared.draft_id)?;
        let before = self.snapshot(&operation.before_snapshot)?;
        if hash(&serde_json::to_vec(&before.pages)?)? != operation.prepared.base_generation {
            return Err(source_error("journal base snapshot mismatch"));
        }
        let expected = draft
            .proposal
            .pages
            .iter()
            .map(|change| {
                Ok(JournalChange {
                    page_id: change.page_id.clone(),
                    before: change.expected_revision.clone(),
                    after: hash(change.content.as_bytes())?,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        if expected != operation.changes {
            return Err(source_error(
                "journal differs from the approved immutable draft",
            ));
        }
        for change in &expected {
            if before.pages.get(&change.page_id) != change.before.as_ref() {
                return Err(source_error(
                    "journal before-image disagrees with its snapshot",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn objects(&self, create: bool) -> Result<File, EngineError> {
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        if create {
            ensure_directory(&internal, "objects")
        } else {
            open_child_directory(&internal, "objects")
        }
    }
    pub(crate) fn read_object(&self, id: &ContentHash) -> Result<Vec<u8>, EngineError> {
        let bytes = read_file(&self.objects(false)?, id.hex(), MAX_TEXT_BYTES)?;
        if hash(&bytes)? != *id {
            return Err(source_error("retained page object integrity mismatch"));
        }
        Ok(bytes)
    }

    pub fn freeze_snapshot(&self) -> Result<Snapshot, EngineError> {
        let _lock = self.source_store()?;
        self.snapshot_locked()
    }
    fn snapshot_locked(&self) -> Result<Snapshot, EngineError> {
        let pages = self.page_contents()?;
        let objects = self.objects(true)?;
        let mut versions = BTreeMap::new();
        for (id, content) in &pages {
            let digest = hash(content.as_bytes())?;
            persist_immutable(&objects, digest.hex(), content.as_bytes())?;
            versions.insert(id.clone(), digest);
        }
        let sources = self.source_records()?;
        if page_generation(&pages)? != page_generation(&self.page_contents()?)? {
            return Err(EngineError::Conflict);
        }
        let id = hash(&serde_json::to_vec(&(1, &versions, &sources))?)?;
        let snapshot = Snapshot {
            schema_version: 1,
            id,
            pages: versions,
            sources,
        };
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        let snapshots = ensure_directory(&internal, "snapshots")?;
        persist_immutable(
            &snapshots,
            &format!("{}.json", snapshot.id.hex()),
            &serde_json::to_vec(&snapshot)?,
        )?;
        internal.sync_all()?;
        Ok(snapshot)
    }
    pub fn snapshot(&self, id: &ContentHash) -> Result<Snapshot, EngineError> {
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        let directory = open_child_directory(&internal, "snapshots")?;
        let snapshot: Snapshot = serde_json::from_slice(&read_file(
            &directory,
            &format!("{}.json", id.hex()),
            MAX_MANIFEST,
        )?)?;
        if snapshot.schema_version != 1
            || snapshot.id != *id
            || hash(&serde_json::to_vec(&(
                1,
                &snapshot.pages,
                &snapshot.sources,
            ))?)?
                != *id
        {
            return Err(source_error("snapshot format or digest mismatch"));
        }
        Ok(snapshot)
    }
    pub fn read_snapshot_page(
        &self,
        snapshot: &ContentHash,
        page_id: &PageId,
        offset: usize,
        maximum: usize,
    ) -> Result<PagePassage, EngineError> {
        if maximum == 0 || maximum > MAX_PAGE_BYTES {
            return Err(source_error("snapshot passage limit must be 1..=262144"));
        }
        let manifest = self.snapshot(snapshot)?;
        let revision = manifest
            .pages
            .get(page_id)
            .ok_or_else(|| source_error("page absent from snapshot"))?;
        let content = String::from_utf8(self.read_object(revision)?)?;
        if !content.is_char_boundary(offset) {
            return Err(source_error("invalid snapshot UTF-8 offset"));
        }
        let mut end = offset.saturating_add(maximum).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && offset < content.len() {
            return Err(source_error("limit too small for a UTF-8 character"));
        }
        Ok(PagePassage {
            api_version: API_VERSION,
            page_id: page_id.clone(),
            revision: revision.to_string(),
            start: offset,
            end,
            content: content[offset..end].into(),
            next_offset: (end < content.len()).then_some(end),
        })
    }
}

fn hash(bytes: &[u8]) -> Result<ContentHash, EngineError> {
    ContentHash::new(content_hash(bytes)).map_err(source_error)
}
fn validate_draft_shape(draft: &Draft) -> Result<(), EngineError> {
    let proposal = &draft.proposal;
    if draft.schema_version != 1
        || draft.id != hash(&serde_json::to_vec(&(1, proposal))?)?
        || proposal.summary.trim().is_empty()
        || proposal.summary.len() > 4096
        || proposal.pages.is_empty()
        || proposal.pages.len() > 100
        || serde_json::to_vec(proposal)?.len() > MAX_DRAFT
    {
        return Err(source_error("invalid prepared draft shape or digest"));
    }
    let ids: BTreeSet<_> = proposal
        .pages
        .iter()
        .map(|change| &change.page_id)
        .collect();
    if ids.len() != proposal.pages.len()
        || proposal
            .pages
            .iter()
            .map(|change| change.citations.len())
            .sum::<usize>()
            > 128
    {
        return Err(source_error(
            "duplicate draft targets or excessive citations",
        ));
    }
    for change in &proposal.pages {
        for citation in &change.citations {
            if !change.content.contains(&citation.locator.evidence_url()) {
                return Err(source_error(
                    "prepared draft omits a portable evidence reference",
                ));
            }
        }
    }
    Ok(())
}
fn page_generation(pages: &BTreeMap<PageId, String>) -> Result<ContentHash, EngineError> {
    let versions = pages
        .iter()
        .map(|(id, text)| (id, content_hash(text.as_bytes())))
        .collect::<BTreeMap<_, _>>();
    hash(&serde_json::to_vec(&versions)?)
}
fn state_name(state: PublicationState) -> &'static str {
    match state {
        PublicationState::Prepared => "prepared",
        PublicationState::Applying => "applying",
        PublicationState::Committed => "committed",
        PublicationState::RolledBack => "rolled_back",
        PublicationState::Conflict => "conflict",
    }
}
fn decode_journal(value: &str) -> Result<Journal, EngineError> {
    let journal: Journal = serde_json::from_str(value)?;
    if journal.schema_version != 1 || journal.prepared.schema_version != 1 {
        return Err(source_error("unsupported publication journal version"));
    }
    Ok(journal)
}
fn save_journal(database: &Connection, journal: &Journal) -> Result<(), EngineError> {
    database.execute("INSERT INTO operations(id,draft_id,state,journal_json) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET state=excluded.state,journal_json=excluded.journal_json",params![journal.prepared.operation_id.as_str(),journal.prepared.draft_id.as_str(),state_name(journal.state),serde_json::to_string(journal)?])?;
    Ok(())
}
fn report(operation: &Journal, reused: bool) -> OperationReport {
    OperationReport {
        api_version: API_VERSION,
        operation_id: operation.prepared.operation_id.clone(),
        draft_id: operation.prepared.draft_id.clone(),
        state: operation.state,
        reused,
    }
}
fn replace_page(
    parent: &File,
    id: &PageId,
    bytes: &[u8],
    operation: &ContentHash,
) -> Result<(), EngineError> {
    let name = format!(".apply-{}-{id}.tmp", operation.hex());
    // A previous interrupted attempt may have left this operation's own staging file.
    match unlinkat(parent, name.as_str(), AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(std::io::Error::from(error).into()),
    }
    let mut file = File::from(
        openat(
            parent,
            name.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .map_err(std::io::Error::from)?,
    );
    file.write_all(bytes)?;
    file.sync_all()?;
    renameat(parent, name.as_str(), parent, format!("{id}.md")).map_err(std::io::Error::from)?;
    parent.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn proposal(wiki: &Wiki) -> ChangeProposal {
        ChangeProposal {
            summary: "Cross-linked update".into(),
            pages: vec![
                PageChange {
                    page_id: PageId::new("welcome").unwrap(),
                    expected_revision: Some(
                        ContentHash::new(
                            wiki.read_page(&PageId::new("welcome").unwrap())
                                .unwrap()
                                .revision,
                        )
                        .unwrap(),
                    ),
                    content: "# Updated welcome\n[[topic]]\n".into(),
                    citations: vec![],
                },
                PageChange {
                    page_id: PageId::new("topic").unwrap(),
                    expected_revision: None,
                    content: "# Topic\n[[welcome]]\n".into(),
                    citations: vec![],
                },
            ],
        }
    }

    #[test]
    fn live_readers_and_writers_exclude_each_other_without_creating_read_locks() {
        let temp = tempdir().unwrap();
        let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
        let reader = wiki.live_read_guard().unwrap();
        assert!(wiki.source_store_unchecked().is_err());
        assert!(!wiki.root().join(".ohmu/sources.lock").exists());
        drop(reader);
        let writer = wiki.source_store_unchecked().unwrap();
        assert!(wiki.read_page(&PageId::new("welcome").unwrap()).is_err());
        assert!(wiki.reconcile_pages().is_err());
        drop(writer);
        assert!(wiki.read_page(&PageId::new("welcome").unwrap()).is_ok());
    }

    #[test]
    fn staging_is_idempotent_and_apply_freezes_versions_and_portable_evidence() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("wiki");
        let wiki = Wiki::init(&root, "test").unwrap();
        let original = fs::read(root.join("pages/welcome.md")).unwrap();
        let input = proposal(&wiki);
        let draft = wiki.stage(input.clone()).unwrap();
        assert_eq!(wiki.stage(input).unwrap(), draft);
        assert_eq!(wiki.drafts().unwrap().len(), 1);
        assert_eq!(fs::read(root.join("pages/welcome.md")).unwrap(), original);
        assert!(wiki.draft_diff(&draft.id).unwrap().contains("+[[topic]]"));
        let prepared = wiki.prepare_apply(&draft.id).unwrap();
        fs::write(root.join("pages/welcome.md"), "# User edit\n").unwrap();
        assert!(matches!(
            wiki.apply_prepared(&prepared),
            Err(EngineError::Conflict)
        ));
        fs::write(root.join("pages/welcome.md"), original).unwrap();
        let result = wiki.apply_prepared(&prepared).unwrap();
        assert_eq!(result.state, PublicationState::Committed);
        assert!(wiki.apply_prepared(&prepared).unwrap().reused);
        assert!(
            wiki.read_page(&PageId::new("topic").unwrap())
                .unwrap()
                .content
                .contains("[[welcome]]")
        );
        let source = temp.path().join("source.md");
        fs::write(&source, "Verified evidence.\n").unwrap();
        let record = wiki.ingest_file(source).unwrap().record;
        let passage = wiki.read_source(&record, 0, 100).unwrap();
        let citation = EvidenceCitation {
            locator: passage.locator,
            quote: passage.content,
        };
        let input = ChangeProposal {
            summary: "Cited note".into(),
            pages: vec![PageChange {
                page_id: PageId::new("note").unwrap(),
                expected_revision: None,
                content: "# Note\nVerified evidence.\n".into(),
                citations: vec![citation],
            }],
        };
        let draft = wiki.stage(input).unwrap();
        assert!(draft.proposal.pages[0].content.contains("ohmu-source://"));
        wiki.apply(&draft.id).unwrap();
    }

    #[test]
    fn every_publication_phase_can_be_reopened_and_finished_without_replaying_committed_writes() {
        for fault in 0..=4 {
            let temp = tempdir().unwrap();
            let root = temp.path().join("wiki");
            let wiki = Wiki::init(&root, "test").unwrap();
            let draft = wiki.stage(proposal(&wiki)).unwrap();
            let prepared = wiki.prepare_apply(&draft.id).unwrap();
            assert!(matches!(
                wiki.apply_with_fault(&prepared, Some(fault)),
                Err(EngineError::RecoveryRequired(_))
            ));
            drop(wiki);
            let wiki = Wiki::open(&root).unwrap();
            if fault < 4 {
                assert!(matches!(
                    wiki.read_page(&PageId::new("welcome").unwrap()),
                    Err(EngineError::RecoveryRequired(_))
                ));
                assert!(wiki.reindex(false).is_err());
            }
            let result = wiki
                .recover(&prepared.operation_id, RecoveryMode::Finish)
                .unwrap();
            assert_eq!(result.state, PublicationState::Committed);
            assert_eq!(
                fs::read_to_string(root.join("pages/welcome.md")).unwrap(),
                draft.proposal.pages[0].content
            );
            assert_eq!(
                fs::read_to_string(root.join("pages/topic.md")).unwrap(),
                draft.proposal.pages[1].content
            );
            assert!(wiki.apply_prepared(&prepared).unwrap().reused);
        }
    }

    #[test]
    fn rollback_restores_before_images_and_a_new_explicit_attempt_has_a_new_identity() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("wiki");
        let wiki = Wiki::init(&root, "test").unwrap();
        let original = fs::read(root.join("pages/welcome.md")).unwrap();
        let draft = wiki.stage(proposal(&wiki)).unwrap();
        let prepared = wiki.prepare_apply(&draft.id).unwrap();
        wiki.apply_with_fault(&prepared, Some(3)).unwrap_err();
        let report = wiki
            .recover(&prepared.operation_id, RecoveryMode::Rollback)
            .unwrap();
        assert_eq!(report.state, PublicationState::RolledBack);
        assert_eq!(fs::read(root.join("pages/welcome.md")).unwrap(), original);
        assert!(!root.join("pages/topic.md").exists());
        let next = wiki.prepare_apply(&draft.id).unwrap();
        assert_ne!(next.operation_id, prepared.operation_id);
        wiki.apply_prepared(&next).unwrap();
    }

    #[test]
    fn divergent_editor_bytes_and_missing_backups_are_never_overwritten_during_recovery() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("wiki");
        let wiki = Wiki::init(&root, "test").unwrap();
        let original = fs::read(root.join("pages/welcome.md")).unwrap();
        let draft = wiki.stage(proposal(&wiki)).unwrap();
        let prepared = wiki.prepare_apply(&draft.id).unwrap();
        wiki.apply_with_fault(&prepared, Some(2)).unwrap_err();
        fs::write(root.join("pages/welcome.md"), "# Important editor change\n").unwrap();
        assert!(
            wiki.recover(&prepared.operation_id, RecoveryMode::Finish)
                .is_err()
        );
        assert!(
            wiki.recover(&prepared.operation_id, RecoveryMode::Rollback)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join("pages/welcome.md")).unwrap(),
            "# Important editor change\n"
        );
        let detail = wiki.operation(&prepared.operation_id).unwrap();
        assert!(detail["conflicts"]["welcome"].is_string());
        fs::write(root.join("pages/welcome.md"), &original).unwrap();
        let backup = hash(&original).unwrap();
        fs::remove_file(root.join(".ohmu/objects").join(backup.hex())).unwrap();
        assert!(
            wiki.recover(&prepared.operation_id, RecoveryMode::Rollback)
                .is_err()
        );
        assert_eq!(fs::read(root.join("pages/welcome.md")).unwrap(), original);
    }

    #[test]
    fn immutable_snapshots_survive_live_changes_and_detect_corrupt_objects() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("wiki");
        let wiki = Wiki::init(&root, "test").unwrap();
        let snapshot = wiki.freeze_snapshot().unwrap();
        assert_eq!(wiki.freeze_snapshot().unwrap(), snapshot);
        let id = PageId::new("welcome").unwrap();
        let old = wiki.read_snapshot_page(&snapshot.id, &id, 0, 4096).unwrap();
        fs::write(root.join("pages/welcome.md"), "# New live content\n").unwrap();
        assert_eq!(
            wiki.read_snapshot_page(&snapshot.id, &id, 0, 4096).unwrap(),
            old
        );
        use std::os::unix::fs::PermissionsExt;
        let object = root.join(".ohmu/objects").join(snapshot.pages[&id].hex());
        fs::set_permissions(&object, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(object, "corrupt").unwrap();
        assert!(wiki.read_snapshot_page(&snapshot.id, &id, 0, 4096).is_err());
    }

    #[test]
    fn invalid_citations_links_duplicate_targets_and_future_state_fail_closed() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("wiki");
        let wiki = Wiki::init(&root, "test").unwrap();
        let mut input = proposal(&wiki);
        input.pages[0].content = "# Bad\n[[missing]]".into();
        assert!(wiki.stage(input).is_err());
        let mut input = proposal(&wiki);
        input.pages.push(input.pages[0].clone());
        assert!(wiki.stage(input).is_err());
        let file = temp.path().join("source.md");
        fs::write(&file, "Actual evidence\n").unwrap();
        let record = wiki.ingest_file(file).unwrap().record;
        let passage = wiki.read_source(&record, 0, 100).unwrap();
        let mut input = proposal(&wiki);
        input.pages[0].citations.push(EvidenceCitation {
            locator: passage.locator,
            quote: "fabricated".into(),
        });
        assert!(wiki.stage(input).is_err());
        let database = Connection::open(root.join(".ohmu/state.sqlite")).unwrap();
        database.pragma_update(None, "user_version", 99).unwrap();
        drop(database);
        assert!(wiki.freeze_snapshot().is_err());
        assert!(wiki.read_page(&PageId::new("welcome").unwrap()).is_err());
    }
}
