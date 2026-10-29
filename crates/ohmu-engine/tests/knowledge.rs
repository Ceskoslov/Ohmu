use ohmu_core::{PageId, SearchCollection};
use ohmu_engine::{EngineError, Wiki};
use std::fs;
use tempfile::tempdir;

fn page(root: &std::path::Path, id: &str, title: &str, aliases: &str, body: &str) {
    fs::write(
        root.join(format!("pages/{id}.md")),
        format!("---\nid: {id}\ntitle: {title}\naliases: {aliases}\n---\n# {title}\n{body}\n"),
    )
    .unwrap();
}

#[test]
fn aliases_wikilinks_markdown_links_and_orphan_diagnostics_use_live_files() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    page(
        &root,
        "model",
        "语言模型",
        "[LLM, 大语言模型]",
        "A concept.",
    );
    page(
        &root,
        "research",
        "Research",
        "[]",
        "[[LLM]] [concept](model.md#section) [[Missing]] `[[ignored]]`\n```\n[[alsoignored]]\n```",
    );
    let catalog = wiki.reconcile_pages().unwrap();
    let links: Vec<_> = catalog
        .links
        .iter()
        .filter(|link| link.from.as_str() == "research")
        .collect();
    assert_eq!(links.len(), 3);
    assert_eq!(catalog.dangling_links.len(), 1);
    assert!(
        !catalog
            .orphan_pages
            .contains(&PageId::new("model").unwrap())
    );
    assert!(
        catalog
            .orphan_pages
            .contains(&PageId::new("research").unwrap())
    );
    assert_eq!(
        wiki.page_neighbors(&PageId::new("model").unwrap())
            .unwrap()
            .incoming,
        vec![PageId::new("research").unwrap()]
    );
    page(&root, "other", "Other", "[LLM]", "Ambiguous alias.");
    let catalog = wiki.reconcile_pages().unwrap();
    assert_eq!(catalog.ambiguous_references, vec!["LLM"]);
    // Explicit IDs remain unambiguous even when an alias collides.
    assert!(catalog.links.iter().any(|link| link.reference == "model"
        && link.to.as_ref().is_some_and(|id| id.as_str() == "model")));
}

#[test]
fn duplicate_ids_invalid_metadata_and_yaml_aliases_are_rejected() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    for text in [
        "---\nid: welcome\n---\n# Collision",
        "---\nid: broken\nid: broken\n---\nBody",
        "---\nid: broken\naliases: [&alias x, *alias]\n---\nBody",
        "---\nid: broken\naliases: [\"\"]\n---\nBody",
        "---\nid: broken\nBody without terminator",
    ] {
        fs::write(root.join("pages/broken.md"), text).unwrap();
        assert!(wiki.reconcile_pages().is_err(), "accepted {text}");
    }
}

#[test]
fn page_pagination_handles_large_files_unicode_and_stale_revisions() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let content = "中文 Rust\n".repeat(40_000);
    fs::write(root.join("pages/large.md"), &content).unwrap();
    let id = PageId::new("large").unwrap();
    assert!(wiki.read_page(&id).is_err());
    let mut offset = 0;
    let mut actual = String::new();
    let first = wiki.read_page_passage(&id, 0, 65536).unwrap();
    loop {
        let part = wiki.read_page_passage(&id, offset, 65536).unwrap();
        actual.push_str(&part.content);
        match part.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(actual, content);
    assert!(wiki.read_page_passage(&id, 1, 100).is_err());
    fs::write(root.join("pages/large.md"), "edited").unwrap();
    assert_ne!(
        wiki.read_page_passage(&id, 0, 100).unwrap().revision,
        first.revision
    );
}

#[test]
fn lexical_collections_chinese_aliases_source_evidence_and_incremental_updates() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    page(
        &root,
        "rust",
        "Rust 实践",
        "[系统编程, rustlang]",
        "Rust provides memory safety. 中文检索支持知识库。中华人民共和国。",
    );
    let input = temp.path().join("paper.md");
    fs::write(
        &input,
        "# Source report\nExclusiveSourceEvidence about memory safety.\n",
    )
    .unwrap();
    let record = wiki.ingest_file(&input).unwrap().record;
    assert!(wiki.search("Rust", SearchCollection::Wiki, 20).is_err());
    let report = wiki.reindex(false).unwrap();
    assert_eq!(report.updated_entities, 3);
    for query in ["Rust", "rustlang", "系统编程", "知识库", "共和国"] {
        let result = wiki.search(query, SearchCollection::Wiki, 20).unwrap();
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.page_id.as_ref().is_some_and(|id| id.as_str() == "rust")),
            "query {query}: {result:?}"
        );
    }
    assert!(
        wiki.search("ExclusiveSourceEvidence", SearchCollection::Wiki, 20)
            .unwrap()
            .hits
            .is_empty()
    );
    let source = wiki
        .search("ExclusiveSourceEvidence", SearchCollection::Sources, 20)
        .unwrap();
    assert_eq!(source.hits[0].source.as_ref(), Some(&record));
    assert!(source.hits[0].excerpt.contains("ExclusiveSourceEvidence"));
    assert_eq!(wiki.reindex(false).unwrap().updated_entities, 0);
    page(
        &root,
        "rust",
        "Rust revised",
        "[rustlang]",
        "NewMemoryEvidence",
    );
    assert!(matches!(
        wiki.search("NewMemoryEvidence", SearchCollection::Wiki, 20),
        Err(EngineError::StaleIndex(_))
    ));
    assert_eq!(wiki.reindex(false).unwrap().updated_entities, 1);
    assert!(
        !wiki
            .search("NewMemoryEvidence", SearchCollection::Wiki, 20)
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(
        wiki.search("知识库", SearchCollection::Wiki, 20)
            .unwrap()
            .hits
            .is_empty()
    );
    fs::remove_file(root.join("pages/rust.md")).unwrap();
    assert!(wiki.search("rustlang", SearchCollection::Wiki, 20).is_err());
    assert_eq!(wiki.reindex(false).unwrap().removed_entities, 1);
    assert!(
        wiki.search("rustlang", SearchCollection::Wiki, 20)
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn missing_caches_rebuild_and_raw_integrity_prevent_stale_answers() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Alpha\nOriginalSourceMarker\n").unwrap();
    let record = wiki.ingest_file(&input).unwrap().record;
    wiki.reindex(false).unwrap();
    fs::remove_file(root.join(".ohmu/catalog.sqlite")).unwrap();
    fs::remove_dir_all(root.join(".ohmu/search-index")).unwrap();
    fs::remove_file(&input).unwrap();
    assert!(
        wiki.search("OriginalSourceMarker", SearchCollection::Sources, 20)
            .is_err()
    );
    wiki.reindex(false).unwrap();
    assert!(
        !wiki
            .search("OriginalSourceMarker", SearchCollection::Sources, 20)
            .unwrap()
            .hits
            .is_empty()
    );
    let artifact = root.join(format!("raw/artifacts/{}.json", record.artifact.hex()));
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(artifact, "{}").unwrap();
    assert!(
        wiki.search("OriginalSourceMarker", SearchCollection::Sources, 20)
            .is_err()
    );
}

#[test]
fn new_sources_and_interrupted_index_manifests_require_reindexing() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    wiki.reindex(false).unwrap();
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Added\nBrandNewSource\n").unwrap();
    wiki.ingest_file(input).unwrap();
    assert!(
        wiki.search("BrandNewSource", SearchCollection::Sources, 20)
            .is_err()
    );
    wiki.reindex(false).unwrap();
    let db = rusqlite::Connection::open(root.join(".ohmu/catalog.sqlite")).unwrap();
    let json: String = db
        .query_row("SELECT state_json FROM index_state", [], |r| r.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["opstamp"] = serde_json::json!(99999999);
    db.execute("UPDATE index_state SET state_json=?1", [value.to_string()])
        .unwrap();
    drop(db);
    assert!(
        wiki.search("BrandNewSource", SearchCollection::Sources, 20)
            .is_err()
    );
    assert!(wiki.reindex(false).unwrap().updated_entities > 0);
    assert!(
        !wiki
            .search("BrandNewSource", SearchCollection::Sources, 20)
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(wiki.search("", SearchCollection::All, 20).is_err());
    assert!(wiki.search("Rust", SearchCollection::All, 0).is_err());
}

#[test]
fn catalog_v1_is_migrated_additively_and_source_nodes_are_navigable() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let db = rusqlite::Connection::open(root.join(".ohmu/catalog.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE sources(source_id TEXT,revision TEXT,parser TEXT,imported_at TEXT,record_json TEXT,PRIMARY KEY(source_id,revision,parser)); PRAGMA user_version=1;").unwrap();
    drop(db);
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Parent\nText\n## Child\nDetails\n").unwrap();
    let record = wiki.ingest_file(input).unwrap().record;
    let tree = wiki.source_neighbors(&record, "n000000").unwrap();
    assert_eq!(tree.children.len(), 1);
    assert!(wiki.source_neighbors(&record, "fabricated").is_err());
    let db = rusqlite::Connection::open(root.join(".ohmu/catalog.sqlite")).unwrap();
    let version: u32 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
}
