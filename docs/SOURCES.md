# Immutable local document sources (M1)

Implemented CLI: `ingest`, `sources`, `source`, `catalog-rebuild`. These are local
source operations, not LLM generation or publication. Ingest results include
parser diagnostics in JSON; human CLI output prints them below the source identity. Existing config v1 Wikis
need no migration: source directories and the derived SQLite catalog are created
on the first import/rebuild, not by read-only `doctor` or `sources`.

## On-disk contract

```text
raw/
  blobs/<raw-sha256-hex>                       # exact original bytes
  artifacts/<artifact-sha256-hex>.json         # ParsedDocument v1
  parses/<raw-hex>-<parser-id-sha256-hex>.json  # reusable output or checksum-bound pointer
  records/<origin-hex>-<raw-hex>-<parser-hex>.json
.ohmu/
  sources.lock                               # advisory cross-process writer lock
  catalog.sqlite                             # derived catalog v1, WAL/SHM when active
```

`SourceId` is `src-` plus SHA-256 of `file:<canonical UTF-8 path>`. The raw
revision is `sha256:<hex>` over original bytes. Identical content at different
paths shares blob/parser artifacts but retains distinct source records. Changed
bytes at one path retain the same source ID and create a new revision. Reimporting
the same origin/revision/parser reuses its first committed record and timestamp;
this is not an append-only retrieval-attempt audit log.

The parser identity includes the Ohmu parser version and pinned format-specific
parser versions. Markdown uses pulldown-cmark 0.13.5; HTML uses dom_smoothie
0.18.2, dom_query 0.28.0 and pulldown-cmark 0.13.5; PDF uses pdf-extract 0.12.1
and lopdf 0.42.0. Parsed JSON is itself SHA-256-addressed. Provenance sidecars retain the
origin, parser identity, raw/artifact hashes, raw byte count and first-import
timestamp. JSON has independent `schema_version = 1`; unknown versions fail.
New HTML/PDF artifacts include `source_revision`, binding normalized text to
its original raw digest. The field is optional on deserialization and omitted
for existing Markdown, preserving its exact artifact bytes and hashes.
HTML/PDF caches store versioned raw/parser/artifact pointers and verify both the
artifact checksum and raw-revision association before reuse. The previous
Markdown cache representation remains supported unchanged. Future parser-affecting
upgrades require explicit identities and compatibility decisions, not silent
reinterpretation of these records. SQLite and source manifest versions remain 1.

Objects are created as read-only files using descriptor-relative temporary writes,
file sync, no-clobber hard links and directory sync. Source writes are serialized
by a nonblocking advisory lock. Existing objects with different bytes are refused;
corruption is not silently repaired by replacing evidence. Final symlinks and
nonregular files are rejected. Crash-orphaned `.tmp-*` files are ignored, while
invalid final manifests fail catalog rebuild.

Raw bytes and parser objects are committed before the provenance manifest. That
manifest is the authoritative import commit point; the SQLite update follows.
A catalog failure after it leaves recoverable source evidence: retry import or
rebuild the catalog. Failure before it may leave unreferenced immutable artifacts,
which are not considered imported sources. No automatic garbage collection or
source deletion is implemented.

## Parsing, locators and limits

Local `.md`/`.markdown`, `.html`/`.htm` and `.pdf` files are supported, case
insensitively. `Wiki::ingest_file` selects the parser, and CLI `ingest` uses it.
The existing `Wiki::ingest_markdown` remains a strict Markdown-only compatibility
wrapper. Reject empty input, invalid text encoding, unsupported suffixes and
symlinked final source paths. HTML requires actual markup and extractable article
text; PDF requires a `%PDF-` signature and a loadable page tree. A suffix alone
is never considered proof that HTML/PDF data is valid. Parent directories are
trusted. Raw reads are capped at 50 MiB and retained text at 16 MiB; heading
count is bounded at 20,000. Normalized Markdown preserves original UTF-8 bytes.

HTML must be UTF-8. Readability extracts a static article and its metadata; the
retained article HTML is serialized as Markdown, preserving available headings
and links. Scripts, styles, embedded frames and SVG are excluded from normalized
text. Relative links remain references; no images, stylesheets, scripts or URLs
are fetched. Byte locators address normalized Markdown, never raw HTML offsets.

PDF parsing is limited to 2,000 physical pages, checked before text extraction.
Each page has a child node with its 1-based physical number. Bookmarks are not
yet extracted; the structure is an honest page tree. Empty pages retain their
position and emit diagnostics. Entirely non-text/image-only documents are rejected
with an OCR requirement, as are encrypted documents (no password API is exposed).
PDF text can lose reading order, glyph fidelity and table layout; retained original
bytes and physical locators allow inspection. Two newline separator bytes belong
to the preceding page. A bounded text writer aborts retained output above 16 MiB.
These checks do not bound third-party decompression memory or CPU time. Parser
unwinding from malformed PDFs is converted into an engine error before commit;
the host's panic hook can still emit its own diagnostics. Process isolation is
not implemented by this local library increment.

The tree contains an introductory root segment and one node per actual Markdown
heading, with its byte range until the next heading. Parent IDs follow heading
levels. A parent node's text range covers its own section, not all descendants;
tree traversal uses explicit parent relationships. Plain text without headings
has one root node. A heading can contain Markdown formatting without polluting
the extracted title. Code-fenced `#` text is not a heading.

`source` requires a pinned source ID and raw revision. Passage locators additionally
carry the parser artifact hash and half-open UTF-8 byte range. Start offsets must
be valid character boundaries; end offsets round down to a boundary. A limit too
small for the next character fails instead of returning a non-advancing cursor.
Default CLI page size is 16 KiB, maximum 256 KiB. PDF passages are additionally
clamped to the current physical page; continue at `next_offset` to read the next
page. Markdown and HTML passages have no PDF page number. At end-of-artifact, an
empty read has no page number and no continuation. The engine also exposes the full ParsedDocument for tree
navigation by embedding applications.

Reads verify committed provenance and raw/artifact hashes before returning text.
Deleting or editing the original input does not change an imported revision.
Changing Wiki Markdown still affects the existing page reader independently;
importing a source never writes into `pages/`.

## Catalog and recovery

SQLite uses bundled rusqlite, `user_version = 1`, WAL, FULL synchronization,
foreign-key enforcement and a two-second busy timeout. The sources table stores
one row per origin/revision/parser. Catalog writes occur under the source writer
lock; unsupported catalog versions fail without schema downgrade.

`sources` returns catalog rows in deterministic chronological/identity order. A
missing catalog returns an empty list; it does not hide a malformed or unsupported
existing database. `catalog-rebuild` validates sidecars and all referenced raw and
parser objects before replacing rows in one transaction. Deleting only this
derived database and rebuilding recovers source provenance without the original
inputs or model calls. Missing/corrupt evidence causes an explicit error and does
not replace existing catalog rows. Source listing alone is metadata, not proof
that all its stored objects are intact; pinned content reads perform that check.

Do not delete arbitrary `.ohmu` files to rebuild. Future draft/recovery state is
not disposable. SQLite's companion-file paths require a trusted internal directory;
these checks are not an OS sandbox against hostile concurrent directory swaps.

## Remaining M1 work

URL fetching, page frontmatter/alias/link reconciliation,
lexical indexing, incremental search, query tools and page-read pagination remain
in [PLAN](PLAN.md). This increment does not claim the full M1 exit criteria.
