# Packaging and compatibility

Ohmu targets Unix: Linux/WSL and macOS. Native Windows is unsupported. Rust 1.98.0
and the committed Cargo.lock are the build baseline. There is no crates.io or
GitHub Release publication implied by a successful source commit.

## Artifact gate

The `package` CI job depends on both Linux and macOS `verify` jobs. It then builds
a native release executable on each runner, packages it with `scripts/package.py`,
extracts the archive and runs version/init/read smoke checks. Artifact names
include the actual rustc host triple, not an assumed macOS CPU architecture.
Actions artifacts include tar.gz plus SHA-256 files. Creating public release tags
or uploading a GitHub Release is a separate action.

```sh
cargo build -p ohmu-cli --release --locked --offline
python3 scripts/package.py --binary target/release/ohmu
```

The script refuses an existing archive. Extract into a user-owned directory and
place `bin/ohmu` on PATH. Linux GNU artifacts use their runner's glibc baseline;
no static/musl or arbitrary-old-distribution promise is made. Build from source
on unsupported libc combinations. Model credentials and Wiki data are never
packaged. BUILD.json records revision, toolchain, target and dirty status; release
artifacts should have dirty=false. Source files and reproducible lockfile are
included alongside the binary.

THIRD-PARTY.json inventories every locked dependency, conservatively including
development and non-host platform packages. License texts and notices are copied
from their downloaded package roots or Git workspace ancestors. Packages that
omit standalone license files retain their complete published source, README,
manifest and file notices under sources/; no copyright text is invented. The
inventory identifies every fallback and original source URL. Dependencies retain their original licenses;
Ohmu's MIT OR Apache-2.0 grant does not relicense them. HTML-related MPL-2.0
components are unmodified; their source archive URLs and exact versions are
included. Cargo.lock retains registry checksums and the harness Git revision.
This is a provenance/notice bundle, not legal advice or an automated compliance
certification.

## Supported harness matrix

| Role | Revision | Evidence |
| --- | --- | --- |
| Pinned Git core and provider | a2ab5b365cfb29bedd48d29b69951510ae5d1ef5 | Workspace tests, workflow/native-provider fixture |
| Local consumer core and provider | Same revision | `check_local_host.py --with-provider`; single local core/provider |
| Latest explicitly checked remote HEAD (2026-10-29) | Same revision | `git ls-remote` matched; no untested upgrade or floating branch |

The probe uses an independent temporary consuming workspace. Git-source packages
are patched to the selected local Nausicaa checkout at the consumer root. It does
not edit either repository or Ohmu's lockfile. Run without `--with-provider` for
the minimal read adapter; use it for model composition. Root-level patches must
cover optional provider packages too when a host uses both dependency sources.

## On-disk compatibility

| Format | Supported versions | Behavior |
| --- | --- | --- |
| Config / API | 1 | Unknown config fails without rewrite |
| Source manifests / parser artifacts | 1 | Old Markdown optional-field omission retained; immutable checksums verified |
| Derived source/page catalog | 1 → 2, 2 | Additive migration; rebuild from authority |
| Draft / operation / state database | 1 | Preserve SQL, never delete through reindex |
| Snapshot / backup manifest | 1 | Hash-check and refuse unknown versions/unsafe paths |

No released version is silently reinterpreted. Tests cover the catalog migration,
legacy Markdown bytes, future-version rejection and SQL-inclusive backup recovery.
The historical producer probe additionally builds isolated Git archives of S0,
Markdown M1, complete M1 and M2, creates real data with their CLI, and reads,
reindexes and (for M2) restores it with the current binary:

```sh
cargo build -p ohmu-cli --locked --offline
python3 scripts/check_legacy.py
```

This requires repository history and cached historical dependencies. Each probe
uses a separate Wiki and build target; original repositories remain unchanged.
It complements the fixture suite rather than replacing its failure injection.

## Evaluation before deployment

See [SCALE](SCALE.md) for measured lexical behavior. Live quality is separate:
`scripts/evaluate.py --run-live` accepts a user-provided held-out question set and
pinned provider configuration. It sends only questions, never human annotations,
and retains journals, acceptance, usage, optional configured-price cost estimates,
source recall and latency. Human answer-support and contradiction judgments remain
explicitly unscored until independent review. No live benchmark is run by default.
