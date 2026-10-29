# Drafts, publication and recovery

M2 is implemented. Markdown remains authoritative. `stage proposal.json` validates
full replacement bytes, expected revisions, IDs, links and evidence, and stores
an immutable draft in `.ohmu/state.sqlite`. It never changes published pages.

```json
{"summary":"Introduce a topic","pages":[{"page_id":"topic","expected_revision":null,"content":"# Topic\n\nA reviewed explanation.\n","citations":[]}]}
```

An update supplies the exact previous `sha256:...` revision. Evidence entries have
`locator` (source_id, revision, artifact, start, end, page) and `quote` fields.
Quotes must exactly match immutable normalized source bytes. The engine appends
portable `ohmu-source://` evidence links and quotes before calculating the draft
digest. This is structural validation, not proof that a claim follows from a quote.

Review with `drafts`, `draft <digest>` and `diff <digest>`. Publish explicitly with
`apply <digest>`. The harness `ohmu_stage` and `ohmu_apply` tools are separately
registered; apply defaults to Ask. Preparation freezes the entire draft, operation
ID and base page manifest; it performs no publication. Scope includes root/pages
filesystem identities. Replaced directories and stale approvals fail.

## Durable state and interruption

Config, sources, drafts, snapshots and backup manifests use format 1. Durable
`.ohmu/state.sqlite` uses schema 1 and must be backed up. Derived catalog schema 2
and Tantivy indexes remain rebuildable. Unknown durable versions fail closed.

Before the first page replacement, publication retains before/after content
objects, a complete snapshot and a durable journal. States are prepared, applying,
committed, rolled_back and conflict. Each file uses same-directory temporary
write, fsync, rename and directory fsync. SQLite uses WAL with FULL synchronous.
An uncertain result reports its operation ID; inspect `operations` or
`operation <id>`, rather than rerunning a model request.

Pending operations block live reads and new maintenance writes. `recover <id>
--strategy finish` completes recorded writes; `--strategy rollback` restores
recorded before images and removes only recorded newly created pages. Recovery
validates all required objects and target hashes before writing. Divergent editor
bytes are retained as content objects and reported as conflict. Resolve such a
conflict deliberately outside the automated recovery path; it is never silently
overwritten. Repeated committed IDs return their durable result.

Cooperating readers and writers use nonblocking shared/exclusive directory locks
and the source lock. Reads do not create lock files. Publication cannot atomically
replace multiple Markdown files for external readers. Editors do not obey these
locks; pause edits during apply. Hash checks detect ordinary changes but cannot
exclude an editor racing between a hash check and rename.

## Snapshots and backups

`snapshot` retains page bytes and source manifests under an immutable manifest
hash. `snapshot-read <snapshot> <page>` serves those bytes after live edits or an
interrupted operation. Snapshotting writes retained objects but publishes nothing.

`backup <new-directory>` includes config, pages, immutable source storage, drafts,
operations, snapshots and content objects. SQLite is copied through its online
backup API, including committed WAL data. Derived catalogs/indexes, model journals,
credentials and machine configuration are excluded. `backup.json` inventories
allowed relative paths and SHA-256 hashes. `restore <backup> <new-directory>`
validates the manifest before creating a destination, refuses overwrite and
symlinks, rechecks each copy, then rebuilds the source catalog. Run `reindex` after
restore. Failed copies retain partial state for inspection.

Tests inject interruption at every implemented journal/file phase, reopen the
Wiki and finish or roll back. They also cover stale approval, concurrent locks,
unknown edits, missing/corrupt objects and backup round trips. These are simulated
interruptions, not hardware power-loss certification.
