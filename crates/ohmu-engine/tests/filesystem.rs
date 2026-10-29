use std::fs;
use std::os::unix::fs::symlink;

use ohmu_core::{MAX_PAGE_BYTES, PageId};
use ohmu_engine::{EngineError, Wiki, content_hash};
use tempfile::tempdir;

#[test]
fn initialization_read_and_external_edit_preserve_markdown_authority() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "研究 Wiki").unwrap();
    let id = PageId::new("welcome").unwrap();
    let before = wiki.read_page(&id).unwrap();
    assert_eq!(before.revision, content_hash(before.content.as_bytes()));
    fs::write(
        root.join("pages/welcome.md"),
        "# 新内容\nExternally edited.\n",
    )
    .unwrap();
    let after = wiki.read_page(&id).unwrap();
    assert_ne!(before.revision, after.revision);
    assert!(after.content.contains("Externally edited"));
    assert!(matches!(
        wiki.read_revision(&id, &before.revision),
        Err(EngineError::Conflict)
    ));
    assert_eq!(wiki.doctor().unwrap().name, "研究 Wiki");
    assert!(Wiki::init(&root, "replacement").is_err());
    assert_eq!(
        fs::read_to_string(root.join("pages/welcome.md")).unwrap(),
        after.content
    );
}

#[test]
fn ids_cannot_be_paths_even_when_deserialized() {
    for id in [
        "",
        "../secret",
        "/etc/passwd",
        "a/b",
        "a\\b",
        ".hidden",
        "A",
        "中文",
        "a.md",
        "-x",
    ] {
        assert!(PageId::new(id).is_err(), "accepted {id:?}");
        assert!(serde_json::from_str::<PageId>(&serde_json::to_string(id).unwrap()).is_err());
    }
    assert!(PageId::new("x".repeat(97)).is_err());
    assert!(PageId::new("research_2026-v1").is_ok());
}

#[test]
fn refuses_symlinked_pages_and_layout_components() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    fs::write(temp.path().join("secret"), "not wiki content").unwrap();
    symlink(temp.path().join("secret"), root.join("pages/leak.md")).unwrap();
    assert!(wiki.read_page(&PageId::new("leak").unwrap()).is_err());
    fs::rename(root.join("pages"), root.join("saved-pages")).unwrap();
    symlink(root.join("saved-pages"), root.join("pages")).unwrap();
    assert!(Wiki::open(&root).is_err());
    assert!(wiki.doctor().is_err());
    // The existing descriptor still refers to its original directory.
    assert!(wiki.read_page(&PageId::new("welcome").unwrap()).is_ok());
}

#[test]
fn directory_replacement_does_not_redirect_an_open_reader() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let id = PageId::new("welcome").unwrap();
    let original = wiki.read_page(&id).unwrap();
    fs::rename(root.join("pages"), root.join("original")).unwrap();
    fs::create_dir(root.join("pages")).unwrap();
    fs::write(root.join("pages/welcome.md"), "replacement").unwrap();
    assert_eq!(wiki.read_page(&id).unwrap(), original);
    assert_eq!(
        Wiki::open(&root).unwrap().read_page(&id).unwrap().content,
        "replacement"
    );
}

#[test]
fn rejects_unsupported_corrupt_or_symlinked_config() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    fs::write(
        root.join("ohmu.toml"),
        "schema_version = 99\nname = 'test'\n",
    )
    .unwrap();
    assert!(matches!(
        Wiki::open(&root),
        Err(EngineError::UnsupportedVersion(99))
    ));
    assert!(wiki.doctor().is_err());
    fs::write(
        root.join("ohmu.toml"),
        "schema_version = 1\nname = 'test'\nunknown = true\n",
    )
    .unwrap();
    assert!(matches!(Wiki::open(&root), Err(EngineError::Config(_))));
    fs::rename(root.join("ohmu.toml"), root.join("real.toml")).unwrap();
    symlink("real.toml", root.join("ohmu.toml")).unwrap();
    assert!(Wiki::open(&root).is_err());
}

#[test]
fn rejects_missing_nonregular_oversized_and_non_utf8_pages() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    assert!(wiki.read_page(&PageId::new("missing").unwrap()).is_err());
    fs::create_dir(root.join("pages/directory.md")).unwrap();
    assert!(matches!(
        wiki.read_page(&PageId::new("directory").unwrap()),
        Err(EngineError::NotRegular(_))
    ));
    fs::write(root.join("pages/large.md"), vec![b'x'; MAX_PAGE_BYTES + 1]).unwrap();
    assert!(matches!(
        wiki.read_page(&PageId::new("large").unwrap()),
        Err(EngineError::TooLarge { .. })
    ));
    fs::write(root.join("pages/binary.md"), [0xff, 0xfe]).unwrap();
    assert!(matches!(
        wiki.read_page(&PageId::new("binary").unwrap()),
        Err(EngineError::Utf8(_))
    ));
}

#[test]
fn invalid_name_does_not_create_a_directory() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    assert!(matches!(
        Wiki::init(&root, " \n"),
        Err(EngineError::EmptyName)
    ));
    assert!(!root.exists());
}
