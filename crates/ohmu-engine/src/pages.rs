use std::collections::{BTreeMap, BTreeSet};

use ohmu_core::{
    API_VERSION, MAX_PAGE_BYTES, MAX_TEXT_BYTES, PageCatalog, PageId, PageInfo, PageLink,
    PageNeighbors, PagePassage,
};
use pulldown_cmark::{Event, Options, Parser, Tag};
use rustix::fs::Dir;
use serde::Deserialize;

use crate::{
    EngineError, Wiki, content_hash, parsers::ParserKind, read_file, sources::source_error,
};

#[derive(Deserialize, Default)]
struct Frontmatter {
    id: Option<PageId>,
    title: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
}

pub(crate) struct PageDocument {
    pub info: PageInfo,
    pub content: String,
    pub references: Vec<String>,
}

impl Wiki {
    /// Parse authoritative page files and compute links without writing caches.
    pub fn reconcile_pages(&self) -> Result<PageCatalog, EngineError> {
        let documents = self.page_documents()?;
        Ok(catalog(&documents))
    }

    pub fn page_neighbors(&self, page_id: &PageId) -> Result<PageNeighbors, EngineError> {
        let catalog = self.reconcile_pages()?;
        let page = catalog
            .pages
            .into_iter()
            .find(|page| page.page_id == *page_id)
            .ok_or_else(|| source_error("page not found"))?;
        Ok(PageNeighbors {
            api_version: API_VERSION,
            page,
            incoming: catalog
                .links
                .iter()
                .filter(|link| link.to.as_ref() == Some(page_id))
                .map(|link| link.from.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            outgoing: catalog
                .links
                .into_iter()
                .filter(|link| link.from == *page_id)
                .collect(),
        })
    }

    /// A UTF-8-safe slice with a revision over the entire file (up to 16 MiB).
    pub fn read_page_passage(
        &self,
        page_id: &PageId,
        offset: usize,
        maximum: usize,
    ) -> Result<PagePassage, EngineError> {
        let _guard = self.live_read_guard()?;
        self.require_recovered()?;
        if maximum == 0 || maximum > MAX_PAGE_BYTES {
            return Err(source_error("page passage limit must be 1..=262144"));
        }
        let content = String::from_utf8(read_file(
            &self.pages,
            &format!("{page_id}.md"),
            MAX_TEXT_BYTES,
        )?)?;
        if !content.is_char_boundary(offset) {
            return Err(source_error(
                "page offset must be a UTF-8 boundary within the file",
            ));
        }
        let mut end = offset.saturating_add(maximum).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && offset < content.len() {
            return Err(source_error(
                "byte limit too small for the next UTF-8 character",
            ));
        }
        Ok(PagePassage {
            api_version: API_VERSION,
            page_id: page_id.clone(),
            revision: content_hash(content.as_bytes()),
            start: offset,
            end,
            content: content[offset..end].into(),
            next_offset: (end < content.len()).then_some(end),
        })
    }

    pub(crate) fn page_documents(&self) -> Result<Vec<PageDocument>, EngineError> {
        let _guard = self.live_read_guard()?;
        self.require_recovered()?;
        self.page_documents_unlocked()
    }

    pub(crate) fn page_documents_unlocked(&self) -> Result<Vec<PageDocument>, EngineError> {
        self.page_contents()?
            .into_iter()
            .map(|(id, content)| parse_page(id, content))
            .collect()
    }

    pub(crate) fn page_contents(&self) -> Result<BTreeMap<PageId, String>, EngineError> {
        let mut names = Vec::new();
        for entry in Dir::read_from(&self.pages).map_err(std::io::Error::from)? {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = entry.file_name().to_str().map_err(source_error)?;
            if name.ends_with(".md") {
                names.push(name.to_owned());
            }
        }
        names.sort();
        let mut documents = BTreeMap::new();
        let mut total = 0usize;
        for name in names {
            let id = PageId::new(name.trim_end_matches(".md")).map_err(source_error)?;
            let content = String::from_utf8(read_file(&self.pages, &name, MAX_TEXT_BYTES)?)?;
            total += content.len();
            if total > 512 * 1024 * 1024 {
                return Err(source_error(
                    "page reconciliation exceeds the 512 MiB in-memory text budget",
                ));
            }
            documents.insert(id, content);
        }
        Ok(documents)
    }
}

pub(crate) fn parse_page(page_id: PageId, content: String) -> Result<PageDocument, EngineError> {
    let (metadata, body_start) = if content.starts_with("---\n") || content.starts_with("---\r\n") {
        let first_end = content.find('\n').expect("prefix has newline") + 1;
        let mut boundary = None;
        let mut offset = first_end;
        for line in content[first_end..].split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == "---" {
                boundary = Some((offset, offset + line.len()));
                break;
            }
            offset += line.len();
            if offset > 16 * 1024 {
                break;
            }
        }
        let (end, body_start) =
            boundary.ok_or_else(|| source_error("frontmatter must close within 16 KiB"))?;
        let mut budget = serde_saphyr::Budget::default();
        budget.max_aliases = 0;
        budget.max_anchors = 0;
        budget.max_events = 4096;
        budget.flow_nesting_limit = 16;
        let mut options = serde_saphyr::Options::default();
        options.budget = Some(budget);
        let metadata: Frontmatter =
            serde_saphyr::from_str_with_options(&content[first_end..end], options).map_err(
                |error| source_error(format!("invalid frontmatter in {page_id}: {error}")),
            )?;

        (metadata, body_start)
    } else {
        (Frontmatter::default(), 0)
    };
    if metadata.id.as_ref().is_some_and(|id| *id != page_id) {
        return Err(source_error(format!(
            "frontmatter ID disagrees with filename: {page_id}"
        )));
    }
    if metadata.aliases.len() > 128
        || metadata
            .aliases
            .iter()
            .any(|alias| alias.trim().is_empty() || alias.len() > 256)
    {
        return Err(source_error(
            "page aliases must be nonempty, at most 256 bytes, with at most 128 entries",
        ));
    }
    let body = &content[body_start..];
    // Empty bodies are rejected rather than silently indexing frontmatter alone.
    let parsed = ParserKind::Markdown.parse(body.as_bytes())?;
    let title = metadata.title.unwrap_or(parsed.title);
    if title.trim().is_empty() || title.len() > 4096 {
        return Err(source_error("invalid page title"));
    }
    let mut nodes = parsed.nodes;
    for node in &mut nodes {
        node.start += body_start;
        node.end += body_start;
    }
    let mut references = BTreeSet::new();
    for event in Parser::new_ext(body, Options::ENABLE_WIKILINKS | Options::ENABLE_TABLES) {
        if let Event::Start(Tag::Link { dest_url, .. }) = event {
            let target = dest_url.as_ref();
            if target.contains("://")
                || target.starts_with('#')
                || target.starts_with("mailto:")
                || target.starts_with("data:")
            {
                continue;
            }
            let target = target.split('#').next().unwrap_or("");
            let target = target.strip_prefix("./").unwrap_or(target);
            let target = percent_encoding::percent_decode_str(target)
                .decode_utf8()
                .map_err(source_error)?;
            let target = target.strip_suffix(".md").unwrap_or(&target).trim();
            if !target.is_empty() {
                references.insert(target.to_owned());
            }
        }
    }
    Ok(PageDocument {
        info: PageInfo {
            page_id,
            revision: content_hash(content.as_bytes()),
            title,
            aliases: metadata.aliases,
            nodes,
        },
        content,
        references: references.into_iter().collect(),
    })
}

pub(crate) fn catalog(documents: &[PageDocument]) -> PageCatalog {
    let mut aliases: BTreeMap<String, BTreeSet<PageId>> = BTreeMap::new();
    let ids: BTreeSet<_> = documents
        .iter()
        .map(|doc| doc.info.page_id.clone())
        .collect();
    for document in documents {
        for name in std::iter::once(&document.info.title).chain(document.info.aliases.iter()) {
            aliases
                .entry(name.to_lowercase())
                .or_default()
                .insert(document.info.page_id.clone());
        }
    }
    let mut links = Vec::new();
    let mut ambiguous = BTreeSet::new();
    for document in documents {
        for reference in &document.references {
            let exact = PageId::new(reference.clone())
                .ok()
                .filter(|id| ids.contains(id));
            let to = exact.or_else(|| {
                let candidates = aliases.get(&reference.to_lowercase())?;
                if candidates.len() == 1 {
                    candidates.first().cloned()
                } else {
                    ambiguous.insert(reference.clone());
                    None
                }
            });
            links.push(PageLink {
                from: document.info.page_id.clone(),
                reference: reference.clone(),
                to,
            });
        }
    }
    let incoming: BTreeSet<_> = links
        .iter()
        .filter(|link| link.to.as_ref() != Some(&link.from))
        .filter_map(|link| link.to.clone())
        .collect();
    PageCatalog {
        api_version: API_VERSION,
        pages: documents.iter().map(|doc| doc.info.clone()).collect(),
        orphan_pages: ids.difference(&incoming).cloned().collect(),
        dangling_links: links
            .iter()
            .filter(|link| link.to.is_none())
            .cloned()
            .collect(),
        ambiguous_references: ambiguous.into_iter().collect(),
        links,
    }
}
