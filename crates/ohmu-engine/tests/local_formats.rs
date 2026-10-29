use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use ohmu_core::{MAX_PDF_PAGES, ParsedDocument};
use ohmu_engine::Wiki;
use pdf_extract::{Document, Object, Stream, dictionary};
use tempfile::tempdir;

fn html(title: &str, marker: &str) -> String {
    format!("<!doctype html><html><head><title>{title}</title></head><body><nav>SPONSORED_NAVIGATION</nav><article><h1>{title}</h1><p>{}</p><h2>第二节</h2><p>{}</p></article><script>SHOULD_NOT_BE_EVIDENCE</script></body></html>",
        format!("{marker}: This is a detailed research article with evidence and clear context. ").repeat(20),
        "第二节提供更多证据。 This paragraph contains further findings, details, and supporting analysis. ".repeat(20))
}

fn pdf_document(pages: &[&str]) -> Document {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let resources = document.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let mut kids = Vec::new();
    for text in pages {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = format!("BT /F1 12 Tf 50 700 Td ({escaped}) Tj ET");
        let contents = document.add_object(Stream::new(dictionary! {}, stream.into_bytes()));
        let page = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()],
            "Resources" => resources, "Contents" => contents,
        });
        kids.push(Object::Reference(page));
    }
    document.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages.len() as i64 }.into(),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog);
    document
}

fn write_pdf(path: &Path, pages: &[&str]) {
    pdf_document(pages).save(path).unwrap();
}

#[test]
fn html_article_is_normalized_with_headings_provenance_and_deduplication() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("article.HTML");
    let raw = html("Research 研究", "FIRSTMARKER");
    fs::write(&input, &raw).unwrap();
    let record = wiki.ingest_file(&input).unwrap().record;
    let parsed = wiki.source_document(&record).unwrap();
    assert_eq!(parsed.title, "Research 研究");
    assert!(parsed.text.contains("FIRSTMARKER"));
    assert!(!parsed.text.contains("SHOULD_NOT_BE_EVIDENCE"));
    assert!(!parsed.text.contains("SPONSORED_NAVIGATION"));
    assert!(parsed.nodes.iter().any(|node| node.title == "第二节"));
    assert_eq!(parsed.source_revision.as_ref(), Some(&record.revision));
    assert_eq!(
        fs::read(root.join("raw/blobs").join(record.revision.hex())).unwrap(),
        raw.as_bytes()
    );
    assert!(wiki.ingest_file(&input).unwrap().reused);
    let copy = temp.path().join("copy.htm");
    fs::write(&copy, raw).unwrap();
    let copied = wiki.ingest_file(copy).unwrap().record;
    assert_ne!(copied.source_id, record.source_id);
    assert_eq!(copied.artifact, record.artifact);
    fs::write(&input, html("Updated", "SECONDMARKER")).unwrap();
    let updated = wiki.ingest_file(&input).unwrap().record;
    assert_eq!(updated.source_id, record.source_id);
    assert_ne!(updated.revision, record.revision);
    assert!(
        wiki.source_document(&record)
            .unwrap()
            .text
            .contains("FIRSTMARKER")
    );
    assert!(wiki.ingest_markdown(input).is_err());
}

#[test]
fn pdf_passages_are_pinned_to_one_physical_page_and_catalog_rebuild_survives_input_deletion() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let input = temp.path().join("report.pdf");
    write_pdf(&input, &["First page evidence.", "Second page findings."]);
    let record = wiki.ingest_file(&input).unwrap().record;
    let document = wiki.source_document(&record).unwrap();
    assert_eq!(document.nodes.len(), 3);
    assert_eq!(document.nodes[1].page, Some(1));
    assert_eq!(document.nodes[2].page, Some(2));
    assert_eq!(document.nodes[1].end, document.nodes[2].start);
    let first = wiki.read_source(&record, 0, 262144).unwrap();
    assert_eq!(first.locator.page, Some(1));
    assert!(first.content.contains("First page evidence."));
    assert!(!first.content.contains("Second page"));
    let second = wiki
        .read_source(&record, first.next_offset.unwrap(), 262144)
        .unwrap();
    assert_eq!(second.locator.page, Some(2));
    assert!(second.content.contains("Second page findings."));
    assert!(second.next_offset.is_none());
    assert!(wiki.ingest_file(&input).unwrap().reused);
    let copy = temp.path().join("copy.pdf");
    fs::copy(&input, &copy).unwrap();
    assert_eq!(
        wiki.ingest_file(&copy).unwrap().record.artifact,
        record.artifact
    );
    fs::remove_file(input).unwrap();
    fs::remove_file(copy).unwrap();
    fs::remove_file(root.join(".ohmu/catalog.sqlite")).unwrap();
    assert_eq!(wiki.rebuild_source_catalog().unwrap(), 2);
    assert_eq!(wiki.source_document(&record).unwrap(), document);
}

#[test]
fn mixed_pdf_blank_pages_keep_physical_numbering_but_image_only_input_is_rejected() {
    let temp = tempdir().unwrap();
    let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
    let mixed = temp.path().join("mixed.pdf");
    write_pdf(&mixed, &["", "Text is on physical page two.", ""]);
    let record = wiki.ingest_file(mixed).unwrap().record;
    let document = wiki.source_document(&record).unwrap();
    assert_eq!(document.nodes[2].page, Some(2));
    assert!(
        document
            .diagnostics
            .iter()
            .any(|text| text.contains("physical page 1 has no extractable text"))
    );
    assert!(
        document
            .diagnostics
            .iter()
            .any(|text| text.contains("physical page 3 has no extractable text"))
    );
    let passage = wiki
        .read_source(&record, document.nodes[2].start, 1024)
        .unwrap();
    assert_eq!(passage.locator.page, Some(2));
    let blank = temp.path().join("blank.pdf");
    write_pdf(&blank, &["", ""]);
    let error = wiki.ingest_file(blank).unwrap_err().to_string();
    assert!(error.contains("OCR"), "{error}");
    let mut image_only = pdf_document(&[""]);
    let image = image_only.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        vec![255, 255, 255],
    ));
    let contents = image_only.add_object(Stream::new(
        dictionary! {},
        b"q 10 0 0 10 50 50 cm /Im0 Do Q".to_vec(),
    ));
    let page = image_only.get_pages()[&1];
    let page = image_only
        .get_object_mut(page)
        .unwrap()
        .as_dict_mut()
        .unwrap();
    page.set(
        "Resources",
        dictionary! { "XObject" => dictionary! { "Im0" => image } },
    );
    page.set("Contents", contents);
    let scan = temp.path().join("scan.pdf");
    image_only.save(&scan).unwrap();
    let error = wiki.ingest_file(scan).unwrap_err().to_string();
    assert!(error.contains("OCR"), "{error}");
    assert_eq!(wiki.sources().unwrap().len(), 1);
}

#[test]
fn media_mismatch_bad_encoding_and_script_only_html_do_not_commit_sources() {
    let temp = tempdir().unwrap();
    let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
    for (name, bytes) in [
        ("fake.pdf", b"# Not a PDF".as_slice()),
        ("corrupt.pdf", b"%PDF-1.5\nnot a real document".as_slice()),
        ("fake.html", b"Plain text is not HTML".as_slice()),
        (
            "encoding.html",
            b"<html><body>\xff</body></html>".as_slice(),
        ),
        (
            "app.html",
            b"<!doctype html><html><body><script>document.write('dynamic')</script></body></html>"
                .as_slice(),
        ),
        ("unsupported.docx", b"not supported".as_slice()),
    ] {
        let input = temp.path().join(name);
        fs::write(&input, bytes).unwrap();
        assert!(wiki.ingest_file(input).is_err(), "accepted {name}");
    }
    assert!(wiki.sources().unwrap().is_empty());
}

#[test]
fn encrypted_and_over_limit_pdf_inputs_are_rejected_before_source_commit() {
    let temp = tempdir().unwrap();
    let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
    for password in ["reader", ""] {
        let mut encrypted = pdf_document(&["Confidential"]);
        // A real password-encrypted PDF, not merely a filename or fake marker.
        encrypted.trailer.set(
            "ID",
            vec![
                Object::string_literal("fixture-id-12345"),
                Object::string_literal("fixture-id-12345"),
            ],
        );
        let version = pdf_extract::EncryptionVersion::V2 {
            document: &encrypted,
            owner_password: "owner",
            user_password: password,
            key_length: 128,
            permissions: pdf_extract::Permissions::all(),
        };
        let state = pdf_extract::EncryptionState::try_from(version).unwrap();
        encrypted.encrypt(&state).unwrap();
        let path = temp.path().join(if password.is_empty() {
            "empty-password.pdf"
        } else {
            "encrypted.pdf"
        });
        encrypted.save(&path).unwrap();
        let error = wiki.ingest_file(path).unwrap_err().to_string();
        assert!(error.contains("encrypted"), "{error}");
    }
    let path = temp.path().join("too-many.pdf");
    write_pdf(&path, &vec!["x"; MAX_PDF_PAGES + 1]);
    let error = wiki.ingest_file(path).unwrap_err().to_string();
    assert!(error.contains("physical pages"), "{error}");
    assert!(wiki.sources().unwrap().is_empty());
}

#[test]
fn normalized_cache_cannot_be_redirected_to_another_sources_artifact() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Wiki::init(&root, "test").unwrap();
    let a = temp.path().join("a.html");
    let b = temp.path().join("b.html");
    fs::write(&a, html("Article A", "EVIDENCE_A")).unwrap();
    fs::write(&b, html("Article B", "EVIDENCE_B")).unwrap();
    let a_record = wiki.ingest_file(&a).unwrap().record;
    let b_record = wiki.ingest_file(&b).unwrap().record;
    let cache = fs::read_dir(root.join("raw/parses"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(a_record.revision.hex())
        })
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    value["artifact"] = serde_json::json!(b_record.artifact);
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(cache, serde_json::to_vec(&value).unwrap()).unwrap();
    let copy = temp.path().join("copy.html");
    fs::copy(a, &copy).unwrap();
    let error = wiki.ingest_file(copy).unwrap_err().to_string();
    assert!(error.contains("raw source revision"), "{error}");
    assert_eq!(wiki.sources().unwrap().len(), 2);
}

#[test]
fn legacy_markdown_artifacts_keep_their_serialized_shape() {
    let old = r##"{"schema_version":1,"parser":"ohmu-markdown/1;pulldown-cmark/0.13.5","title":"Old","text":"# Old\n","nodes":[{"id":"root","parent":null,"title":"Old","start":0,"end":0,"page":null},{"id":"n000000","parent":"root","title":"Old","start":0,"end":6,"page":null}],"diagnostics":[]}"##;
    let parsed: ParsedDocument = serde_json::from_str(old).unwrap();
    assert!(parsed.source_revision.is_none());
    assert_eq!(serde_json::to_string(&parsed).unwrap(), old);
}
