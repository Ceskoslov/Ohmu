use crate::{DocumentNode, PageId, SourceRecord};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub page_id: PageId,
    pub revision: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub nodes: Vec<DocumentNode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageLink {
    pub from: PageId,
    pub reference: String,
    pub to: Option<PageId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageCatalog {
    pub api_version: u32,
    pub pages: Vec<PageInfo>,
    pub links: Vec<PageLink>,
    pub orphan_pages: Vec<PageId>,
    pub dangling_links: Vec<PageLink>,
    pub ambiguous_references: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageNeighbors {
    pub api_version: u32,
    pub page: PageInfo,
    pub incoming: Vec<PageId>,
    pub outgoing: Vec<PageLink>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PagePassage {
    pub api_version: u32,
    pub page_id: PageId,
    pub revision: String,
    pub start: usize,
    pub end: usize,
    pub content: String,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchCollection {
    #[default]
    Wiki,
    Sources,
    All,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub collection: SearchCollection,
    pub page_id: Option<PageId>,
    pub source: Option<SourceRecord>,
    pub revision: String,
    pub node_id: String,
    pub title: String,
    pub heading: String,
    pub start: usize,
    pub end: usize,
    pub page: Option<u32>,
    pub score: f32,
    pub excerpt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub api_version: u32,
    pub generation: String,
    pub hits: Vec<SearchHit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReindexReport {
    pub api_version: u32,
    pub generation: String,
    pub updated_entities: usize,
    pub removed_entities: usize,
    pub indexed_sections: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceNeighbors {
    pub api_version: u32,
    pub source: SourceRecord,
    pub node: DocumentNode,
    pub parent: Option<DocumentNode>,
    pub children: Vec<DocumentNode>,
}
