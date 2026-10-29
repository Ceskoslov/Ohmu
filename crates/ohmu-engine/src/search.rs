//! Incremental lexical indexing. Every query checks the authoritative generation.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use jieba_rs::{Jieba, TokenizeMode};
use ohmu_core::*;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tantivy::{
    Index, IndexWriter, Term,
    collector::TopDocs,
    query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery},
    schema::*,
    tokenizer::{TextAnalyzer, Token, TokenStream, Tokenizer},
};

use crate::{
    EngineError, Wiki, content_hash, open_child_directory,
    pages::{PageDocument, catalog},
    sources::{ensure_directory, source_error},
};

const TOKENIZER: &str = "ohmu-jieba-search-lower/1;jieba-rs/0.11.0;tantivy/0.27.0";
static JIEBA: OnceLock<Arc<Jieba>> = OnceLock::new();

#[derive(Clone)]
struct ChineseTokenizer {
    jieba: Arc<Jieba>,
}
impl Default for ChineseTokenizer {
    fn default() -> Self {
        Self {
            jieba: JIEBA.get_or_init(|| Arc::new(Jieba::new())).clone(),
        }
    }
}
struct ChineseStream {
    tokens: std::vec::IntoIter<Token>,
    current: Token,
}
impl TokenStream for ChineseStream {
    fn advance(&mut self) -> bool {
        if let Some(token) = self.tokens.next() {
            self.current = token;
            true
        } else {
            false
        }
    }
    fn token(&self) -> &Token {
        &self.current
    }
    fn token_mut(&mut self) -> &mut Token {
        &mut self.current
    }
}
impl Tokenizer for ChineseTokenizer {
    type TokenStream<'a> = ChineseStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let mut tokens = self
            .jieba
            .tokenize(text, TokenizeMode::Search, false)
            .into_iter()
            .filter(|token| token.word.chars().any(char::is_alphanumeric))
            .map(|token| Token {
                text: token.word.to_lowercase(),
                offset_from: token.byte_start,
                offset_to: token.byte_end,
                position: token.start,
                position_length: 1,
            })
            .collect::<Vec<_>>();
        // Search-mode subwords can precede their encompassing token in Jieba's
        // output. Tantivy requires monotonically ordered token positions.
        tokens.sort_by_key(|token| (token.offset_from, token.offset_to));
        tokens.dedup_by(|a, b| {
            a.offset_from == b.offset_from && a.offset_to == b.offset_to && a.text == b.text
        });
        ChineseStream {
            tokens: tokens.into_iter(),
            current: Token::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexState {
    version: u32,
    tokenizer: String,
    opstamp: u64,
    generation: String,
    entities: BTreeMap<String, String>,
}

struct Fields {
    key: Field,
    collection: Field,
    payload: Field,
    title: Field,
    aliases: Field,
    heading: Field,
    body: Field,
}
fn schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let key = builder.add_text_field("key", STRING);
    let collection = builder.add_text_field("collection", STRING);
    let payload = builder.add_text_field("payload", STORED);
    let options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let title = builder.add_text_field("title", options.clone());
    let aliases = builder.add_text_field("aliases", options.clone());
    let heading = builder.add_text_field("heading", options.clone());
    let body = builder.add_text_field("body", options);
    (
        builder.build(),
        Fields {
            key,
            collection,
            payload,
            title,
            aliases,
            heading,
            body,
        },
    )
}
fn register(index: &Index) {
    index
        .tokenizers()
        .register(TOKENIZER, TextAnalyzer::from(ChineseTokenizer::default()));
}
fn error(error: impl std::fmt::Display) -> EngineError {
    EngineError::StaleIndex(error.to_string())
}
fn source_key(record: &SourceRecord) -> String {
    format!(
        "source:{}:{}:{}",
        record.source_id, record.revision, record.artifact
    )
}
fn entities(pages: &[PageDocument], records: &[SourceRecord]) -> BTreeMap<String, String> {
    pages
        .iter()
        .map(|page| {
            (
                format!("wiki:{}", page.info.page_id),
                page.info.revision.clone(),
            )
        })
        .chain(records.iter().map(|record| {
            (
                source_key(record),
                content_hash(
                    serde_json::to_string(record)
                        .expect("source record is serializable")
                        .as_bytes(),
                ),
            )
        }))
        .collect()
}
fn generation(entities: &BTreeMap<String, String>) -> Result<String, EngineError> {
    Ok(content_hash(&serde_json::to_vec(&(TOKENIZER, entities))?))
}

impl Wiki {
    /// Rebuild missing/corrupt derived storage, or incrementally update changed entities.
    /// `rebuild` retires the old derived Tantivy directory; authoritative data is untouched.
    pub fn reindex(&self, rebuild: bool) -> Result<ReindexReport, EngineError> {
        self.rebuild_source_catalog()?;
        let _lock = self.source_store()?;
        let pages = self.page_documents()?;
        let records = self.source_records()?;
        let current = entities(&pages, &records);
        let fingerprint = generation(&current)?;
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        let path = self.root.join(".ohmu/search-index");
        if rebuild && path.try_exists()? {
            open_child_directory(&internal, "search-index")?;
            let retired = self.root.join(format!(
                ".ohmu/search-index-retired-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(source_error)?
                    .as_nanos()
            ));
            std::fs::rename(&path, retired)?;
        }
        ensure_directory(&internal, "search-index")?;
        let mut database = self.catalog(true)?;
        let previous = if rebuild {
            None
        } else {
            load_state(&database).ok().flatten()
        };
        let (expected_schema, fields) = schema();
        let index = if path.join("meta.json").try_exists()? {
            Index::open_in_dir(&path).map_err(error)?
        } else {
            Index::create_in_dir(&path, expected_schema.clone()).map_err(error)?
        };
        if index.schema() != expected_schema {
            return Err(error("incompatible index schema; run reindex --rebuild"));
        }
        register(&index);
        let previous = previous.filter(|state| {
            state.version == 1
                && state.tokenizer == TOKENIZER
                && index
                    .load_metas()
                    .is_ok_and(|meta| meta.opstamp == state.opstamp)
        });
        let old = previous
            .as_ref()
            .map(|state| state.entities.clone())
            .unwrap_or_default();
        let mut writer: IndexWriter = index
            .writer_with_num_threads(1, 32_000_000)
            .map_err(error)?;
        if previous.is_none() {
            writer.delete_all_documents().map_err(error)?;
        }
        let removed: Vec<_> = old
            .keys()
            .filter(|key| !current.contains_key(*key))
            .collect();
        for key in &removed {
            writer.delete_term(Term::from_field_text(fields.key, key));
        }
        let mut changed = 0;
        for page in &pages {
            let key = format!("wiki:{}", page.info.page_id);
            if old.get(&key) == current.get(&key) {
                continue;
            }
            writer.delete_term(Term::from_field_text(fields.key, &key));
            for node in &page.info.nodes {
                if node.start == node.end {
                    continue;
                }
                let hit = SearchHit {
                    collection: SearchCollection::Wiki,
                    page_id: Some(page.info.page_id.clone()),
                    source: None,
                    revision: page.info.revision.clone(),
                    node_id: node.id.clone(),
                    title: page.info.title.clone(),
                    heading: node.title.clone(),
                    start: node.start,
                    end: node.end,
                    page: None,
                    score: 0.0,
                    excerpt: String::new(),
                };
                add(
                    &writer,
                    &fields,
                    &key,
                    &page.info.aliases,
                    &page.content[node.start..node.end],
                    hit,
                )?;
            }
            changed += 1;
        }
        for record in &records {
            let key = source_key(record);
            if old.get(&key) == current.get(&key) {
                continue;
            }
            let document = self.source_document(record)?;
            writer.delete_term(Term::from_field_text(fields.key, &key));
            for node in &document.nodes {
                if node.start == node.end {
                    continue;
                }
                let hit = SearchHit {
                    collection: SearchCollection::Sources,
                    page_id: None,
                    source: Some(record.clone()),
                    revision: record.revision.to_string(),
                    node_id: node.id.clone(),
                    title: document.title.clone(),
                    heading: node.title.clone(),
                    start: node.start,
                    end: node.end,
                    page: node.page,
                    score: 0.0,
                    excerpt: String::new(),
                };
                add(
                    &writer,
                    &fields,
                    &key,
                    &[],
                    &document.text[node.start..node.end],
                    hit,
                )?;
            }
            changed += 1;
        }
        // Ordinary editor races are detected before committing the index. Query
        // generation checks also catch edits occurring after this point.
        if fingerprint != generation(&entities(&self.page_documents()?, &self.source_records()?))? {
            writer.rollback().map_err(error)?;
            return Err(EngineError::Conflict);
        }
        let opstamp = writer.commit().map_err(error)?;
        writer.wait_merging_threads().map_err(error)?;
        let state = IndexState {
            version: 1,
            tokenizer: TOKENIZER.into(),
            opstamp,
            generation: fingerprint.clone(),
            entities: current,
        };
        let catalog = catalog(&pages);
        let transaction = database.transaction()?;
        transaction.execute("DELETE FROM page_links", [])?;
        transaction.execute("DELETE FROM page_catalog", [])?;
        for page in &catalog.pages {
            transaction.execute(
                "INSERT INTO page_catalog(page_id,metadata_json) VALUES(?1,?2)",
                params![page.page_id.as_str(), serde_json::to_string(page)?],
            )?;
        }
        for link in &catalog.links {
            transaction.execute(
                "INSERT INTO page_links(from_id,reference,to_id) VALUES(?1,?2,?3)",
                params![
                    link.from.as_str(),
                    link.reference,
                    link.to.as_ref().map(PageId::as_str)
                ],
            )?;
        }
        transaction.execute("INSERT INTO index_state(singleton,state_json) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET state_json=excluded.state_json",[serde_json::to_string(&state)?])?;
        transaction.commit()?;
        internal.sync_all()?;
        let count = index.reader().map_err(error)?.searcher().num_docs() as usize;
        Ok(ReindexReport {
            api_version: API_VERSION,
            generation: fingerprint,
            updated_entities: changed,
            removed_entities: removed.len(),
            indexed_sections: count,
        })
    }

    /// Literal lexical search; stale/missing generations fail even for an empty result.
    pub fn search(
        &self,
        query: &str,
        collection: SearchCollection,
        limit: usize,
    ) -> Result<SearchResult, EngineError> {
        if query.trim().is_empty() || query.len() > 4096 || !(1..=100).contains(&limit) {
            return Err(source_error(
                "query must be nonempty and <=4096 bytes; limit must be 1..=100",
            ));
        }
        let pages = self.page_documents()?;
        let records = self.source_records()?;
        let fingerprint = generation(&entities(&pages, &records))?;
        let database = self.catalog(false).map_err(error)?;
        let state = load_state(&database)?.ok_or_else(|| error("index manifest missing"))?;
        if state.version != 1 || state.tokenizer != TOKENIZER || state.generation != fingerprint {
            return Err(error("content or tokenizer generation changed"));
        }
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        open_child_directory(&internal, "search-index")?;
        let index = Index::open_in_dir(self.root.join(".ohmu/search-index")).map_err(error)?;
        let (expected_schema, fields) = schema();
        if index.schema() != expected_schema
            || index.load_metas().map_err(error)?.opstamp != state.opstamp
        {
            return Err(error("index commit and catalog manifest disagree"));
        }
        register(&index);
        let mut tokenizer = ChineseTokenizer::default();
        let mut stream = tokenizer.token_stream(query);
        let mut tokens = BTreeSet::new();
        while stream.advance() {
            tokens.insert(stream.token().text.clone());
        }
        if tokens.is_empty() || tokens.len() > 64 {
            return Err(source_error("query must produce 1..=64 lexical terms"));
        }
        let mut terms: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for token in tokens {
            for (field, boost) in [
                (fields.title, 4.0),
                (fields.aliases, 3.0),
                (fields.heading, 2.0),
                (fields.body, 1.0),
            ] {
                terms.push((
                    Occur::Should,
                    Box::new(BoostQuery::new(
                        Box::new(TermQuery::new(
                            Term::from_field_text(field, &token),
                            IndexRecordOption::WithFreqs,
                        )),
                        boost,
                    )),
                ));
            }
        }
        terms.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(TermQuery::new(
                    Term::from_field_text(fields.key, &format!("wiki:{}", query.trim())),
                    IndexRecordOption::Basic,
                )),
                8.0,
            )),
        ));
        let lexical: Box<dyn Query> = Box::new(BooleanQuery::new(terms));
        let query: Box<dyn Query> = match collection {
            SearchCollection::All => lexical,
            _ => Box::new(BooleanQuery::new(vec![
                (Occur::Must, lexical),
                (
                    Occur::Must,
                    Box::new(TermQuery::new(
                        Term::from_field_text(fields.collection, collection_name(collection)),
                        IndexRecordOption::Basic,
                    )),
                ),
            ])),
        };
        let reader = index.reader().map_err(error)?;
        let searcher = reader.searcher();
        let top = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(error)?;
        let mut hits = Vec::new();
        for (score, address) in top {
            let document: TantivyDocument = searcher.doc(address).map_err(error)?;
            let json = document
                .get_first(fields.payload)
                .and_then(|value| value.as_str())
                .ok_or_else(|| error("index payload absent"))?;
            let mut hit: SearchHit = serde_json::from_str(json)?;
            let (text, node, title) = if let Some(id) = &hit.page_id {
                let page = pages
                    .iter()
                    .find(|page| &page.info.page_id == id && page.info.revision == hit.revision)
                    .ok_or_else(|| error("page revision changed"))?;
                let node = page
                    .info
                    .nodes
                    .iter()
                    .find(|node| node.id == hit.node_id)
                    .cloned()
                    .ok_or_else(|| error("page node missing"))?;
                (page.content.clone(), node, page.info.title.clone())
            } else if let Some(source) = &hit.source {
                let document = self.source_document(source)?;
                let node = document
                    .nodes
                    .iter()
                    .find(|node| node.id == hit.node_id)
                    .cloned()
                    .ok_or_else(|| error("source node missing"))?;
                (document.text, node, document.title)
            } else {
                return Err(error("index hit has no identity"));
            };
            if node.start != hit.start || node.end != hit.end {
                return Err(error("cached section boundaries disagree with content"));
            }
            let mut end = (node.start + 768).min(node.end);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            hit.title = title;
            hit.heading = node.title;
            hit.page = node.page;
            hit.excerpt = text[node.start..end].into();
            hit.score = score;
            hits.push(hit);
        }
        if fingerprint != generation(&entities(&self.page_documents()?, &self.source_records()?))? {
            return Err(EngineError::Conflict);
        }
        Ok(SearchResult {
            api_version: API_VERSION,
            generation: fingerprint,
            hits,
        })
    }
}

fn collection_name(collection: SearchCollection) -> &'static str {
    match collection {
        SearchCollection::Wiki => "wiki",
        SearchCollection::Sources => "sources",
        SearchCollection::All => "all",
    }
}
fn load_state(database: &rusqlite::Connection) -> Result<Option<IndexState>, EngineError> {
    let value: Option<String> = database
        .query_row(
            "SELECT state_json FROM index_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(error)?;
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|e| error(format!("invalid index manifest: {e}")))
        })
        .transpose()
}
fn add(
    writer: &IndexWriter,
    fields: &Fields,
    key: &str,
    aliases: &[String],
    text: &str,
    hit: SearchHit,
) -> Result<(), EngineError> {
    let mut document = TantivyDocument::default();
    document.add_text(fields.key, key);
    document.add_text(fields.collection, collection_name(hit.collection));
    document.add_text(fields.payload, serde_json::to_string(&hit)?);
    document.add_text(fields.title, &hit.title);
    document.add_text(fields.aliases, aliases.join(" "));
    document.add_text(fields.heading, &hit.heading);
    document.add_text(fields.body, text);
    writer.add_document(document).map_err(error)?;
    Ok(())
}
