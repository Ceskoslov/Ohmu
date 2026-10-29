//! Verified directory backups of authoritative content and durable state.
use crate::{
    EngineError, Wiki, content_hash, open_child_directory, open_directory, read_file,
    sources::{ensure_directory, source_error},
};
use ohmu_core::ContentHash;
use rusqlite::{Connection, backup::Backup};
use rustix::fs::{Dir, Mode, OFlags, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path},
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub schema_version: u32,
    pub id: ContentHash,
    pub files: BTreeMap<String, ContentHash>,
}

impl Wiki {
    /// Create a new backup directory. Derived catalogs/indexes and credentials
    /// are excluded; drafts, operations, snapshots and original evidence remain.
    pub fn backup(&self, destination: impl AsRef<Path>) -> Result<BackupManifest, EngineError> {
        let destination = destination.as_ref();
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        if parent.starts_with(&self.root) {
            return Err(source_error("backup destination must be outside the Wiki"));
        }
        let _lock = self.source_store()?;
        let before = self.page_contents()?;
        fs::create_dir(destination)?;
        let target = open_directory(destination)?;
        ensure_directory(&target, ".ohmu")?;
        let mut files = BTreeMap::new();
        copy_named(
            &self.directory,
            "ohmu.toml",
            &target,
            "ohmu.toml",
            "ohmu.toml",
            &mut files,
        )?;
        for name in ["pages", "raw"] {
            let source = open_child_directory(&self.directory, name)?;
            let output = ensure_directory(&target, name)?;
            copy_tree(&source, &output, name, &mut files)?;
        }
        let internal = open_child_directory(&self.directory, ".ohmu")?;
        let output = open_child_directory(&target, ".ohmu")?;
        for name in ["objects", "snapshots"] {
            match open_child_directory(&internal, name) {
                Ok(source) => {
                    let directory = ensure_directory(&output, name)?;
                    copy_tree(&source, &directory, &format!(".ohmu/{name}"), &mut files)?;
                }
                Err(EngineError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(source) = self.state_database(false)? {
            let path = destination.join(".ohmu/state.sqlite");
            let mut database = Connection::open(&path)?;
            Backup::new(&source, &mut database)?.run_to_completion(
                64,
                Duration::from_millis(5),
                None,
            )?;
            drop(database);
            File::open(&path)?.sync_all()?;
            files.insert(
                ".ohmu/state.sqlite".into(),
                file_hash(&open_regular(&output, "state.sqlite")?)?,
            );
        }
        if before != self.page_contents()? {
            return Err(EngineError::Conflict);
        }
        if files.keys().any(|path| !allowed_path(path)) {
            return Err(source_error(
                "backup contains an unmanaged authoritative path",
            ));
        }
        let id = hash(&serde_json::to_vec(&(1, &files))?)?;
        let manifest = BackupManifest {
            schema_version: 1,
            id,
            files,
        };
        let mut file = File::from(
            openat(
                &target,
                "backup.json",
                OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(std::io::Error::from)?,
        );
        file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        file.sync_all()?;
        output.sync_all()?;
        target.sync_all()?;
        open_directory(&parent)?.sync_all()?;
        Ok(manifest)
    }

    /// Restore a verified backup into a nonexistent directory. No in-place
    /// overwrite or format downgrade is attempted. Partial failures are retained.
    pub fn restore(
        backup: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<Wiki, EngineError> {
        let backup = backup.as_ref();
        let destination = destination.as_ref();
        let source = open_directory(backup)?;
        let manifest: BackupManifest =
            serde_json::from_slice(&read_file(&source, "backup.json", 64 * 1024 * 1024)?)?;
        if manifest.schema_version != 1
            || hash(&serde_json::to_vec(&(1, &manifest.files))?)? != manifest.id
            || !manifest.files.contains_key("ohmu.toml")
        {
            return Err(source_error("invalid backup manifest"));
        }
        for (path, expected) in &manifest.files {
            if !allowed_path(path) {
                return Err(source_error(
                    "backup path is outside the supported archive layout",
                ));
            }
            let (directory, name) = relative_parent(&source, path, false)?;
            if file_hash(&open_regular(&directory, &name)?)? != *expected {
                return Err(source_error(format!("backup integrity mismatch: {path}")));
            }
        }
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        if parent.starts_with(backup.canonicalize()?) {
            return Err(source_error(
                "restore destination must be outside the backup",
            ));
        }
        fs::create_dir(destination)?;
        let target = open_directory(destination)?;
        for directory in ["pages", "raw", ".ohmu"] {
            ensure_directory(&target, directory)?;
        }
        let mut copied = BTreeMap::new();
        for (path, expected) in &manifest.files {
            let (input, name) = relative_parent(&source, path, false)?;
            let (output, target_name) = relative_parent(&target, path, true)?;
            copy_named(&input, &name, &output, &target_name, path, &mut copied)?;
            if copied.get(path) != Some(expected) {
                return Err(source_error("backup changed while restoring"));
            }
        }
        target.sync_all()?;
        open_directory(&parent)?.sync_all()?;
        let wiki = Wiki::open(destination)?;
        wiki.require_recovered()?;
        wiki.rebuild_source_catalog()?;
        wiki.reconcile_pages()?;
        Ok(wiki)
    }
}

fn hash(bytes: &[u8]) -> Result<ContentHash, EngineError> {
    ContentHash::new(content_hash(bytes)).map_err(source_error)
}
fn open_regular(parent: &File, name: &str) -> Result<File, EngineError> {
    let file = File::from(
        openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    if !file.metadata()?.is_file() {
        return Err(EngineError::NotRegular(name.into()));
    }
    Ok(file)
}
fn file_hash(file: &File) -> Result<ContentHash, EngineError> {
    let mut file = file.try_clone()?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    ContentHash::new(format!("sha256:{:x}", hash.finalize())).map_err(source_error)
}
fn copy_named(
    source: &File,
    name: &str,
    target: &File,
    target_name: &str,
    path: &str,
    files: &mut BTreeMap<String, ContentHash>,
) -> Result<(), EngineError> {
    let mut input = open_regular(source, name)?;
    let before = input.metadata()?;
    let mut output = File::from(
        openat(
            target,
            target_name,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    let after = input.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(EngineError::UnstableRead);
    }
    output.sync_all()?;
    target.sync_all()?;
    files.insert(
        path.into(),
        ContentHash::new(format!("sha256:{:x}", digest.finalize())).map_err(source_error)?,
    );
    Ok(())
}
fn copy_tree(
    source: &File,
    target: &File,
    prefix: &str,
    files: &mut BTreeMap<String, ContentHash>,
) -> Result<(), EngineError> {
    for entry in Dir::read_from(source).map_err(std::io::Error::from)? {
        let entry = entry.map_err(std::io::Error::from)?;
        let name = entry.file_name().to_str().map_err(source_error)?;
        if name == "." || name == ".." || name.starts_with(".tmp-") || name.starts_with(".apply-") {
            continue;
        }
        let file = File::from(
            openat(
                source,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        if file.metadata()?.is_dir() {
            let directory = ensure_directory(target, name)?;
            copy_tree(&file, &directory, &format!("{prefix}/{name}"), files)?;
        } else {
            copy_named(
                source,
                name,
                target,
                name,
                &format!("{prefix}/{name}"),
                files,
            )?;
        }
    }
    Ok(())
}
fn relative_parent(root: &File, path: &str, create: bool) -> Result<(File, String), EngineError> {
    let path = Path::new(path);
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => parts.push(
                name.to_str()
                    .ok_or_else(|| source_error("backup path is not UTF-8"))?
                    .to_owned(),
            ),
            _ => return Err(source_error("backup path must stay inside its root")),
        }
    }
    let name = parts
        .pop()
        .ok_or_else(|| source_error("empty backup path"))?;
    let mut directory = root.try_clone()?;
    for part in parts {
        directory = if create {
            ensure_directory(&directory, &part)?
        } else {
            open_child_directory(&directory, &part)?
        };
    }
    Ok((directory, name))
}

fn allowed_path(path: &str) -> bool {
    if path == "ohmu.toml" || path == ".ohmu/state.sqlite" {
        return true;
    }
    let parts: Vec<_> = path.split('/').collect();
    let valid_hash = |value: &str| ContentHash::new(format!("sha256:{value}")).is_ok();
    match parts.as_slice() {
        ["pages", name] => name
            .strip_suffix(".md")
            .is_some_and(|id| ohmu_core::PageId::new(id).is_ok()),
        ["raw", "blobs", name] | [".ohmu", "objects", name] => valid_hash(name),
        ["raw", "artifacts", name] | [".ohmu", "snapshots", name] => {
            name.strip_suffix(".json").is_some_and(valid_hash)
        }
        ["raw", kind @ ("parses" | "records"), name] => {
            name.strip_suffix(".json").is_some_and(|name| {
                let hashes: Vec<_> = name.split('-').collect();
                hashes.len() == if *kind == "parses" { 2 } else { 3 }
                    && hashes.into_iter().all(valid_hash)
            })
        }
        _ => false,
    }
}
