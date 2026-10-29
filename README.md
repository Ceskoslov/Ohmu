# Ohmu

**English** | [简体中文](README.zh-CN.md)

A local-first LLM wiki in Rust: continuously compile sources into readable, editable Markdown knowledge with traceable evidence. No embeddings, vector database, or vector similarity search.

Ohmu integrates with [Nausicaa](https://github.com/Ceskoslov/Nausicaa) across separate repositories. Ohmu owns knowledge; Nausicaa owns the agent loop, model protocol, authority, cancellation, and durable tool receipts.

## Current status

**M1–M3 are implemented: vectorless retrieval, reviewed publication/recovery and snapshot-pinned cited model workflows.** M4 is under measured validation.

| Implemented | Later stages |
| --- | --- |
| Local Markdown/HTML/PDF and public HTTP(S) imports | Live model quality evaluation |
| Immutable sources, page/byte evidence and catalog rebuild | Scale evaluation |
| Page frontmatter, aliases, backlinks and health checks | Scale evaluation |
| Jieba + Tantivy BM25, incremental indexing and stale checks | Packaging and compatibility matrix |
| Nausicaa page, search, neighbor and source-read tools | OCR and multi-user services are outside v1 |

## Quick start

Requires Rust 1.98.0. Linux/WSL is supported locally; CI also configures macOS. The first build needs network access for dependencies and the pinned Nausicaa commit.

```sh
cargo run -p ohmu-cli -- init /tmp/ohmu-demo --name "研究 Wiki"
cargo run -p ohmu-cli -- doctor --wiki /tmp/ohmu-demo
cargo run -p ohmu-cli -- read welcome --wiki /tmp/ohmu-demo --json
```

The initialization target must not exist; its parent must exist. Use fresh paths when repeating the demo. Edit `pages/welcome.md` directly, then read it again to obtain a new content hash. Whole-page reads are capped at 256 KiB and refuse symlinks and non-UTF-8 files.

Read through Nausicaa without an API key or a shell tool:

```sh
cargo run -p ohmu-nausicaa --example read_wiki -- /tmp/ohmu-demo /tmp/ohmu-demo-events.jsonl
```

The event file must not already exist. The example uses a scripted model and verifies that a tool result is persisted before reaching the model. It does not establish live-model answer quality.

Import local Markdown, HTML or text PDFs as original evidence without creating or publishing a Wiki page:

```sh
cargo run -p ohmu-cli -- ingest ./README.md --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- sources --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- catalog-rebuild --wiki /tmp/ohmu-demo
```

`ingest` returns the source ID, raw revision and parser artifact hash. Use `source <id> --revision <hash> --wiki <directory>` to read a pinned revision, with `--offset` and `--limit` byte pagination. `catalog-rebuild` restores SQLite metadata from provenance manifests. PDF passages retain physical page numbers and never cross a page boundary; HTML locators address the extracted Markdown. Scanned documents require OCR. Explicit `ingest-url` imports public HTTP(S) resources; private, local and reserved destinations are refused.

Build or incrementally update the index, then search the published Wiki; use `--collection sources` for original evidence:

```sh
cargo run -p ohmu-cli -- reindex --wiki /tmp/ohmu-demo
cargo run -p ohmu-cli -- search Ohmu --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- search Rust --collection sources --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- lint --wiki /tmp/ohmu-demo --json
```

External edits and newly imported sources invalidate the previous index; run `reindex` again to update it. `read --offset 0 --limit 16384` reads large pages in bounded slices. `lint` and `neighbors` always inspect current Markdown. Model generation is available; publication is explicitly reviewed.

## Design and integration

- [Implementation plan and acceptance criteria](docs/PLAN.md)
- [Architecture and current boundaries](docs/ARCHITECTURE.md)
- [Technology decisions](docs/ADR.md)
- [Nausicaa integration and local dependency overrides](docs/NAUSICAA.md)
- [Validation evidence and reproduction](docs/VALIDATION.md)

All documentation except the README defaults to English. The two README versions share status and commands; use the links at the top to switch languages.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
cargo test --workspace --all-features --locked --offline
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --offline
python3 scripts/check_docs.py
python3 scripts/check_local_host.py --nausicaa ../Nausicaa
```

Run `cargo fetch --locked` first on a fresh checkout. The local override check creates a temporary consumer and changes neither Nausicaa nor this repository's lockfile.

Licensed under either MIT or Apache-2.0, at your option.

Review publication with `stage proposal.json`, `drafts`, `draft <digest>` and
`diff <digest>`, then explicitly `apply <digest>`. Use `operations` and
`recover <operation> --strategy finish|rollback` after interruption. `snapshot`,
`snapshot-read`, `backup <new-directory>` and `restore <backup> <new-directory>`
retain knowledge and durable workflow state. See [publication](docs/PUBLICATION.md)
for the proposal schema, evidence, recovery and filesystem limits.

`ask` returns cited facts, synthesis, contradictions and evidence gaps; `compile`
generates a draft. Both use a frozen query snapshot and never automatically
publish. Live requests require explicit provider/model, context/output budgets,
a trusted model-specific token counter and a fresh event journal. See
[workflows](docs/WORKFLOWS.md) for configuration and limits. Scripted fixtures do
not establish live-model quality.
