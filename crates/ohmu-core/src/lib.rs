//! Domain types for Ohmu. No model provider, harness, filesystem or database dependency.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod source;
pub use source::*;

/// Current on-disk configuration format. Unknown versions are rejected.
pub const CONFIG_VERSION: u32 = 1;
/// Version of CLI JSON and tool result envelopes.
pub const API_VERSION: u32 = 1;
/// Whole-page read bound in the scaffold; pagination is planned for M1.
pub const MAX_PAGE_BYTES: usize = 256 * 1024;

/// Stable, flat page identity, independent of its human-readable title.
///
/// The v1 scaffold uses `pages/<id>.md`. Nested paths are deliberately not IDs.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PageId(String);

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error(
    "page ID must be 1..=96 lowercase ASCII letters, digits, '_' or '-', starting with a letter or digit"
)]
pub struct InvalidPageId;

impl PageId {
    /// Validate an ID without interpreting it as a path.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidPageId> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 96
            || !value.as_bytes()[0].is_ascii_alphanumeric()
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(InvalidPageId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<String> for PageId {
    type Error = InvalidPageId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PageId> for String {
    fn from(value: PageId) -> Self {
        value.0
    }
}

/// Immutable bytes returned by one successful page read, with a SHA-256 revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageSnapshot {
    pub api_version: u32,
    pub page_id: PageId,
    pub revision: String,
    pub content: String,
}

/// Local configuration. Model credentials do not belong in this file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WikiConfig {
    pub schema_version: u32,
    pub name: String,
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_VERSION,
            name: "Ohmu Wiki".into(),
        }
    }
}
