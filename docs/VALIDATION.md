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

macOS is configured in CI but was not run locally. There is no hosted CI result
for this local-only repository. Native Windows is unsupported; Windows users
should use WSL. There is no live-model quality result, PDF/parser benchmark,
10,000-source performance result, multi-page transaction or recovery guarantee
at S0. Those require the fixtures and stage exits in [PLAN](PLAN.md).

The local-link checker does not crawl external sites or validate Markdown anchor
fragments. File checks do not constitute an OS sandbox or perfect exclusion of
uncooperative editor races. The current whole-page reader has a 256 KiB bound;
source pagination and semantic provenance are future work.
