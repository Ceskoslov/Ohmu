//! Reproducible synthetic corpus and lexical measurements, without model calls.
use ohmu_core::SearchCollection;
use ohmu_engine::{Wiki, content_hash};
use pdf_extract::{Document, Object, Stream, dictionary};
use serde_json::{Value, json};
use std::{fs, path::Path, sync::Arc, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn pdf(path: &Path, i: usize, text: &str) -> Result<()> {
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let font =
        doc.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
    let resources = doc.add_object(dictionary! {"Font"=>dictionary!{"F1"=>font}});
    let mut kids = vec![];
    for number in 0..(1 + i % 4) {
        let raw =
            format!("BT /F1 12 Tf 50 700 Td (Report {i} physical page {number}. {text}) Tj ET");
        let stream = doc.add_object(Stream::new(dictionary! {}, raw.into_bytes()));
        let page=doc.add_object(dictionary!{"Type"=>"Page","Parent"=>pages,"MediaBox"=>vec![0.into(),0.into(),600.into(),800.into()],"Resources"=>resources,"Contents"=>stream});
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(
        pages,
        dictionary! {"Type"=>"Pages","Kids"=>kids,"Count"=>(1+i%4) as i64}.into(),
    );
    let catalog = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
    doc.trailer.set("Root", catalog);
    doc.save(path)?;
    Ok(())
}
fn disk(path: &Path) -> Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() {
            bytes += disk(&path)?;
        } else {
            bytes += fs::metadata(path)?.len();
        }
    }
    Ok(bytes)
}
fn percentiles(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    json!({"count":values.len(),"min":values[0],"p50":values[(values.len()-1)*50/100],"p95":values[(values.len()*95).div_ceil(100)-1],"max":values[values.len()-1]})
}
const QUERIES: [&str; 8] = [
    "group007",
    "group099",
    "知识证据",
    "research",
    "physical page",
    "transaction recovery",
    "Ohmu",
    "no_such_zqx_marker",
];
fn generate(root: &Path, count: usize) -> Result<()> {
    fs::create_dir(root)?;
    fs::create_dir(root.join("inputs"))?;
    let wiki = Wiki::init(root.join("wiki"), "Scale fixture")?;
    let mut manifest = vec![];
    for i in 0..count {
        let target = match (i / 10) % 100 {
            0..=79 => 1024,
            80..=94 => 4096,
            95..=98 => 16384,
            _ => 65536,
        };
        let english = format!(
            "group{:03} research evidence transaction recovery measurement record{i}. ",
            i % 100
        );
        let mixed = format!("{english} 知识证据 可追溯资料 中文检索。 ");
        let (format, extension) = match i % 10 {
            0..=5 => ("markdown", "md"),
            6..=8 => ("html", "html"),
            _ => ("pdf", "pdf"),
        };
        let path = root
            .join("inputs")
            .join(format!("source-{i:05}.{extension}"));
        let text = if format == "pdf" {
            english.repeat(target / english.len() + 1)
        } else {
            mixed.repeat(target / mixed.len() + 1)
        };
        match format {
            "markdown" => fs::write(
                &path,
                format!("# Research 研究 {i}\n\n{text}\n\n## Details\n\n{english}\n"),
            )?,
            "html" => fs::write(
                &path,
                format!(
                    "<!doctype html><html><head><title>Research 研究 {i}</title></head><body><article><h1>Research 研究 {i}</h1><p>{text}</p><h2>Details</h2><p>{mixed}</p></article></body></html>"
                ),
            )?,
            _ => pdf(&path, i, &text)?,
        }
        let raw = fs::read(&path)?;
        manifest.push(json!({"path":path.file_name().unwrap().to_str().unwrap(),"format":format,"language":if format=="pdf" {"en"} else {"zh-en"},"bytes":raw.len(),"hash":content_hash(&raw)}));
        if i < count / 10 {
            let next = if i + 1 < count / 10 { i + 1 } else { 0 };
            fs::write(
                wiki.root().join(format!("pages/topic-{i:05}.md")),
                format!(
                    "---\nid: topic-{i:05}\ntitle: Research 研究 {i}\naliases: [概念{i}]\n---\n\n# Research 研究 {i}\n\n{mixed}\n\n[[topic-{next:05}]]\n"
                ),
            )?;
        }
    }
    let bytes = serde_json::to_vec(&manifest)?;
    fs::write(root.join("corpus.json"), &bytes)?;
    eprintln!("Generated {count} sources: {}", content_hash(&bytes));
    Ok(())
}
fn prepare(root: &Path) -> Result<()> {
    let wiki = Wiki::open(root.join("wiki"))?;
    let manifest: Vec<Value> = serde_json::from_slice(&fs::read(root.join("corpus.json"))?)?;
    let start = Instant::now();
    let mut raw = vec![];
    let mut normalized = vec![];
    for (i, item) in manifest.iter().enumerate() {
        let path = root
            .join("inputs")
            .join(item["path"].as_str().ok_or("invalid corpus path")?);
        let bytes = fs::read(&path)?;
        if item["hash"] != content_hash(&bytes) {
            return Err("corpus hash mismatch".into());
        }
        wiki.ingest_file(path)?;
        raw.push(bytes.len() as f64);
        if (i + 1) % 1000 == 0 {
            eprintln!(
                "Imported {} sources ({:.1}s)",
                i + 1,
                start.elapsed().as_secs_f64()
            );
        }
    }
    let ingest = start.elapsed().as_secs_f64();
    for record in wiki.sources()? {
        normalized.push(wiki.source_document(&record)?.text.len() as f64);
    }
    let start = Instant::now();
    let first = wiki.reindex(false)?;
    let index = start.elapsed().as_secs_f64();
    let start = Instant::now();
    wiki.reindex(true)?;
    let rebuild = start.elapsed().as_secs_f64();
    let start = Instant::now();
    wiki.reindex(false)?;
    let incremental = start.elapsed().as_secs_f64();
    fs::write(
        root.join("prepare.json"),
        serde_json::to_vec_pretty(
            &json!({"schema_version":1,"corpus_hash":content_hash(&fs::read(root.join("corpus.json"))?),"sources":manifest.len(),"sections":first.indexed_sections,"source_bytes":percentiles(raw),"normalized_bytes":percentiles(normalized),"ingest_seconds":ingest,"index_seconds":index,"rebuild_seconds":rebuild,"noop_incremental_seconds":incremental,"wiki_logical_bytes":disk(wiki.root())?,"index_logical_bytes":disk(&wiki.root().join(".ohmu/search-index"))?}),
        )?,
    )?;
    Ok(())
}
fn measure(root: &Path, iterations: usize) -> Result<()> {
    let wiki = Arc::new(Wiki::open(root.join("wiki"))?);
    let start = Instant::now();
    wiki.search(QUERIES[0], SearchCollection::All, 20)?;
    let first = start.elapsed().as_secs_f64() * 1000.;
    let mut live = vec![];
    for i in 0..iterations {
        let start = Instant::now();
        wiki.search(QUERIES[i % QUERIES.len()], SearchCollection::All, 20)?;
        live.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let start = Instant::now();
    let snapshot = wiki.freeze_snapshot()?;
    let freeze = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let view = wiki.frozen_view(&snapshot.id)?;
    let view_seconds = start.elapsed().as_secs_f64();
    let start = Instant::now();
    view.search(QUERIES[0], SearchCollection::All, 20)?;
    let frozen_first = start.elapsed().as_secs_f64() * 1000.;
    let mut frozen = vec![];
    for i in 0..iterations {
        let start = Instant::now();
        view.search(QUERIES[i % QUERIES.len()], SearchCollection::All, 20)?;
        frozen.push(start.elapsed().as_secs_f64() * 1000.);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema_version":1,"queries":QUERIES,"iterations":iterations,"top_k":20,"collection":"all","first_live_ms":first,"live_ms":percentiles(live),"snapshot_seconds":freeze,"frozen_build_seconds":view_seconds,"first_frozen_ms":frozen_first,"frozen_ms":percentiles(frozen),"cold_definition":"new process, OS cache not flushed; first live query only","prepare":serde_json::from_slice::<Value>(&fs::read(root.join("prepare.json"))?)?})
        )?
    );
    Ok(())
}
fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 3 {
        return Err(
            "usage: scale generate|prepare|measure <new-corpus-directory> [count/iterations]"
                .into(),
        );
    }
    let root = Path::new(&args[2]);
    match args[1].as_str() {
        "cold" => {
            let wiki = Wiki::open(root.join("wiki"))?;
            let i: usize = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(0);
            let query = QUERIES[i % QUERIES.len()];
            let start = Instant::now();
            wiki.search(query, SearchCollection::All, 20)?;
            println!(
                "{}",
                json!({"query":query,"ms":start.elapsed().as_secs_f64()*1000.})
            );
            Ok(())
        }
        "generate" => generate(
            root,
            args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(10000),
        ),
        "prepare" => prepare(root),
        "measure" => measure(
            root,
            args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(100),
        ),
        _ => Err("unknown benchmark mode".into()),
    }
}
