mod model;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ohmu_core::{
    API_VERSION, ContentHash, InvalidDigest, InvalidPageId, PageId, SearchCollection, SourceId,
};
use ohmu_engine::{EngineError, RecoveryMode, Wiki};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Parser)]
#[command(name = "ohmu", version, about = "Vectorless Markdown wiki")]
struct Cli {
    /// Emit versioned JSON. Runtime errors go to stderr; clap usage errors exit 2.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new wiki directory. The parent must exist; existing paths are refused.
    Init {
        path: PathBuf,
        #[arg(long, default_value = "Ohmu Wiki")]
        name: String,
    },
    /// Check configuration and layout without model credentials or writes.
    Doctor {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Read one regular UTF-8 page by ID with its exact SHA-256 revision.
    Read {
        page_id: String,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        revision: Option<String>,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Import a local Markdown, HTML or text PDF source without publishing a Wiki page.
    Ingest {
        path: PathBuf,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// List immutable source revisions from the derived catalog.
    Sources {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Read a pinned source revision with UTF-8-safe byte pagination.
    Source {
        source_id: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        artifact: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 16384)]
        limit: usize,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Rebuild the source catalog from validated provenance sidecars.
    CatalogRebuild {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Explicitly fetch and import one public HTTP(S) source.
    IngestUrl {
        url: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Reconcile page metadata and inspect orphan, dangling and ambiguous links.
    Lint {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Read current incoming/outgoing links for a page ID.
    Neighbors {
        page_id: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Navigate a pinned source's document tree.
    SourceNeighbors {
        source_id: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        artifact: Option<String>,
        #[arg(long, default_value = "root")]
        node: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Incrementally reconcile and index current Wiki sections and source revisions.
    Reindex {
        #[arg(long)]
        rebuild: bool,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Literal BM25 search, using the same Chinese/English tokenizer as indexing.
    Search {
        query: String,
        #[arg(long, default_value = "wiki", value_parser = ["wiki", "sources", "all"])]
        collection: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Validate and stage a JSON proposal; never publish automatically.
    Stage {
        file: PathBuf,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Drafts {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Draft {
        id: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Diff {
        id: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Publish exactly the immutable draft identified by this digest.
    Apply {
        id: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Operations {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Operation {
        id: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Reconcile a pending operation using recorded bytes; divergent edits fail.
    Recover {
        id: String,
        #[arg(long,value_parser=["finish","rollback"])]
        strategy: String,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Snapshot {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    SnapshotRead {
        snapshot: String,
        page_id: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 16384)]
        limit: usize,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    Backup {
        destination: PathBuf,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Ask a cited question using an explicitly configured live provider.
    Ask(model::ModelArgs),
    /// Generate a cited draft; never automatically publish.
    Compile(model::ModelArgs),
    Restore {
        backup: PathBuf,
        destination: PathBuf,
    },
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    PageId(#[from] InvalidPageId),
    #[error(transparent)]
    Digest(#[from] InvalidDigest),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Model(String),
}

fn run(command: Command) -> Result<(&'static str, Value, String), CliError> {
    match command {
        Command::Ask(args) => model::run(args, ohmu_nausicaa::workflow::WorkflowKind::Ask),
        Command::Compile(args) => model::run(args, ohmu_nausicaa::workflow::WorkflowKind::Compile),
        Command::Init { path, name } => {
            let wiki = Wiki::init(&path, &name)?;
            Ok((
                "init",
                json!({"root": wiki.root(), "config": wiki.config()}),
                format!("Initialized {}", wiki.root().display()),
            ))
        }
        Command::Doctor { wiki } => {
            let report = Wiki::open(wiki)?.doctor()?;
            let text = format!(
                "OK: {} ({})\nConfig v{}; layout valid.\nImplemented: {}\nPlanned: {}",
                report.name,
                report.root.display(),
                report.schema_version,
                report.implemented.join(", "),
                report.planned.join(", ")
            );
            Ok(("doctor", serde_json::to_value(report)?, text))
        }
        Command::Read {
            wiki,
            page_id,
            offset,
            limit,
            revision,
        } => {
            let wiki = Wiki::open(wiki)?;
            let id = PageId::new(page_id)?;
            if offset.is_some() || limit.is_some() {
                let passage =
                    wiki.read_page_passage(&id, offset.unwrap_or(0), limit.unwrap_or(16384))?;
                if revision.is_some_and(|revision| revision != passage.revision) {
                    return Err(EngineError::Conflict.into());
                }
                let text = passage.content.clone();
                return Ok(("read", serde_json::to_value(passage)?, text));
            }
            let snapshot = wiki.read_page(&id)?;
            if revision.is_some_and(|revision| revision != snapshot.revision) {
                return Err(EngineError::Conflict.into());
            }
            let text = format!(
                "{} @ {}\n\n{}",
                snapshot.page_id, snapshot.revision, snapshot.content
            );
            Ok(("read", serde_json::to_value(snapshot)?, text))
        }
        Command::Ingest { path, wiki } => {
            let report = Wiki::open(wiki)?.ingest_file(path)?;
            let mut text = format!(
                "{} @ {}\n{} nodes; reused: {}",
                report.record.source_id, report.record.revision, report.node_count, report.reused
            );
            if !report.diagnostics.is_empty() {
                text.push_str("\nDiagnostics:\n");
                text.push_str(&report.diagnostics.join("\n"));
            }
            Ok(("ingest", serde_json::to_value(report)?, text))
        }
        Command::Sources {
            wiki,
            offset,
            limit,
        } => {
            if !(1..=1000).contains(&limit) {
                return Err(
                    EngineError::Source("source-list limit must be 1..=1000".into()).into(),
                );
            }
            let records = Wiki::open(wiki)?.sources()?;
            let total = records.len();
            let records: Vec<_> = records.into_iter().skip(offset).take(limit).collect();
            let text = records
                .iter()
                .map(|record| {
                    format!(
                        "{} @ {}  {}",
                        record.source_id, record.revision, record.origin
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok((
                "sources",
                json!({"sources": records,"total":total,"next_offset":(offset.saturating_add(limit)<total).then_some(offset.saturating_add(limit))}),
                text,
            ))
        }
        Command::Source {
            source_id,
            revision,
            artifact,
            offset,
            limit,
            wiki,
        } => {
            let wiki = Wiki::open(wiki)?;
            let source_id = SourceId::new(source_id)?;
            let revision = ContentHash::new(revision)?;
            let artifact = artifact.map(ContentHash::new).transpose()?;
            let record = wiki.find_source(&source_id, &revision, artifact.as_ref())?;
            let passage = wiki.read_source(&record, offset, limit)?;
            let text = passage.content.clone();
            Ok(("source", serde_json::to_value(passage)?, text))
        }
        Command::CatalogRebuild { wiki } => {
            let count = Wiki::open(wiki)?.rebuild_source_catalog()?;
            Ok((
                "catalog-rebuild",
                json!({"source_revisions": count}),
                format!("Rebuilt catalog: {count} source revisions"),
            ))
        }
        Command::IngestUrl { url, wiki } => {
            let report = Wiki::open(wiki)?.ingest_url(&url)?;
            let text = format!("{} @ {}", report.record.source_id, report.record.revision);
            Ok(("ingest-url", serde_json::to_value(report)?, text))
        }
        Command::Lint { wiki } => {
            let report = Wiki::open(wiki)?.reconcile_pages()?;
            let text = format!(
                "{} pages; {} orphan pages; {} dangling links; {} ambiguous references",
                report.pages.len(),
                report.orphan_pages.len(),
                report.dangling_links.len(),
                report.ambiguous_references.len()
            );
            Ok(("lint", serde_json::to_value(report)?, text))
        }
        Command::Neighbors { page_id, wiki } => {
            let report = Wiki::open(wiki)?.page_neighbors(&PageId::new(page_id)?)?;
            let text = serde_json::to_string_pretty(&report)?;
            Ok(("neighbors", serde_json::to_value(report)?, text))
        }
        Command::SourceNeighbors {
            source_id,
            revision,
            artifact,
            node,
            wiki,
        } => {
            let wiki = Wiki::open(wiki)?;
            let artifact = artifact.map(ContentHash::new).transpose()?;
            let record = wiki.find_source(
                &SourceId::new(source_id)?,
                &ContentHash::new(revision)?,
                artifact.as_ref(),
            )?;
            let report = wiki.source_neighbors(&record, &node)?;
            let text = serde_json::to_string_pretty(&report)?;
            Ok(("source-neighbors", serde_json::to_value(report)?, text))
        }
        Command::Reindex { rebuild, wiki } => {
            let report = Wiki::open(wiki)?.reindex(rebuild)?;
            let text = format!(
                "{} sections; {} updated entities; {} removed; {}",
                report.indexed_sections,
                report.updated_entities,
                report.removed_entities,
                report.generation
            );
            Ok(("reindex", serde_json::to_value(report)?, text))
        }
        Command::Search {
            query,
            collection,
            limit,
            wiki,
        } => {
            let collection = match collection.as_str() {
                "wiki" => SearchCollection::Wiki,
                "sources" => SearchCollection::Sources,
                _ => SearchCollection::All,
            };
            let report = Wiki::open(wiki)?.search(&query, collection, limit)?;
            let text = report
                .hits
                .iter()
                .map(|hit| format!("{} — {}\n{}", hit.title, hit.heading, hit.excerpt))
                .collect::<Vec<_>>()
                .join("\n\n");
            Ok(("search", serde_json::to_value(report)?, text))
        }
        Command::Stage { file, wiki } => {
            let draft = Wiki::open(wiki)?.stage_file(file)?;
            let text = format!(
                "Staged {}: {} ({} pages)",
                draft.id,
                draft.proposal.summary,
                draft.proposal.pages.len()
            );
            Ok(("stage", serde_json::to_value(draft)?, text))
        }
        Command::Drafts { wiki } => {
            let drafts = Wiki::open(wiki)?.drafts()?;
            let summaries=drafts.iter().map(|draft|json!({"id":draft.id,"summary":draft.proposal.summary,"pages":draft.proposal.pages.len()})).collect::<Vec<_>>();
            let text = serde_json::to_string_pretty(&summaries)?;
            Ok(("drafts", json!({"drafts":summaries}), text))
        }
        Command::Draft { id, wiki } => {
            let draft = Wiki::open(wiki)?.draft(&ContentHash::new(id)?)?;
            let text = serde_json::to_string_pretty(&draft)?;
            Ok(("draft", serde_json::to_value(draft)?, text))
        }
        Command::Diff { id, wiki } => {
            let diff = Wiki::open(wiki)?.draft_diff(&ContentHash::new(id)?)?;
            Ok(("diff", json!({"diff":diff}), diff))
        }
        Command::Apply { id, wiki } => {
            let report = Wiki::open(wiki)?.apply(&ContentHash::new(id)?)?;
            let text = serde_json::to_string_pretty(&report)?;
            Ok(("apply", serde_json::to_value(report)?, text))
        }
        Command::Operations { wiki } => {
            let operations = Wiki::open(wiki)?.operations()?;
            let text = serde_json::to_string_pretty(&operations)?;
            Ok(("operations", json!({"operations":operations}), text))
        }
        Command::Operation { id, wiki } => {
            let operation = Wiki::open(wiki)?.operation(&ContentHash::new(id)?)?;
            let text = serde_json::to_string_pretty(&operation)?;
            Ok(("operation", operation, text))
        }
        Command::Recover { id, strategy, wiki } => {
            let mode = if strategy == "finish" {
                RecoveryMode::Finish
            } else {
                RecoveryMode::Rollback
            };
            let report = Wiki::open(wiki)?.recover(&ContentHash::new(id)?, mode)?;
            let text = serde_json::to_string_pretty(&report)?;
            Ok(("recover", serde_json::to_value(report)?, text))
        }
        Command::Snapshot { wiki } => {
            let snapshot = Wiki::open(wiki)?.freeze_snapshot()?;
            let text = format!(
                "Snapshot {}: {} pages, {} sources",
                snapshot.id,
                snapshot.pages.len(),
                snapshot.sources.len()
            );
            Ok(("snapshot", serde_json::to_value(snapshot)?, text))
        }
        Command::SnapshotRead {
            snapshot,
            page_id,
            offset,
            limit,
            wiki,
        } => {
            let passage = Wiki::open(wiki)?.read_snapshot_page(
                &ContentHash::new(snapshot)?,
                &PageId::new(page_id)?,
                offset,
                limit,
            )?;
            let text = passage.content.clone();
            Ok(("snapshot-read", serde_json::to_value(passage)?, text))
        }
        Command::Backup { destination, wiki } => {
            let manifest = Wiki::open(wiki)?.backup(destination)?;
            let text = format!("Backup {}: {} files", manifest.id, manifest.files.len());
            Ok(("backup", serde_json::to_value(manifest)?, text))
        }
        Command::Restore {
            backup,
            destination,
        } => {
            let wiki = Wiki::restore(backup, destination)?;
            let text = format!("Restored {}", wiki.root().display());
            Ok(("restore", json!({"root":wiki.root()}), text))
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok((command, result, text)) => {
            if cli.json {
                println!(
                    "{}",
                    json!({"api_version": API_VERSION, "command": command, "result": result})
                );
            } else {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            if cli.json {
                eprintln!(
                    "{}",
                    json!({"api_version": API_VERSION, "error": {"code": "operation_failed", "message": error.to_string()}})
                );
            } else {
                eprintln!("error: {error}");
            }
            ExitCode::FAILURE
        }
    }
}
