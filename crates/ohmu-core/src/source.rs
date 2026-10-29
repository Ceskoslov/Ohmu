use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Immutable source/artifact format, independent of the Wiki config version.
pub const SOURCE_FORMAT_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
/// Physical-page cap checked before extracting text from a PDF.
pub const MAX_PDF_PAGES: usize = 2_000;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid content-addressed identifier")]
pub struct InvalidDigest;

macro_rules! digest_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidDigest> {
                let value = value.into();
                let hex = value.strip_prefix($prefix).ok_or(InvalidDigest)?;
                if hex.len() != 64
                    || !hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(InvalidDigest);
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn hex(&self) -> &str {
                &self.0[$prefix.len()..]
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidDigest;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

digest_type!(ContentHash, "sha256:");
digest_type!(SourceId, "src-");

/// A section's half-open byte span in an immutable UTF-8 artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentNode {
    pub id: String,
    pub parent: Option<String>,
    pub title: String,
    pub start: usize,
    pub end: usize,
    pub page: Option<u32>,
}

/// Parser output; immutable once content-addressed. Nodes refer to `text` bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParsedDocument {
    pub schema_version: u32,
    pub parser: String,
    /// Binds normalized formats to their raw bytes. Legacy byte-preserving
    /// Markdown artifacts omit this field and retain their original hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<ContentHash>,
    pub title: String,
    pub text: String,
    pub nodes: Vec<DocumentNode>,
    pub diagnostics: Vec<String>,
}

/// Recoverable provenance sidecar; the derived SQLite catalog is not authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecord {
    pub schema_version: u32,
    pub source_id: SourceId,
    pub origin: String,
    pub revision: ContentHash,
    pub artifact: ContentHash,
    pub parser: String,
    pub byte_len: u64,
    pub imported_at_unix_ms: u128,
    /// Final HTTP(S) URL after validated redirects. Absent for local sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_url: Option<String>,
}

/// Evidence identity. Offsets refer to the immutable normalized artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLocator {
    pub source_id: SourceId,
    pub revision: ContentHash,
    pub artifact: ContentHash,
    pub start: usize,
    pub end: usize,
    pub page: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourcePassage {
    pub api_version: u32,
    pub locator: SourceLocator,
    pub content: String,
    pub next_offset: Option<usize>,
}
