//! Immutable local document sources with provenance sidecars and a rebuildable catalog.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ohmu_core::{
    API_VERSION, ContentHash, MAX_PAGE_BYTES, MAX_SOURCE_BYTES, MAX_TEXT_BYTES, ParsedDocument,
    SOURCE_FORMAT_VERSION, SourceId, SourceLocator, SourcePassage, SourceRecord,
};
use rusqlite::{Connection, OpenFlags, params};
use rustix::fs::{
    AtFlags, Dir, FlockOperation, Mode, OFlags, flock, linkat, mkdirat, openat, unlinkat,
};
use serde::{Deserialize, Serialize};

use crate::{EngineError, Wiki, content_hash, open_child_directory, open_directory, read_file};

use crate::parsers::{MAX_NODES, ParserKind};
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
// Text plus JSON escaping and a bounded number of structural nodes.
const MAX_ARTIFACT_BYTES: usize = 8 * MAX_TEXT_BYTES;
const CATALOG_VERSION: u32 = 1;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize)]
pub struct IngestReport {
    pub api_version: u32,
    pub record: SourceRecord,
    pub node_count: usize,
    /// Parser limitations, empty-page notices and normalization information.
    pub diagnostics: Vec<String>,
    /// True if this origin/revision/parser already had a committed manifest.
    pub reused: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ParseCache {
    schema_version: u32,
    revision: ContentHash,
    parser: String,
    artifact: ContentHash,
}

struct SourceStore {
    _lock: File,
    raw: File,
    blobs: File,
    artifacts: File,
    parses: File,
    records: File,
}

impl Wiki {
    /// Import one UTF-8 Markdown file, preserving bytes, headings and provenance.
    /// No Wiki page is created or changed. Existing source revisions are immutable.
    pub fn ingest_markdown(&self, path: impl AsRef<Path>) -> Result<IngestReport, EngineError> {
        let path = path.as_ref();
        if !path.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
        }) {
            return Err(source_error(
                "only .md and .markdown local sources are currently supported",
            ));
        }
        self.ingest_file(path)
    }

    /// Import local Markdown, UTF-8 HTML or a text PDF without publishing a Wiki page.
    /// File selection is validated by the parser, not trusted as media proof.
    pub fn ingest_file(&self, path: impl AsRef<Path>) -> Result<IngestReport, EngineError> {
        let path = path.as_ref();
        let kind = ParserKind::for_path(path)?;
        let parser = kind.identity();
        let parent = open_directory(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let filename = path
            .file_name()
            .ok_or_else(|| source_error("missing source filename"))?;
        let name = filename
            .to_str()
            .ok_or_else(|| source_error("source filenames must be UTF-8"))?;
        let bytes = read_file(&parent, name, MAX_SOURCE_BYTES)?;
        let canonical = path.canonicalize()?;
        let canonical = canonical
            .to_str()
            .ok_or_else(|| source_error("source paths must be UTF-8"))?;
        let origin = format!("file:{canonical}");
        let revision = digest(&bytes)?;
        let source_id = SourceId::new(format!("src-{}", digest(origin.as_bytes())?.hex()))
            .map_err(source_error)?;
        let store = self.source_store()?;
        let manifest_name = manifest_name(&source_id, &revision, parser)?;

        if let Some(existing) = optional_read(&store.records, &manifest_name, MAX_MANIFEST_BYTES)? {
            let record: SourceRecord = serde_json::from_slice(&existing)?;
            validate_record(&record)?;
            if record.origin != origin
                || record.source_id != source_id
                || record.revision != revision
                || record.parser != parser
            {
                return Err(source_error("source manifest identity mismatch"));
            }
            let document = self.load_document_from(&store, &record)?;
            let mut catalog = self.catalog(true)?;
            catalog_record(&mut catalog, &record)?;
            return Ok(IngestReport {
                api_version: API_VERSION,
                record,
                node_count: document.nodes.len(),
                diagnostics: document.diagnostics,
                reused: true,
            });
        }

        let cache_name = format!(
            "{}-{}.json",
            revision.hex(),
            digest(parser.as_bytes())?.hex()
        );
        let document = if let Some(cached) =
            optional_read(&store.parses, &cache_name, MAX_ARTIFACT_BYTES)?
        {
            let parsed: ParsedDocument = if kind == ParserKind::Markdown {
                // Preserve the already published Markdown cache format.
                serde_json::from_slice(&cached)?
            } else {
                let cache: ParseCache = serde_json::from_slice(&cached)?;
                if cache.schema_version != 1 || cache.revision != revision || cache.parser != parser
                {
                    return Err(source_error("parser cache identity mismatch"));
                }
                let artifact = read_file(
                    &store.artifacts,
                    &format!("{}.json", cache.artifact.hex()),
                    MAX_ARTIFACT_BYTES,
                )?;
                if digest(&artifact)? != cache.artifact {
                    return Err(source_error("cached artifact integrity mismatch"));
                }
                serde_json::from_slice(&artifact)?
            };
            validate_document(&parsed, parser)?;
            kind.validate_raw(&bytes, &parsed)?;
            parsed
        } else {
            kind.parse(&bytes)?
        };
        validate_document(&document, parser)?;
        let artifact_bytes = serde_json::to_vec(&document)?;
        if artifact_bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(EngineError::TooLarge {
                maximum: MAX_ARTIFACT_BYTES,
            });
        }
        let artifact = digest(&artifact_bytes)?;
        let record = SourceRecord {
            schema_version: SOURCE_FORMAT_VERSION,
            source_id,
            origin,
            revision,
            artifact,
            parser: parser.into(),
            byte_len: bytes.len() as u64,
            imported_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(source_error)?
                .as_millis(),
        };
        let record_bytes = serde_json::to_vec(&record)?;
        if record_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(EngineError::TooLarge {
                maximum: MAX_MANIFEST_BYTES,
            });
        }

        // Validate/open the catalog before committing authoritative source files.
        let mut catalog = self.catalog(true)?;
        persist_immutable(&store.blobs, record.revision.hex(), &bytes)?;
        persist_immutable(
            &store.artifacts,
            &format!("{}.json", record.artifact.hex()),
            &artifact_bytes,
        )?;
        let cache_bytes = if kind == ParserKind::Markdown {
            artifact_bytes.clone()
        } else {
            serde_json::to_vec(&ParseCache {
                schema_version: 1,
                revision: record.revision.clone(),
                parser: parser.into(),
                artifact: record.artifact.clone(),
            })?
        };
        persist_immutable(&store.parses, &cache_name, &cache_bytes)?;
        // The durable provenance sidecar is the source commit point. On catalog
        // failure, retry or rebuild recovers it without another parse/model call.
        persist_immutable(&store.records, &manifest_name, &record_bytes)?;
        store.raw.sync_all()?;
        catalog_record(&mut catalog, &record)?;
        Ok(IngestReport {
            api_version: API_VERSION,
            record,
            node_count: document.nodes.len(),
            diagnostics: document.diagnostics,
            reused: false,
        })
    }

    /// List cataloged revisions. A missing catalog is empty; rebuild restores it
    /// from immutable sidecars. This call does not initialize storage or mutate it.
    pub fn sources(&self) -> Result<Vec<SourceRecord>, EngineError> {
        let path = self.root.join(".ohmu/catalog.sqlite");
        if !path.try_exists()? {
            return Ok(Vec::new());
        }
        let catalog = self.catalog(false)?;
        let mut statement = catalog.prepare(
            "SELECT record_json FROM sources ORDER BY imported_at, source_id, revision, parser",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut records = Vec::new();
        for row in rows {
            let record: SourceRecord = serde_json::from_str(&row?)?;
            validate_record(&record)?;
            records.push(record);
        }
        Ok(records)
    }

    /// Rebuild only the source catalog. It never deletes drafts/recovery data.
    /// Every referenced raw/artifact object is validated before replacing rows.
    pub fn rebuild_source_catalog(&self) -> Result<usize, EngineError> {
        let store = self.source_store()?;
        let mut records = Vec::new();
        for entry in Dir::read_from(&store.records).map_err(std::io::Error::from)? {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = entry.file_name().to_str().map_err(source_error)?;
            if !name.ends_with(".json") {
                continue;
            }
            let bytes = read_file(&store.records, name, MAX_MANIFEST_BYTES)?;
            let record: SourceRecord = serde_json::from_slice(&bytes)?;
            validate_record(&record)?;
            if manifest_name(&record.source_id, &record.revision, &record.parser)? != name {
                return Err(source_error("manifest filename disagrees with identity"));
            }
            self.load_document_from(&store, &record)?;
            records.push(record);
        }
        let mut catalog = self.catalog(true)?;
        let transaction = catalog.transaction()?;
        transaction.execute("DELETE FROM sources", [])?;
        for record in &records {
            insert_record(&transaction, record)?;
        }
        transaction.commit()?;
        Ok(records.len())
    }

    /// Load an immutable parser artifact after checking its source and artifact hashes.
    pub fn source_document(&self, record: &SourceRecord) -> Result<ParsedDocument, EngineError> {
        validate_record(record)?;
        let raw = open_child_directory(&self.directory, "raw")?;
        let records = open_child_directory(&raw, "records")?;
        let committed: SourceRecord = serde_json::from_slice(&read_file(
            &records,
            &manifest_name(&record.source_id, &record.revision, &record.parser)?,
            MAX_MANIFEST_BYTES,
        )?)?;
        if committed != *record {
            return Err(source_error(
                "requested provenance does not match its committed manifest",
            ));
        }
        let blobs = open_child_directory(&raw, "blobs")?;
        let artifacts = open_child_directory(&raw, "artifacts")?;
        load_document(&blobs, &artifacts, record)
    }

    /// Read a bounded UTF-8 passage from a pinned source artifact. Locator end
    /// is clamped only to a valid character boundary, never split mid-character.
    pub fn read_source(
        &self,
        record: &SourceRecord,
        offset: usize,
        maximum: usize,
    ) -> Result<SourcePassage, EngineError> {
        if maximum == 0 || maximum > MAX_PAGE_BYTES {
            return Err(source_error("passage byte limit must be 1..=262144"));
        }
        let document = self.source_document(record)?;
        if !document.text.is_char_boundary(offset) {
            return Err(source_error(
                "offset must be a UTF-8 boundary within the artifact",
            ));
        }
        let page_node = document
            .nodes
            .iter()
            .find(|node| node.page.is_some() && node.start <= offset && offset < node.end);
        let mut end = offset.saturating_add(maximum).min(document.text.len());
        if let Some(node) = page_node {
            end = end.min(node.end);
        }
        while !document.text.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && offset < document.text.len() {
            return Err(source_error(
                "byte limit is too small for the next UTF-8 character",
            ));
        }
        Ok(SourcePassage {
            api_version: API_VERSION,
            locator: SourceLocator {
                source_id: record.source_id.clone(),
                revision: record.revision.clone(),
                artifact: record.artifact.clone(),
                start: offset,
                end,
                page: page_node.and_then(|node| node.page),
            },
            content: document.text[offset..end].to_owned(),
            next_offset: (end < document.text.len()).then_some(end),
        })
    }

    fn load_document_from(
        &self,
        store: &SourceStore,
        record: &SourceRecord,
    ) -> Result<ParsedDocument, EngineError> {
        load_document(&store.blobs, &store.artifacts, record)
    }

    fn source_store(&self) -> Result<SourceStore, EngineError> {
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        let lock = File::from(
            openat(
                &internal,
                "sources.lock",
                OFlags::CREATE
                    | OFlags::RDWR
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o600),
            )
            .map_err(std::io::Error::from)?,
        );
        if !lock.metadata()?.is_file() {
            return Err(EngineError::NotRegular("sources.lock".into()));
        }
        // Nonblocking acquisition reports a busy writer rather than hanging CLI/model calls.
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(std::io::Error::from)?;
        let raw = open_child_directory(&self.directory, "raw")?;
        let blobs = ensure_directory(&raw, "blobs")?;
        let artifacts = ensure_directory(&raw, "artifacts")?;
        let parses = ensure_directory(&raw, "parses")?;
        let records = ensure_directory(&raw, "records")?;
        raw.sync_all()?;
        internal.sync_all()?;
        Ok(SourceStore {
            _lock: lock,
            raw,
            blobs,
            artifacts,
            parses,
            records,
        })
    }

    fn catalog(&self, writable: bool) -> Result<Connection, EngineError> {
        // SQLite owns its WAL/SHM path handling. The internal directory is
        // trusted host storage, not a sandbox against concurrent directory swaps.
        open_child_directory(&self.directory, ".ohmu")?;
        open_child_directory(&open_directory(&self.root)?, ".ohmu")?;
        let access = if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let catalog = Connection::open_with_flags(
            self.root.join(".ohmu/catalog.sqlite"),
            access | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        catalog.busy_timeout(Duration::from_secs(2))?;
        catalog.pragma_update(None, "foreign_keys", true)?;
        let version: u32 = catalog.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > CATALOG_VERSION {
            return Err(source_error(format!(
                "unsupported catalog version: {version}"
            )));
        }
        if writable {
            catalog.pragma_update(None, "journal_mode", "WAL")?;
            catalog.pragma_update(None, "synchronous", "FULL")?;
            if version == 0 {
                catalog.execute_batch("BEGIN IMMEDIATE; CREATE TABLE sources (source_id TEXT NOT NULL, revision TEXT NOT NULL, parser TEXT NOT NULL, imported_at TEXT NOT NULL, record_json TEXT NOT NULL, PRIMARY KEY(source_id, revision, parser)); PRAGMA user_version=1; COMMIT;")?;
            }
        } else if version != CATALOG_VERSION {
            return Err(source_error("catalog is uninitialized; rebuild it first"));
        }
        Ok(catalog)
    }
}

fn digest(bytes: &[u8]) -> Result<ContentHash, EngineError> {
    ContentHash::new(content_hash(bytes)).map_err(source_error)
}

fn manifest_name(
    source: &SourceId,
    revision: &ContentHash,
    parser: &str,
) -> Result<String, EngineError> {
    Ok(format!(
        "{}-{}-{}.json",
        source.hex(),
        revision.hex(),
        digest(parser.as_bytes())?.hex()
    ))
}

fn validate_record(record: &SourceRecord) -> Result<(), EngineError> {
    if record.schema_version != SOURCE_FORMAT_VERSION
        || ParserKind::for_identity(&record.parser).is_none()
        || record.byte_len > MAX_SOURCE_BYTES as u64
    {
        return Err(source_error("unsupported or invalid source manifest"));
    }
    let expected = SourceId::new(format!("src-{}", digest(record.origin.as_bytes())?.hex()))
        .map_err(source_error)?;
    if record.source_id != expected {
        return Err(source_error("source ID does not match its origin"));
    }
    Ok(())
}

fn validate_document(document: &ParsedDocument, parser: &str) -> Result<(), EngineError> {
    if document.schema_version != SOURCE_FORMAT_VERSION
        || document.parser != parser
        || document.text.trim().is_empty()
        || document.text.len() > MAX_TEXT_BYTES
        || document.nodes.is_empty()
        || document.nodes.len() > MAX_NODES + 1
    {
        return Err(source_error("invalid or unsupported parser artifact"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for node in &document.nodes {
        if node.start > node.end
            || !document.text.is_char_boundary(node.start)
            || !document.text.is_char_boundary(node.end)
            || node
                .parent
                .as_ref()
                .is_some_and(|parent| !seen.contains(parent))
            || !seen.insert(node.id.clone())
        {
            return Err(source_error("invalid document node span or hierarchy"));
        }
    }
    Ok(())
}

fn load_document(
    blobs: &File,
    artifacts: &File,
    record: &SourceRecord,
) -> Result<ParsedDocument, EngineError> {
    let raw = read_file(blobs, record.revision.hex(), MAX_SOURCE_BYTES)?;
    if digest(&raw)? != record.revision || raw.len() as u64 != record.byte_len {
        return Err(source_error("raw source integrity mismatch"));
    }
    let bytes = read_file(
        artifacts,
        &format!("{}.json", record.artifact.hex()),
        MAX_ARTIFACT_BYTES,
    )?;
    if digest(&bytes)? != record.artifact {
        return Err(source_error("parser artifact integrity mismatch"));
    }
    let document: ParsedDocument = serde_json::from_slice(&bytes)?;
    validate_document(&document, &record.parser)?;
    ParserKind::for_identity(&record.parser)
        .ok_or_else(|| source_error("unsupported source parser"))?
        .validate_raw(&raw, &document)?;
    Ok(document)
}

fn catalog_record(catalog: &mut Connection, record: &SourceRecord) -> Result<(), EngineError> {
    let transaction = catalog.transaction()?;
    insert_record(&transaction, record)?;
    transaction.commit()?;
    Ok(())
}

fn insert_record(catalog: &Connection, record: &SourceRecord) -> Result<(), EngineError> {
    catalog.execute("INSERT INTO sources(source_id, revision, parser, imported_at, record_json) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(source_id,revision,parser) DO UPDATE SET record_json=excluded.record_json, imported_at=excluded.imported_at",
        params![record.source_id.as_str(), record.revision.as_str(), record.parser, format!("{:039}", record.imported_at_unix_ms), serde_json::to_string(record)?])?;
    Ok(())
}

fn ensure_directory(parent: &File, name: &str) -> Result<File, EngineError> {
    match mkdirat(parent, name, Mode::from_raw_mode(0o755)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => open_child_directory(parent, name),
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}

fn optional_read(parent: &File, name: &str, limit: usize) -> Result<Option<Vec<u8>>, EngineError> {
    match read_file(parent, name, limit) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(EngineError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn persist_immutable(parent: &File, name: &str, bytes: &[u8]) -> Result<(), EngineError> {
    if let Some(existing) = optional_read(parent, name, bytes.len())? {
        if existing != bytes {
            return Err(source_error(
                "existing immutable object has different bytes",
            ));
        }
        return Ok(());
    }
    let (temporary, mut file) = loop {
        let temporary = format!(
            ".tmp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        match openat(
            parent,
            temporary.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o444),
        ) {
            Ok(fd) => break (temporary, File::from(fd)),
            Err(rustix::io::Errno::EXIST) => continue,
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
    };
    let result: Result<(), EngineError> = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        linkat(parent, temporary.as_str(), parent, name, AtFlags::empty())
            .map_err(std::io::Error::from)?;
        parent.sync_all()?;
        Ok(())
    })();
    // Crash-orphaned .tmp files are ignored by manifest enumeration.
    let cleanup =
        unlinkat(parent, temporary.as_str(), AtFlags::empty()).map_err(std::io::Error::from);
    result?;
    cleanup?;
    parent.sync_all()?;
    Ok(())
}

pub(crate) fn source_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::Source(error.to_string())
}
