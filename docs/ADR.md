# Architecture decision records

All decisions below are accepted for the design. Implementation stage is explicit;
acceptance of a decision does not imply its future subsystem already exists.

## ADR-001 — Vectorless retrieval (S0 constraint; M1/M3 implementation)

Use BM25 full-text search, explicit aliases, document trees and page links. A
model may rewrite a query and choose which candidate node to read. Do not compute
or store embeddings, including sparse learned embeddings or a fallback vector path.
Ordinary lexical BM25 statistics are allowed.

This retains explicit evidence and avoids an embedding pipeline, at the cost of
weaker automatic recall for paraphrases and cross-language concepts. Use curated
aliases, entity pages, limited rewrites and measured recall fixtures to address
that gap. Do not claim any method is universally superior on all corpora.

Alternatives: vector/hybrid RAG violates the user constraint; all-corpus long
context alone is unsuitable for the target scale. Tree navigation supplements
candidate retrieval rather than sending the full corpus tree to every model call.

Reference: [PageIndex's hierarchical retrieval](https://github.com/VectifyAI/PageIndex).
PageIndex is an implementation approach, not a universal interoperability standard.

## ADR-002 — Persistent compiled Wiki (M2/M3)

Compile sources into linked topic/entity/comparison pages during ingestion workflows.
Preserve raw evidence and expose contradictions instead of repeatedly deriving
every relationship from scratch. Questions can lead to new drafts, never automatic
publication. A generated page is a synthesis, not an independent original source.

Reference: [the LLM Wiki pattern](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).
The key extra cost is maintaining citations and refreshing affected pages when a
source changes. A reverse citation index identifies pages needing review; source
changes mark their evidence stale rather than silently rewriting old conclusions.

## ADR-003 — Markdown authority and retained source artifacts (S0/M1)

Keep Wiki content in ordinary editable Markdown, with stable IDs independent of
titles. Raw and normalized source revisions use content-addressed immutable files
plus versioned sidecar provenance manifests. SQLite catalogs and Tantivy indexes
are derived views. The manifests retain the information needed to rebuild source
catalogs; raw bytes alone cannot recover provenance, parser versions or URLs.

SQLite also stores durable draft/publication state, which is not disposable.
Backups include files, manifests, draft records, recovery journals and schema
versions. Rebuild only derived data. An all-SQLite content source would simplify
transactions but would turn editor changes into imports; the user chose direct
Markdown editing.

## ADR-004 — Local SQLite and Tantivy (M1/M2)

Use `rusqlite` with bundled SQLite, WAL, foreign keys, a bounded busy timeout and
versioned migrations. Keep database operations in bounded synchronous engine
methods; the async application dispatches long operations off its model I/O path.
Use one engine writer connection/operation lock per wiki. Do not introduce SQLx,
PostgreSQL, a graph service or a resident worker in v1.

Use Tantivy rather than SQLite FTS5 for explicit per-field tokenization, ranking
and a dedicated Rust index. Build a thin tokenizer against the chosen Tantivy
API using jieba-rs, avoiding a separate adapter crate tied to another Tantivy
version. Keep exact IDs untokenized and version tokenizer settings.

The tradeoff is two storage generations instead of one SQLite transaction.
Version-filter search hits and track index generations; content authority wins
when a cache disagrees. A missing/stale index is a visible operational state.

References: [rusqlite](https://docs.rs/rusqlite/latest/rusqlite/),
[SQLite WAL](https://sqlite.org/wal.html),
[Tantivy](https://github.com/quickwit-oss/tantivy),
[jieba-rs](https://docs.rs/jieba-rs/latest/jieba_rs/).
The lighter [SQLite FTS5](https://sqlite.org/fts5.html) alternative is viable for a
smaller corpus but is not the selected first implementation.

## ADR-005 — Rust parsing, no mandatory Python service (M1)

Use pulldown-cmark with heading and source-offset support; dom_smoothie plus
dom_query Markdown serialization for local static HTML; pdf-extract with lopdf
for page-separated text. URL fetching via reqwest/Rustls remains planned. Keep a parser
interface returning normalized text, nodes, locators and diagnostics so a future
OCR/layout adapter does not rewrite the engine. Content-address parser outputs
with their parser version. File extensions alone are not trusted media validation.

Local HTML/PDF ingestion is implemented with pinned parser versions. Existing
Markdown artifacts keep their byte-preserving format. Normalized HTML/PDF
artifacts additionally bind their raw revision; their parser-cache pointers
verify artifact checksums and raw-revision association. Parser-affecting dependency
upgrades require a new parser identity and compatibility fixtures.

The selected direct parser crates report MIT license metadata. The pinned HTML
stack also includes MPL-2.0 components: cssparser 0.37.0, cssparser-macros 0.7.1,
dtoa-short 0.3.5 and selectors 0.38.0. These remain upstream dependencies with
their own licenses; Ohmu's dual license describes Ohmu code, not a relicensing
of dependencies. No dependencies are vendored or modified by this increment.
Distribution notices/packaging remain an M4 deliverable.

PDF extraction can lose reading order and tables. The v1 contract is traceable
page text, not universal document understanding. Empty/scanned pages are flagged.
Do not silently call a cloud OCR service or install a Python runtime. PageIndex
Cloud and external document services would add operational and data dependencies
and are not required for this local scaffold.

References: [pulldown-cmark](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/),
[dom_smoothie](https://docs.rs/dom_smoothie/latest/dom_smoothie/),
[pdf-extract](https://docs.rs/pdf-extract/latest/pdf_extract/).

## ADR-006 — Reuse Nausicaa through a one-way adapter (S0/M3)

The core/engine are independent of the harness; ohmu-nausicaa implements Tool and
owns no nested runtime. The trusted application composes Nausicaa's runtime,
model adapter, context compiler, executor, policy and event store. Read access is
explicitly granted; registration alone grants nothing. Tool results are evidence,
not authority.

Use a fixed Git revision and provide workspace-root path overrides for local
development. Rust traits from two Cargo package sources are different types even
when crate names and versions match. The integration check must establish a
single resolved harness core, not merely that both repositories build separately.

Native library composition is selected over MCP/JSON-RPC for v1 to preserve typed
prepared actions and receipts without another protocol. A remote adapter can be
added later. Reference: [Nausicaa](https://github.com/Ceskoslov/Nausicaa).

## ADR-007 — Explicit staged publication with a journal (M2)

Freeze proposed page bytes, expected versions and an immutable changeset digest.
Approval binds to that exact operation. Use writer locking, retained snapshots,
backups and a durable operation state machine. Engine reads see completed
snapshots; multi-file filesystem replacement itself is not atomic.

External editors do not honor locks. Refuse recognized stale bases and preserve
divergent bytes; document the remaining race window rather than claiming perfect
isolation. If recovery cannot identify the state from hashes, stop in conflict.
Neither a missing receipt nor Git history justifies replaying uncertain writes.

## ADR-008 — Bounded runtime, portable source and honest evidence (S0 onward)

Pin Rust 1.98.0 for Ohmu; leave Nausicaa's own MSRV untouched. S0 supports Unix,
with Linux/WSL local validation and macOS CI configuration. Native Windows is not
supported yet. Use rustix safe descriptor APIs; forbid unsafe code in Ohmu crates.
Directory checks are not process containment.

Commit Cargo.lock and fetch it before offline checks. Do not add unused planned
dependencies. Credential-free tests prove protocol and deterministic behavior;
live quality and scale need separate evidence. Model cost/retry/budget behavior
is implemented in the host, not assumed from a harness roadmap entry.

## ADR-009 — Documentation language and delivery (S0)

README.md defaults to English; README.zh-CN.md provides Simplified Chinese with reciprocal
switch links. All planning, architecture, decisions, integration and contributor
documentation defaults to English. Keep commands and implementation status aligned
between README versions. No bilingual documentation generator or website is needed.
