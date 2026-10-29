//! Metadata is a same-query race detector, never an authoritative content cache.
use crate::{EngineError, Wiki, open_child_directory, sources::source_error};
use rustix::fs::{AtFlags, Dir, FileType, statat};
use std::{collections::BTreeMap, fs::File};

type Stamp = BTreeMap<String, [i128; 8]>;
fn entry(directory: &File, name: &str) -> Result<[i128; 8], EngineError> {
    let s = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    if name != "." && FileType::from_raw_mode(s.st_mode) != FileType::RegularFile {
        return Err(EngineError::NotRegular(name.into()));
    }
    Ok([
        s.st_dev as i128,
        s.st_ino as i128,
        s.st_mode as i128,
        s.st_size as i128,
        s.st_mtime as i128,
        s.st_mtime_nsec as i128,
        s.st_ctime as i128,
        s.st_ctime_nsec as i128,
    ])
}
fn directory_stamp(
    directory: &File,
    prefix: &str,
    suffix: &str,
    result: &mut Stamp,
) -> Result<(), EngineError> {
    result.insert(format!("{prefix}/."), entry(directory, ".")?);
    for item in Dir::read_from(directory).map_err(std::io::Error::from)? {
        let item = item.map_err(std::io::Error::from)?;
        let name = item.file_name().to_str().map_err(source_error)?;
        if name.ends_with(suffix) {
            result.insert(format!("{prefix}/{name}"), entry(directory, name)?);
        }
    }
    Ok(())
}
impl Wiki {
    pub(crate) fn live_stamp(&self) -> Result<Stamp, EngineError> {
        let mut stamp = Stamp::new();
        directory_stamp(&self.pages, "pages", ".md", &mut stamp)?;
        let raw = open_child_directory(&self.directory, "raw")?;
        stamp.insert("raw/.".into(), entry(&raw, ".")?);
        match open_child_directory(&raw, "records") {
            Ok(records) => directory_stamp(&records, "records", ".json", &mut stamp)?,
            Err(EngineError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(stamp)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn stamps_detect_same_size_restored_mtime_replacements_additions_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
        let path = wiki.root().join("pages/welcome.md");
        let before = wiki.live_stamp().unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] = b'X';
        fs::write(&path, &bytes).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_modified(metadata.modified().unwrap())
            .unwrap();
        assert_ne!(wiki.live_stamp().unwrap(), before);
        let before = wiki.live_stamp().unwrap();
        fs::rename(&path, wiki.root().join("pages/old.tmp")).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert_ne!(wiki.live_stamp().unwrap(), before);
        let before = wiki.live_stamp().unwrap();
        fs::write(wiki.root().join("pages/new.md"), "# New\n").unwrap();
        assert_ne!(wiki.live_stamp().unwrap(), before);
        std::os::unix::fs::symlink(&path, wiki.root().join("pages/link.md")).unwrap();
        assert!(wiki.live_stamp().is_err());
    }
}
