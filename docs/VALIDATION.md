# Validation evidence

## Initial scaffold

Validated on Linux/WSL with Rust/Cargo 1.98.0. Nausicaa baseline:
`a2ab5b365cfb29bedd48d29b69951510ae5d1ef5`.
The Nausicaa working tree remained unchanged. No remote repository or push was
performed. Build artifacts and temporary smoke-test Wiki/journal data are not
part of the source commit.

| Check | Local result |
| --- | --- |
| `cargo check --workspace --all-targets` | Passed |
| `cargo fmt --all -- --check` | Passed |
| Strict Clippy, all workspace targets/features | Passed |
| Workspace tests and doc-tests | Passed: 13 integration tests; no live credentials |
| Rustdoc with warnings denied | Passed |
| CLI init/doctor/read, including JSON | Passed against a temporary Wiki |
| Scripted example with JsonlEventStore | Passed; receipt recorded before the second model request |
| Pinned Git dependency metadata | Exactly one harness core at the pinned commit |
| Temporary local host consumer | Compiled; exactly one core from the local Nausicaa path |
| Documentation links and README switches | Checked by `scripts/check_docs.py` |

The first offline metadata probe required additional non-host platform packages
not downloaded by the earlier Linux build. `cargo fetch --locked` populated them;
both offline dependency-graph checks then passed. This was a dependency-cache
prerequisite, not a failed Rust assertion. A fresh checkout should fetch first.

## Reproduce

From the repository root:

```sh
cargo fetch --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
cargo test --workspace --all-features --locked --offline
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --offline
python3 scripts/check_docs.py
python3 scripts/check_local_host.py --nausicaa ../Nausicaa
git diff --check
```

The local-host script expects a Nausicaa checkout; adjust its argument if it is
not a sibling. It does not edit either source tree or the Ohmu lockfile.

For an interactive smoke run, use fresh paths:

```sh
cargo run -p ohmu-cli --locked --offline -- init /tmp/ohmu-validation-wiki
cargo run -p ohmu-cli --locked --offline -- doctor --wiki /tmp/ohmu-validation-wiki
cargo run -p ohmu-cli --locked --offline -- read welcome --wiki /tmp/ohmu-validation-wiki --json
cargo run -p ohmu-nausicaa --example read_wiki --locked --offline -- /tmp/ohmu-validation-wiki /tmp/ohmu-validation-events.jsonl
```

Inspect the journal to see `tool_execution_started`, `tool_receipt_recorded`, the
second `model_request_started` and `turn_completed` in that order. The example
requires a new journal and refuses to reuse an existing file.

## What the tests establish

- CLI: initialization, read-only diagnostic/read commands, versioned JSON,
  runtime failure output, refusal to overwrite and absence of planned commands.
- Engine: content hashes reflect editor changes; stale revisions conflict;
  deserialization cannot bypass ID validation; symlinked pages/layout/config,
  missing/nonregular/oversized/non-UTF-8 files and unsupported config are refused.
  Directory replacement does not redirect an already-open reader.
- Adapter: exact prepared revisions/scope, invalid arguments and cancellation;
  policy-projected schemas; fabricated hidden calls denied without execution;
  receipt persistence before the next model call and durable replay.
- Compatibility: the pinned Git graph has one harness core; a local consumer can
  pass its core ToolRegistry to the adapter after applying a root-level patch.

## Evidence limits

macOS is configured in CI but was not run locally. Hosted CI results are not asserted by these local checks; the repository has
since been synchronized to its user-created GitHub remote. Native Windows is unsupported; Windows users
should use WSL. There is no live-model quality result, PDF/parser benchmark,
10,000-source performance result, multi-page transaction or recovery guarantee
at S0. Those require the fixtures and stage exits in [PLAN](PLAN.md).

The local-link checker does not crawl external sites or validate Markdown anchor
fragments. File checks do not constitute an OS sandbox or perfect exclusion of
uncooperative editor races. The current whole-page reader has a 256 KiB bound;
source pagination and semantic provenance are future work.

## M1 increment: Markdown sources and catalog

The remote `git@github.com:Ceskoslov/Ohmu.git` was added and the initial scaffold
synchronized to `origin/main` at the user's request. README.md is now English,
with a Simplified Chinese switch to README.zh-CN.md; other documents remain English.

The first M1 increment adds validated source/hash identifiers, immutable Markdown
bytes and parser artifacts, source provenance sidecars, a rebuildable SQLite v1
catalog, heading trees and bounded UTF-8 passage reads. Eight additional tests
cover the new engine/CLI flow: **21 integration tests pass in total**.

Validated cases include duplicate import reuse, distinct origins with shared
content, changed content at one origin, retained old revisions after input
deletion, catalog deletion/rebuild, exact heading parents, byte-locator
reconstruction, malformed/empty input, final source/storage symlinks, corrupt
artifacts, unknown catalog versions and nonblocking writer contention. The
original policy/receipt tests continue to pass.

Formatting, strict Clippy, workspace tests, rustdoc with warnings denied, local
document links and the local-host integration check all passed for this
increment. This is partial M1; HTML/PDF, lexical search, page reconciliation and
additional tools remain planned. See [SOURCES](SOURCES.md) for the implemented
contract and [PLAN](PLAN.md) for the remaining exit criteria.

## M1 increment: local HTML and text PDF parsing

Added `Wiki::ingest_file` and CLI dispatch for local Markdown, HTML and PDF.
`ingest_markdown` remains a compatible Markdown-only API. HTML extraction retains
the original raw bytes and produces heading-aware normalized Markdown. PDF
extraction creates physical-page nodes and clamps passages at page boundaries.
Ingest reports now expose parser diagnostics in human and JSON output.

The seven format integration tests and one new CLI test bring the suite to
**29 passing integration tests**. New fixtures exercise:

+- HTML titles/headings, script/navigation exclusion, changed revisions and
  deduplication across distinct file origins.
+- Multi-page PDF text, blank-page numbering, actual image-only PDF rejection,
  encrypted PDFs (including empty user passwords) and the 2,000-page cap.
+- Malformed media, unsupported file suffixes, invalid UTF-8 and script-only HTML.
+- Checksummed normalized-cache pointers bound to their raw source revision,
  mixed-format catalog rebuild and preserved old Markdown JSON bytes/hashes.
+- CLI HTML ingestion, visible diagnostics and normalized source passage reads.

Raw/source/catalog format versions stay at 1. `ParsedDocument::source_revision`
is a new optional serde field used for normalized formats; legacy Markdown
omits it. Rust callers constructing ParsedDocument literals must supply `None`
for legacy Markdown or the raw hash for normalized formats. No Nausicaa trait
or durable runtime-event format changes.

Passed locally: formatting, strict workspace Clippy, all tests, rustdoc with
warnings denied, documentation links, pinned dependency metadata and the local
Nausicaa consumer check. An actual CLI smoke run imported HTML and a two-page
PDF, continued passages across physical pages, rebuilt the mixed catalog and
ran the durable Nausicaa example successfully. The pinned parser dependencies' license metadata and
the HTML stack's MPL-2.0 components are recorded in ADR-005. These checks do not
establish arbitrary-PDF layout accuracy, process isolation or hosted macOS CI
success. URL fetching, OCR and full-text search remain unimplemented.

## M1 completion

Added public URL ingestion, live page reconciliation/links, Jieba + Tantivy BM25,
incremental indexing, page/source tree navigation and explicit harness query
capabilities. Forty-five tests cover the completed M1 behaviors, including local
HTTP redirect fixtures, reserved/private address rejection, failed/oversized
responses, Unicode aliases, external edits, added/removed entities, missing
indexes, mismatching commit manifests, source integrity, derived-cache rebuild,
v1→v2 catalog migration, paginated reads and denied fabricated query calls.
The source-query → evidence-read harness flow closes durable receipts.

Content/artifact/config versions remain 1. Catalog writes migrate v1 to v2
additively; source reads still accept v1. SourceRecord adds optional resolved_url
(default None), and new page/search/report types are additive. Rust struct-literal
callers must supply the new optional source field. No Nausicaa durable event or
trait format changes. New query tools require explicit grants.

Passed: the locked workspace suite (45 tests), strict Clippy, formatting,
rustdoc, documentation/README checks and local-host type compatibility. M4
performance and live model quality are not inferred from these fixtures.

## M2 publication and recovery

The 58-test workspace suite covers pure preparation, stage idempotency, exact
apply approval, all implemented journal/write interruption phases with reopen,
finish/rollback, retained divergent editor content, missing/corrupt objects,
immutable snapshots, shared reader/exclusive writer contention, state-version
rejection and backup/restore with durable SQL state. CLI and harness flows
exercise review, explicit publication and reconciliation. Full validation uses
the reproduction commands above; simulated failures do not establish physical
power-loss durability or exclusion against uncooperative editors. No live model
requests or Nausicaa modifications are needed.

## M3 workflow integration

Deterministic workflow tests cover source-search receipts, cited answers, staged
generation without publication, malformed quotes, truncation, evidence gaps,
model call/deadline limits, cancellation, rewrite limits, invented nodes and
snapshot isolation after live edits/imports. Scripted responses establish host
integration and structural citation checks, not live answer support. The native
provider is pinned to the same Nausicaa revision. No live credentials were read
and no paid/live model evaluation was performed.

M3 local result: 64 workspace tests pass, including a loopback native-provider
CLI fixture with required counter, durable usage/acceptance, injected apply denial
and cited contradiction handling. Formatting, strict Clippy, rustdoc, docs and
local-host compatibility passed. M2 hosted CI also passed on Ubuntu 24.04 and
macOS 15: [run 40873189271](https://github.com/Ceskoslov/Ohmu/actions/runs/40873189271).

## M4 scale, compatibility and packaging

The 10,000-source corpus and all aggregate measurements are recorded in
[SCALE](SCALE.md) and [results](results/README.md). Warm strict live P95 improved
from 328.24 ms to 220.93 ms without dropping each-query content generation/hash
validation. Frozen-query P95 was 9.94 ms, with setup measured separately. The
host was 6-core/12-thread WSL, not the exact 8-core reference machine. Physical
storage topology, storage-cold queries and live model quality remain unmeasured.

All four archived historical producers passed current read/reindex/search; M2
also passed backup/draft/snapshot round trips. The local host probe passed with
both core and native provider unified, and Nausicaa remained clean at its pinned
revision (also the explicitly checked remote HEAD on 2026-10-29). A local native
archive included 367 locked dependency notices and passed extraction/version/
init/read smoke checks; development provenance correctly recorded dirty=true.
CI packages clean native builds only after both Ubuntu/macOS verification jobs.

The final local suite contains 67 passing tests, including same-size/restored-
mtime freshness races, in-flight cancellation of a legacy adapter and refusal to reuse an acceptance destination before
model calls or staging. Formatting, strict Clippy, rustdoc, documentation links
and extended local-host compatibility passed. M3 hosted validation passed both
platforms: [run 40874147443](https://github.com/Ceskoslov/Ohmu/actions/runs/40874147443).
Current hosted packaging results are published in the
[CI workflow](https://github.com/Ceskoslov/Ohmu/actions/workflows/ci.yml); do not
interpret the workflow configuration alone as a completed run.
