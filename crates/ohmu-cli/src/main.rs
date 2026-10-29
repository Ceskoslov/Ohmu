use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ohmu_core::{API_VERSION, ContentHash, InvalidDigest, InvalidPageId, PageId, SourceId};
use ohmu_engine::{EngineError, Wiki};
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
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Import a local Markdown source without publishing a Wiki page.
    Ingest {
        path: PathBuf,
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// List immutable source revisions from the derived catalog.
    Sources {
        #[arg(long, default_value = ".")]
        wiki: PathBuf,
    },
    /// Read a pinned source revision with UTF-8-safe byte pagination.
    Source {
        source_id: String,
        #[arg(long)]
        revision: String,
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
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    PageId(#[from] InvalidPageId),
    #[error(transparent)]
    Digest(#[from] InvalidDigest),
    #[error("source revision not found; use `sources` or rebuild the catalog")]
    MissingSource,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

fn run(command: Command) -> Result<(&'static str, Value, String), CliError> {
    match command {
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
        Command::Read { wiki, page_id } => {
            let snapshot = Wiki::open(wiki)?.read_page(&PageId::new(page_id)?)?;
            let text = format!(
                "{} @ {}\n\n{}",
                snapshot.page_id, snapshot.revision, snapshot.content
            );
            Ok(("read", serde_json::to_value(snapshot)?, text))
        }
        Command::Ingest { path, wiki } => {
            let report = Wiki::open(wiki)?.ingest_markdown(path)?;
            let text = format!(
                "{} @ {}\n{} nodes; reused: {}",
                report.record.source_id, report.record.revision, report.node_count, report.reused
            );
            Ok(("ingest", serde_json::to_value(report)?, text))
        }
        Command::Sources { wiki } => {
            let records = Wiki::open(wiki)?.sources()?;
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
            Ok(("sources", json!({"sources": records}), text))
        }
        Command::Source {
            source_id,
            revision,
            offset,
            limit,
            wiki,
        } => {
            let wiki = Wiki::open(wiki)?;
            let source_id = SourceId::new(source_id)?;
            let revision = ContentHash::new(revision)?;
            let record = wiki
                .sources()?
                .into_iter()
                .find(|record| record.source_id == source_id && record.revision == revision)
                .ok_or(CliError::MissingSource)?;
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
