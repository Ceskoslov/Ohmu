use ohmu_core::{ChangeProposal, PageChange, PageId};
use ohmu_engine::Wiki;
use std::{fs, os::unix::fs::symlink};
use tempfile::tempdir;

#[test]
fn backup_roundtrip_preserves_sources_drafts_snapshots_and_publication_idempotency() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let source = temp.path().join("paper.md");
    fs::write(&source, "# Source\nEvidence remains portable.\n").unwrap();
    let record = wiki.ingest_file(&source).unwrap().record;
    let draft = wiki
        .stage(ChangeProposal {
            summary: "Add note".into(),
            pages: vec![PageChange {
                page_id: PageId::new("note").unwrap(),
                expected_revision: None,
                content: "# Portable note\n[[welcome]]\n".into(),
                citations: vec![],
            }],
        })
        .unwrap();
    let result = wiki.apply(&draft.id).unwrap();
    let snapshot = wiki.freeze_snapshot().unwrap();
    wiki.reindex(false).unwrap();
    let backup = temp.path().join("backup");
    let manifest = wiki.backup(&backup).unwrap();
    assert!(manifest.files.contains_key(".ohmu/state.sqlite"));
    assert!(
        !manifest
            .files
            .keys()
            .any(|path| path.contains("search-index") || path.ends_with("catalog.sqlite"))
    );
    fs::remove_file(source).unwrap();
    let restored = Wiki::restore(&backup, temp.path().join("restored")).unwrap();
    assert_eq!(restored.draft(&draft.id).unwrap(), draft);
    assert_eq!(restored.snapshot(&snapshot.id).unwrap(), snapshot);
    assert_eq!(
        restored.apply(&draft.id).unwrap().operation_id,
        result.operation_id
    );
    assert!(
        restored
            .source_document(&record)
            .unwrap()
            .text
            .contains("Evidence remains portable")
    );
    restored.reindex(false).unwrap();
    assert!(
        !restored
            .search("Portable", ohmu_core::SearchCollection::Wiki, 20)
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(wiki.backup(&backup).is_err());
    assert!(wiki.backup(root.join("nested")).is_err());
    assert!(Wiki::restore(&backup, &root).is_err());
}

#[test]
fn corrupt_or_traversing_backups_fail_before_creating_restore_targets() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let backup = temp.path().join("backup");
    wiki.backup(&backup).unwrap();
    fs::write(backup.join("pages/welcome.md"), "tampered").unwrap();
    let destination = temp.path().join("restore");
    assert!(Wiki::restore(&backup, &destination).is_err());
    assert!(!destination.exists());
    let backup2 = temp.path().join("backup2");
    wiki.backup(&backup2).unwrap();
    fs::remove_file(backup2.join("pages/welcome.md")).unwrap();
    symlink(
        root.join("pages/welcome.md"),
        backup2.join("pages/welcome.md"),
    )
    .unwrap();
    assert!(Wiki::restore(&backup2, temp.path().join("restore2")).is_err());
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(backup2.join("backup.json")).unwrap()).unwrap();
    value["schema_version"] = serde_json::json!(99);
    fs::write(
        backup2.join("backup.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert!(Wiki::restore(&backup2, temp.path().join("restore3")).is_err());
}
