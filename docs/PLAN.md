# Implementation plan

## Goal, scope and delivery

Build a local-first, single-user, vectorless LLM wiki for roughly 10,000 mixed
Chinese/English sources. Ingest Markdown, static web articles and text-based
PDFs; compile persistent linked knowledge; answer with source evidence. Markdown
remains editable content authority. All model-authored content is staged for
explicit review and apply.

The agreed initial delivery is **S0: complete design plus a runnable scaffold**.
M1–M4 below describe the full product and must not be presented as complete at
S0. The initial scaffold was local-only. The user subsequently created
[the Ohmu remote](https://github.com/Ceskoslov/Ohmu) and authorized staged
implementation and synchronization to `origin/main`.
README is bilingual with language switch links; other documents are English.

Out of v1: embeddings of any kind, vector databases/search, model training, OCR,
Office, JavaScript rendering, crawling, web UI, multi-user authorization,
distributed workers, MCP server and automatic publication. These omissions do
not prevent future provider-neutral interfaces.

## Architecture and selected stack

See [ARCHITECTURE](ARCHITECTURE.md) for data and recovery semantics,
[ADR](ADR.md) for tradeoffs, and [NAUSICAA](NAUSICAA.md) for integration.

| Boundary | Implementation | Introduction |
| --- | --- | --- |
| Domain | `ohmu-core`: validated IDs, snapshots, versioned config; later sources/locators/changesets | S0 onward |
| Engine | `ohmu-engine`: bounded filesystem reads; later parsing, storage, retrieval and publication | S0 onward |
| Harness adapter | `ohmu-nausicaa`: Nausicaa Tool implementations; no second agent loop | S0 onward |
| Application | `ohmu-cli`: clap/TOML/JSON, later trusted runtime composition | S0 onward |
| Content | Markdown + SHA-256-addressed source artifacts | S0 / M1 |
| Metadata/workflows | bundled SQLite via rusqlite; migrations and backups | M1 / M2 |
| Retrieval | Tantivy BM25 + jieba-rs tokenizer + explicit tree/link navigation | M1 / M3 |
| Parsing | pulldown-cmark; reqwest + dom_smoothie; pdf-extract | M1 |
| Model execution | Nausicaa ModelAdapter and optional existing native reqwest provider on Tokio | M3 |

Use Rust 2024, pinned Rust 1.98.0 and MIT OR Apache-2.0. Commit Cargo.lock.
Declare heavyweight dependencies only when implementing their milestone. New
crate versions must pass locked compilation, license review and stage fixtures;
an online latest-version label is not compatibility evidence.

## S0 — design and integration scaffold

**Status: implemented; validation evidence in [VALIDATION](VALIDATION.md).**

Deliver portable manifests, four crates, contribution rules, architecture,
decisions, this roadmap, reproducible validation and CI. Implement:

- `ohmu init <new-directory> [--name <name>]`: create versioned config, page/raw/
  internal directories and an editable welcome page, refusing existing targets.
- `ohmu doctor [--wiki <directory>]`: read-only config/layout checks; no credentials.
- `ohmu read <page-id> [--wiki <directory>]`: bounded UTF-8 content and SHA-256 revision.
- Global `--json`: v1 envelopes; runtime failures use stderr and exit 1; clap
  usage/unknown commands use its normal exit 2. Do not register planned commands.
- `ohmu_read` tool with an exact root/ID/revision canonical action, cancellation,
  explicit policy grant and persisted receipts through the host runtime.
- A credential-free scripted example using JsonlEventStore, and a temporary local
  consumer check that unifies harness dependency sources without editing either repo.

Exit: CLI smoke and filesystem/adapter tests pass; receipt visible on disk before
next model call; hidden fabricated calls denied; changed page/scope refused;
fmt, strict Clippy, tests and rustdoc pass; local links resolve; pinned Git and
local-path consumer both compile with one harness core. Create the initial local
commit only after checks, without a remote or a Nausicaa source change.

## M1 — sources, deterministic structure and lexical retrieval

**Status: in progress. Depends on S0.**

- Completed: validated source IDs/revisions/locators, deterministic Markdown
  heading trees, immutable raw/parser artifacts and provenance sidecars.
- Completed: SQLite v1 source catalog, duplicate parse reuse, source listing,
  UTF-8 source pagination and catalog rebuild. See [SOURCES](SOURCES.md).
- Completed: local HTML article extraction/heading trees and PDF physical-page
  text extraction, normalized artifact provenance and parser diagnostics.
- Remaining: URL fetching, page reconciliation/relationships, Tantivy/Jieba
  indexing, query tools and the complete M1 exit matrix.

1. Add versioned SourceId/SourceRevision, SourceLocator and DocumentNode domain
   types. Track source bytes, parser version, text artifact hash and physical pages.
2. Introduce SQLite catalog migrations and content-addressed raw/normalized
   artifact storage. Importing identical bytes must reuse parse artifacts while
   retaining distinct provenance records. Changed content creates a new revision.
3. Implement Markdown/static HTML/text PDF parsers and explicit limits from the
   architecture. Return parse diagnostics for broken encodings, empty PDFs,
   unsupported/encrypted documents and unreliable extraction; never create a
   successful empty source silently. Validate redirect destinations on URL imports.
4. Implement page frontmatter reconciliation, stable IDs, aliases, Markdown
   links, orphan/dangling link reports and incoming/outgoing relation lookup.
5. Add Tantivy fields/tokenization and index-generation tracking. Separate source
   and published Wiki collections. Provide incremental updates and complete
   rebuild from authoritative files; reject stale revision hits.
6. Add `ingest`, `search`, `reindex` and source/page read pagination CLI operations;
   expose `ohmu_search`/`ohmu_neighbors` only after their engines exist.

Exit fixtures: Chinese, English, mixed identifiers, aliases, Unicode titles,
duplicate imports, changed URL/PDF revisions, multi-page documents, absent
headings, image-only PDFs, failed fetches, redirect limits and external edits.
After deleting derived catalogs/indexes, published-content search can be rebuilt
without model calls or losing raw provenance. Draft/recovery state is never
included in a destructive rebuild command.

## M2 — knowledge drafts, publication and recovery

**Status: planned. Depends on M1.**

1. Specify versioned Draft/ChangeSet/ApplyOperation formats and SQL migrations.
   Use full proposed page bytes and expected prior hashes rather than model-
   generated shell or unrestricted filesystem patches.
2. Provide validated `ohmu_stage` and draft CLI list/show/diff commands. Models may
   propose creations/updates, citations, related concepts and contradiction notes.
   The engine enforces page IDs, target scope, citations and link consistency.
3. Implement query manifest snapshots and external-edit reconciliation. Retain
   immutable page snapshots for active reads and publication recovery.
4. Implement the writer lock, durable operation journal, staging/backups, fsync
   phases and explicit `apply`/`recover`. Freeze changeset digest plus base
   manifest for approval. Repeated operation IDs reconcile rather than duplicate.
5. Provide deterministic `lint` checks and versioned backup/restore commands.
   Never equate a structural lint pass with validated semantic truth.

Exit: one source can produce a reviewable multi-page change with citations;
published pages remain unchanged until apply. Inject a failure before and after
each write/journal phase, restart, and prove recoverability or explicit conflict.
Test stale approval, concurrent writers, partial index updates, external edits,
missing/corrupt backups and unknown harness receipt reconciliation. User-edited
bytes must not be silently discarded. Rollback uses recorded bytes, not an LLM.

## M3 — Nausicaa-powered synthesis and cited answers

**Status: planned. Depends on M1 and M2.**

1. Compose the existing AgentRuntime and ModelAdapter in the application; reuse
   provider configuration and native async transport instead of implementing a
   provider client in the engine. Configure secrets in the host environment.
2. Provide distinct read and maintenance capabilities; publication requires the
   host's exact-action approval. Store imported source text as advisory evidence,
   never as AGENTS.md rules or execution policy.
3. Use bounded search, navigation, read, stage and apply tools. Create an `ask`
   workflow and a draft-generation workflow; neither automatically publishes.
4. Default budgets: top 20 candidates, at most three navigation hops and two
   query rewrites, 12 model calls and 120 seconds per query. Enforce model-call
   and deadline limits in the host; use Nausicaa cancellation. Require a configured
   provider window/output budget with a supported token counter for live models;
   do not advertise character caps as token accounting. No automatic retries in v1.
5. Validate answer citations and separate facts, synthesis, contradictions and
   evidence gaps. Return partial results on exhausted budgets. Answer-to-Wiki
   writes create a draft with provenance rather than feeding unsupported claims
   back into the corpus as independent sources.

Exit: deterministic fixtures cover evidence selection, missing evidence,
contradictions, invented node IDs, prompt injection, cancellation, budgets and
truncation. Opt-in live evaluation uses pinned model/configuration and a held-out
human-annotated question set. Report citation correctness, answer support, cost,
latency and abstention separately. Scripted fixtures establish integration only.

## M4 — scale, packaging and compatibility

**Status: planned. Depends on M3.**

Create a reproducible 10,000-source mixed-language corpus manifest with document
size distribution and a published hardware profile. Measure ingest/index/rebuild
time, peak RSS, disk usage, warm/cold lexical query P50/P95 and separate model
latencies. Initial performance gate: warm top-20 lexical search P95 ≤ 250 ms on
an 8-core, 16 GiB RAM, local SSD Linux host, with index size/data distribution
recorded. This target is unmeasured, not a current claim; a failed gate requires
a measured optimization or an explicit revised ADR before release.

Exercise backup/restore and migration from every released on-disk version. Verify
Nausicaa's pinned revision and the latest explicitly supported revision in a
consumer compatibility matrix; upgrades are intentional PRs, not floating branches.
Package installable CLI artifacts only after Linux and macOS CI execute. Windows
native support, team services and an MCP server remain separate decisions.

## Acceptance, rollout and maintenance

Each milestone ships with its fixtures and updates implemented/planned status.
Default tests require no model credentials, network service or process sandbox.
Network parser fixtures use local test servers, with explicit test-only allowance
for loopback URLs. Live evaluation is opt-in and cannot gate ordinary development.

Until M2 recovery passes, publish is not exposed as a working command. Introduce
format migrations explicitly, retain backups, and fail closed on unknown versions.
Record operation IDs, content revisions, parser/tokenizer versions, durations and
model usage in local diagnostics; do not log credentials. Keep source content in
user-owned storage, with no telemetry service by default.
