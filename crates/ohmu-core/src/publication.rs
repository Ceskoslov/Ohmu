use crate::{ContentHash, PageId, SourceLocator, SourceRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceCitation {
    pub locator: SourceLocator,
    pub quote: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageChange {
    pub page_id: PageId,
    pub expected_revision: Option<ContentHash>,
    pub content: String,
    #[serde(default)]
    pub citations: Vec<EvidenceCitation>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeProposal {
    pub summary: String,
    pub pages: Vec<PageChange>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub schema_version: u32,
    pub id: ContentHash,
    pub proposal: ChangeProposal,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedApply {
    pub schema_version: u32,
    pub draft_id: ContentHash,
    pub operation_id: ContentHash,
    pub base_generation: ContentHash,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    Prepared,
    Applying,
    Committed,
    RolledBack,
    Conflict,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationReport {
    pub api_version: u32,
    pub operation_id: ContentHash,
    pub draft_id: ContentHash,
    pub state: PublicationState,
    pub reused: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: ContentHash,
    pub pages: BTreeMap<PageId, ContentHash>,
    pub sources: Vec<SourceRecord>,
}
