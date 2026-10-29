# Ohmu

**English** | [简体中文](README.zh-CN.md)

A local-first LLM wiki in Rust: continuously compile sources into readable, editable Markdown knowledge with traceable evidence. No embeddings, vector database, or vector similarity search.

Ohmu integrates with [Nausicaa](https://github.com/Ceskoslov/Nausicaa) across separate repositories. Ohmu owns knowledge; Nausicaa owns the agent loop, model protocol, authority, cancellation, and durable tool receipts.

## Current status

This is a **complete design and runnable scaffold**, not the fully implemented wiki product.

| Implemented | Planned, not implemented |
| --- | --- |
| Wiki initialization, configuration and layout diagnostics | Markdown, static web and text PDF ingestion |
| Bounded Markdown reads with SHA-256 revisions | Chinese tokenization, BM25 search, tree and link navigation |
| Nausicaa `ohmu_read` tool | LLM drafts, multi-page review/publication and recovery |
| Credential-free integration example and durable receipt checks | Source-cited question answering and scale evaluation |

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
