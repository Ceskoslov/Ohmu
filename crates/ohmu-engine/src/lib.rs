//! Bounded, descriptor-relative reads for a local Markdown wiki.
//!
//! Supports Unix (Linux/WSL and macOS), immutable Markdown/HTML/PDF source ingestion and
//! a rebuildable source catalog, page relations and Chinese/English lexical search.
//! Draft publication and model-powered synthesis are separate layers.

#[cfg(not(unix))]
compile_error!("Ohmu currently supports Unix only; use WSL on Windows.");

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use ohmu_core::{API_VERSION, CONFIG_VERSION, MAX_PAGE_BYTES, PageId, PageSnapshot, WikiConfig};
use rustix::fs::{Mode, OFlags, open, openat};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

mod sources;
pub use sources::IngestReport;
mod pages;
mod parsers;
mod publication;
mod search;
pub use search::FrozenWiki;
mod web;
pub use publication::RecoveryMode;
mod backup;
pub use backup::BackupManifest;

const CONFIG_FILE: &str = "ohmu.toml";
const MAX_CONFIG_BYTES: usize = 16 * 1024;
const WELCOME: &str = "---\nid: welcome\ntitle: Welcome to Ohmu\n---\n\n# Welcome to Ohmu\n\nThis Markdown file is the content source of truth.\n\nEdit it in your editor, then read it through the CLI or Nausicaa.\n\nImport sources, run reindex, and search with the CLI. Model-generated changes will be staged for review before publication.\n";

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid wiki configuration: {0}")]
    Config(String),
    #[error("unsupported configuration version: {0}")]
    UnsupportedVersion(u32),
    #[error("wiki name cannot be empty")]
    EmptyName,
    #[error("expected a regular file: {0}")]
    NotRegular(String),
    #[error("file exceeds the {maximum}-byte read limit")]
    TooLarge { maximum: usize },
    #[error("page content changed; prepare the action again")]
    Conflict,
    #[error("file changed while being read; retry the read")]
    UnstableRead,
    #[error("source/catalog error: {0}")]
    Source(String),
    #[error("catalog database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("source serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("index is stale or unavailable; run reindex: {0}")]
    StaleIndex(String),
    #[error("publication {0} requires explicit recovery before further live reads or writes")]
    RecoveryRequired(String),
    #[error("file is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// Open directory descriptors keep reads attached to the validated directories.
/// Symlinks are refused for the final root component, layout and content files.
#[derive(Debug)]
pub struct Wiki {
    root: PathBuf,
    directory: File,
    pages: File,
    config: WikiConfig,
}

pub(crate) struct LiveReadGuard {
    _root: File,
    _legacy: Option<File>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub api_version: u32,
    pub root: PathBuf,
    pub name: String,
    pub schema_version: u32,
    pub checks: Vec<String>,
    pub implemented: Vec<String>,
    pub planned: Vec<String>,
}

impl Wiki {
    /// Initialize a new directory. Existing paths are never overwritten.
    ///
    /// The parent must already exist. A failed initialization leaves its partial
    /// directory for inspection; it never recursively removes user content.
    pub fn init(root: impl AsRef<Path>, name: &str) -> Result<Self, EngineError> {
        if name.trim().is_empty() {
            return Err(EngineError::EmptyName);
        }
        let config = WikiConfig {
            name: name.to_owned(),
            ..WikiConfig::default()
        };
        let encoded = toml::to_string_pretty(&config)
            .map_err(|error| EngineError::Config(error.to_string()))?;
        if encoded.len() > MAX_CONFIG_BYTES {
            return Err(EngineError::TooLarge {
                maximum: MAX_CONFIG_BYTES,
            });
        }
        let root = root.as_ref();
        fs::create_dir(root)?;
        let directory = open_directory(root)?;
        for name in ["pages", "raw", ".ohmu"] {
            rustix::fs::mkdirat(&directory, name, Mode::from_raw_mode(0o755))
                .map_err(std::io::Error::from)?;
        }
        write_new(&directory, CONFIG_FILE, encoded.as_bytes())?;
        let pages = open_child_directory(&directory, "pages")?;
        write_new(&pages, "welcome.md", WELCOME.as_bytes())?;
        pages.sync_all()?;
        directory.sync_all()?;
        Self::open(root)
    }

    /// Open an initialized wiki without changing it or loading project rules.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, EngineError> {
        let directory = open_directory(root.as_ref())?;
        let config = load_config(&directory)?;
        let pages = open_child_directory(&directory, "pages")?;
        // Verify required layout components, refusing symlinks.
        open_child_directory(&directory, "raw")?;
        open_child_directory(&directory, ".ohmu")?;
        Ok(Self {
            root: root.as_ref().canonicalize()?,
            directory,
            pages,
            config,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config(&self) -> &WikiConfig {
        &self.config
    }

    /// Exact filesystem identity for write approvals, including non-UTF-8 paths.
    pub fn authority_scope(&self) -> Result<String, EngineError> {
        let root = self.directory.metadata()?;
        let pages = self.pages.metadata()?;
        Ok(format!(
            "{} [{}:{};pages={}:{}]",
            self.root.display(),
            root.dev(),
            root.ino(),
            pages.dev(),
            pages.ino()
        ))
    }

    pub(crate) fn check_write_root(&self) -> Result<(), EngineError> {
        let root = open_directory(&self.root)?;
        let current = root.metadata()?;
        let held = self.directory.metadata()?;
        let pages = open_child_directory(&root, "pages")?.metadata()?;
        let held_pages = self.pages.metadata()?;
        if (current.dev(), current.ino(), pages.dev(), pages.ino())
            != (held.dev(), held.ino(), held_pages.dev(), held_pages.ino())
        {
            return Err(EngineError::Conflict);
        }
        Ok(())
    }

    pub(crate) fn root_lock(&self, exclusive: bool) -> Result<File, EngineError> {
        let lock = open_child_directory(&self.directory, ".")?;
        rustix::fs::flock(
            &lock,
            if exclusive {
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            } else {
                rustix::fs::FlockOperation::NonBlockingLockShared
            },
        )
        .map_err(std::io::Error::from)?;
        Ok(lock)
    }

    pub(crate) fn live_read_guard(&self) -> Result<LiveReadGuard, EngineError> {
        let root = self.root_lock(false)?;
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        let legacy = match openat(
            &internal,
            "sources.lock",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let lock = File::from(fd);
                if !lock.metadata()?.is_file() {
                    return Err(EngineError::NotRegular("sources.lock".into()));
                }
                rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockShared)
                    .map_err(std::io::Error::from)?;
                Some(lock)
            }
            Err(rustix::io::Errno::NOENT) => None,
            Err(error) => return Err(std::io::Error::from(error).into()),
        };
        Ok(LiveReadGuard {
            _root: root,
            _legacy: legacy,
        })
    }

    /// Read at most 256 KiB from a regular UTF-8 page, without following links.
    /// The hash identifies exactly the returned bytes, including frontmatter.
    pub fn read_page(&self, page_id: &PageId) -> Result<PageSnapshot, EngineError> {
        let _guard = self.live_read_guard()?;
        self.require_recovered()?;
        let name = format!("{page_id}.md");
        let content = String::from_utf8(read_file(&self.pages, &name, MAX_PAGE_BYTES)?)?;
        Ok(PageSnapshot {
            api_version: API_VERSION,
            page_id: page_id.clone(),
            revision: content_hash(content.as_bytes()),
            content,
        })
    }

    /// Fail rather than silently read a different revision than the prepared one.
    pub fn read_revision(
        &self,
        page_id: &PageId,
        expected: &str,
    ) -> Result<PageSnapshot, EngineError> {
        let snapshot = self.read_page(page_id)?;
        if snapshot.revision != expected {
            return Err(EngineError::Conflict);
        }
        Ok(snapshot)
    }

    /// Revalidate local layout and config. No network or model credentials used.
    /// This is not yet a full wiki health check or a writability probe.
    pub fn doctor(&self) -> Result<DoctorReport, EngineError> {
        let config = load_config(&self.directory)?;
        for name in ["pages", "raw", ".ohmu"] {
            open_child_directory(&self.directory, name)?;
        }
        Ok(DoctorReport {
            api_version: API_VERSION,
            root: self.root.clone(),
            name: config.name,
            schema_version: config.schema_version,
            checks: vec!["config_valid".into(), "layout_valid".into()],
            implemented: vec![
                "init".into(),
                "doctor".into(),
                "read".into(),
                "local_document_ingest".into(),
                "source_catalog".into(),
                "url_ingest".into(),
                "search".into(),
                "page_relations".into(),
                "nausicaa_read_tool".into(),
                "drafts".into(),
                "apply_recover".into(),
                "snapshots".into(),
                "backup_restore".into(),
                "cited_workflows".into(),
            ],
            planned: vec!["scale_release_validation".into()],
        })
    }
}

#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn open_directory(path: &Path) -> Result<File, EngineError> {
    Ok(File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}

fn open_child_directory(parent: &File, name: &str) -> Result<File, EngineError> {
    Ok(File::from(
        openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}

fn write_new(parent: &File, name: &str, content: &[u8]) -> Result<(), EngineError> {
    let mut file = File::from(
        openat(
            parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .map_err(std::io::Error::from)?,
    );
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn read_file(parent: &File, name: &str, maximum: usize) -> Result<Vec<u8>, EngineError> {
    let mut file = File::from(
        openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(EngineError::NotRegular(name.to_owned()));
    }
    if before.len() > maximum as u64 {
        return Err(EngineError::TooLarge { maximum });
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(EngineError::TooLarge { maximum });
    }
    let after = file.metadata()?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || after.len() != bytes.len() as u64
    {
        return Err(EngineError::UnstableRead);
    }
    Ok(bytes)
}

fn load_config(directory: &File) -> Result<WikiConfig, EngineError> {
    let text = String::from_utf8(read_file(directory, CONFIG_FILE, MAX_CONFIG_BYTES)?)?;
    let config: WikiConfig =
        toml::from_str(&text).map_err(|error| EngineError::Config(error.to_string()))?;
    if config.schema_version != CONFIG_VERSION {
        return Err(EngineError::UnsupportedVersion(config.schema_version));
    }
    if config.name.trim().is_empty() {
        return Err(EngineError::EmptyName);
    }
    Ok(config)
}
