use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use ohmu_core::{ContentHash, SourceId};
use ohmu_engine::Wiki;
use rusqlite::Connection;
use tempfile::tempdir;

fn entries(path: impl AsRef<std::path::Path>) -> usize {
    fs::read_dir(path)
        .unwrap()
        .filter(|entry| {
            !entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        })
        .count()
}

#[test]
fn import_retains_history_and_reuses_content_across_distinct_origins() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let a = temp.path().join("a.md");
    let b = temp.path().join("b.md");
    let text = "# 研究\n\nSame source text.\n\n## Details\n更多证据。\n";
    fs::write(&a, text).unwrap();
    fs::write(&b, text).unwrap();
    let first = wiki.ingest_markdown(&a).unwrap();
    assert!(!first.reused);
    let duplicate = wiki.ingest_markdown(&a).unwrap();
    assert!(duplicate.reused);
    assert_eq!(first.record, duplicate.record);
    let other = wiki.ingest_markdown(&b).unwrap();
    assert_ne!(first.record.source_id, other.record.source_id);
    assert_eq!(first.record.revision, other.record.revision);
    assert_eq!(first.record.artifact, other.record.artifact);
    for directory in ["blobs", "artifacts", "parses"] {
        assert_eq!(entries(root.join("raw").join(directory)), 1);
    }
    assert_eq!(entries(root.join("raw/records")), 2);
    fs::write(&a, "# Revised\nNew facts.\n").unwrap();
    let revised = wiki.ingest_markdown(&a).unwrap();
    assert_eq!(revised.record.source_id, first.record.source_id);
    assert_ne!(revised.record.revision, first.record.revision);
    fs::remove_file(&a).unwrap();
    assert_eq!(wiki.source_document(&first.record).unwrap().text, text);
    assert_eq!(wiki.sources().unwrap().len(), 3);
    // Importing sources never publishes model/user source text as a Wiki page.
    assert_eq!(entries(root.join("pages")), 1);
}

#[test]
fn catalog_can_be_deleted_and_rebuilt_without_original_inputs() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Paper\nCited evidence.\n").unwrap();
    let record = wiki.ingest_markdown(&input).unwrap().record;
    fs::remove_file(input).unwrap();
    fs::remove_file(root.join(".ohmu/catalog.sqlite")).unwrap();
    assert!(wiki.sources().unwrap().is_empty());
    assert_eq!(wiki.rebuild_source_catalog().unwrap(), 1);
    assert_eq!(wiki.sources().unwrap(), vec![record.clone()]);
    assert_eq!(wiki.source_document(&record).unwrap().title, "Paper");
    assert_eq!(wiki.rebuild_source_catalog().unwrap(), 1);
}

#[test]
fn heading_tree_and_utf8_locators_preserve_exact_evidence() {
    let temp = tempdir().unwrap();
    let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
    let input = temp.path().join("研究.md");
    let text = "前言\n# Topic\n正文\n## **Child**\n细节\n### Grandchild\nDeep\n## Sibling\nEnd\n";
    fs::write(&input, text).unwrap();
    let record = wiki.ingest_markdown(input).unwrap().record;
    let doc = wiki.source_document(&record).unwrap();
    assert_eq!(doc.title, "Topic");
    assert_eq!(&doc.text[doc.nodes[0].start..doc.nodes[0].end], "前言\n");
    assert_eq!(doc.nodes[2].title, "Child");
    assert_eq!(doc.nodes[2].parent.as_deref(), Some("n000000"));
    assert_eq!(doc.nodes[3].parent.as_deref(), Some("n000001"));
    assert_eq!(doc.nodes[4].parent.as_deref(), Some("n000000"));
    let mut offset = 0;
    let mut reconstructed = String::new();
    loop {
        let part = wiki.read_source(&record, offset, 7).unwrap();
        assert_eq!(part.content, text[part.locator.start..part.locator.end]);
        assert_eq!(part.locator.artifact, record.artifact);
        reconstructed.push_str(&part.content);
        match part.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(reconstructed, text);
    assert!(wiki.read_source(&record, 1, 10).is_err());
    assert!(wiki.read_source(&record, 0, 1).is_err());
    assert!(wiki.read_source(&record, 0, 0).is_err());
    assert!(wiki.read_source(&record, usize::MAX, 10).is_err());
}

#[test]
fn invalid_input_and_symlinks_do_not_commit_source_records() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("bad.md");
    fs::write(&input, [0xff]).unwrap();
    assert!(wiki.ingest_markdown(&input).is_err());
    fs::write(&input, " \n").unwrap();
    assert!(wiki.ingest_markdown(&input).is_err());
    fs::write(&input, "# Valid\n").unwrap();
    let link = temp.path().join("link.md");
    symlink(&input, &link).unwrap();
    assert!(wiki.ingest_markdown(link).is_err());
    fs::rename(root.join("raw/blobs"), root.join("saved-blobs")).unwrap();
    symlink(root.join("saved-blobs"), root.join("raw/blobs")).unwrap();
    assert!(wiki.ingest_markdown(input).is_err());
    assert!(wiki.sources().unwrap().is_empty());
}

#[test]
fn corrupted_artifact_or_manifest_is_rejected_without_replacing_catalog_rows() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Evidence\nTrusted snapshot.\n").unwrap();
    let record = wiki.ingest_markdown(&input).unwrap().record;
    let artifact = root.join(format!("raw/artifacts/{}.json", record.artifact.hex()));
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(artifact, "{}").unwrap();
    assert!(wiki.source_document(&record).is_err());
    assert!(wiki.rebuild_source_catalog().is_err());
    assert!(wiki.ingest_markdown(&input).is_err());
    assert_eq!(wiki.sources().unwrap(), vec![record]);
}

#[test]
fn future_catalog_version_is_not_reinterpreted_and_writer_lock_is_nonblocking() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Evidence\n").unwrap();
    let connection = Connection::open(root.join(".ohmu/catalog.sqlite")).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert!(wiki.ingest_markdown(&input).is_err());
    assert_eq!(entries(root.join("raw/records")), 0);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".ohmu/sources.lock"))
        .unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let error = wiki.ingest_markdown(input).unwrap_err().to_string();
    assert!(error.contains("I/O"));
}

#[test]
fn hashed_ids_cannot_be_used_as_paths() {
    for value in ["../x", "sha256:../x", "sha256:ABC", "sha256:00"] {
        assert!(ContentHash::new(value).is_err());
        assert!(
            serde_json::from_str::<ContentHash>(&serde_json::to_string(value).unwrap()).is_err()
        );
        assert!(SourceId::new(value).is_err());
    }
}
