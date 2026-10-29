# M1 ingestion, page relations and lexical retrieval

## Explicit URL imports

`ohmu ingest-url <http(s)-url> --wiki <directory>` fetches one resource, not a
crawler. Origin identity is the normalized requested URL without a fragment;
`resolved_url` records its final location. Different bytes retain prior revisions.
MIME types supported: text/html, application/xhtml+xml, application/pdf,
text/markdown and text/plain. HTML/Markdown must be UTF-8 (or compatible ASCII).
Unknown MIME types, encoded bodies, empty/invalid parses and HTTP errors fail.

The fetcher disables proxies, automatic retries and automatic redirects. Every
hop resolves and validates all destination addresses, then pins those addresses
while preserving the hostname for Host/TLS verification. Private/local/reserved
IPv4 and non-public/special-use IPv6 destinations fail. The IPv6 policy is
conservative global-unicast-only, excluding transition and documentation ranges.
Embedded credentials, non-HTTP(S) schemes and URLs over 8192 bytes fail. There is
no production flag to allow localhost; loopback is enabled only for unit fixtures.

At most five redirects, 50 MiB of response body and a 30-second HTTP/body deadline
are allowed. OS DNS resolution is synchronous and is not forcibly interrupted;
the remaining deadline is checked after it returns. Imports do not fetch images,
scripts or stylesheets. Source text never becomes execution authority.

## Page metadata and relations

Pages use flat `pages/<stable-id>.md` files. Optional YAML frontmatter supports
`id`, `title` and `aliases`; an omitted ID defaults to the filename, but an
explicit mismatching ID is rejected. Missing title falls back to the first
heading. Duplicate YAML keys fail. Frontmatter is bounded to 16 KiB; YAML
anchors/aliases and excessive nesting/events are disallowed. Includes are not
enabled. Other metadata remains advisory and does not grant tools.

Markdown links and `[[wikilinks|labels]]` resolve against explicit IDs first,
then case-normalized titles/aliases when exactly one page matches. Percent-encoded
targets and heading fragments are supported. Code spans/fences do not create
links. External URLs do not create page edges. There is no filename traversal or
implicit file loading through a link. Alias collisions are diagnosed, not merged.

`lint` reports orphan pages, dangling links and ambiguous references; `neighbors`
reports incoming/outgoing links. These operations read current files without
writing them. Node offsets refer to the original full page bytes, excluding
frontmatter from indexed sections. Page parsing allows 16 MiB per page with a
512 MiB aggregate in-memory reconciliation budget. These bounds are not a claim
about 10,000-document performance; that is measured in M4.

Whole-page `read` preserves the legacy 256 KiB limit. Supplying `--offset` or
`--limit` enables UTF-8-safe pagination of pages up to 16 MiB, with a hash of the
entire file and `next_offset`. Optional `--revision` rejects a changed page.
Source reads remain revision/artifact-pinned and preserve PDF physical boundaries.
`source-neighbors` navigates a pinned source node (`root` by default).

## Index and generation semantics

`reindex` restores the source catalog from provenance sidecars, reconciles current
pages, and updates only changed/added entities and removes deleted entities.
Wiki sections and source-revision nodes occupy separate searchable collections.
Sources include historical immutable revisions, never unpublished drafts.

Tantivy 0.27.0 uses a custom tokenizer with Jieba 0.11.0 search-mode segmentation,
Unicode lowercasing, original UTF-8 offsets and monotonic token positions. The
same tokenizer runs during querying. This is lexical retrieval, not learned
sparse or dense vector search. No embedding is computed. Query terms are literal,
not a query-language program. Weights are title 4, aliases 3, headings 2, body 1;
an exact page ID gets an additional boost. Default result limit is 20, maximum
100; queries are bounded to 4096 bytes and 64 unique lexical terms.

Default `search` collection is `wiki`; `--collection sources` or `all` opts into
original documents. Results include identity, revision, section byte offsets,
optional physical page, score and a UTF-8 excerpt. Returned content is revalidated
against authoritative page/source bytes and source artifact hashes.

The content generation hashes current page revisions and provenance manifests
with the tokenizer version. Search rejects an absent/mismatching generation even
when the stale index would return no hits. New imports, deletions and ordinary
editor changes therefore require `reindex`, not silent cached answers. Search
does not write or automatically index; permission to query is not permission to
rebuild. M1 checks one query's consistency, not a multi-query retained snapshot;
the latter is an M2 responsibility.

Index data lives in `.ohmu/search-index`. SQLite's index-state row binds the
generation to Tantivy's committed operation stamp. If an index commit succeeds
but catalog persistence fails, queries reject the disagreement. Reindexing
without a trustworthy manifest rebuilds the derived documents. `reindex --rebuild`
retires the old index directory and creates a fresh one; retired directories are
retained for inspection rather than garbage-collected automatically.

Catalog migration v1 → v2 adds page metadata/links and index state transactionally
without changing source rows or immutable source/artifact formats. Reads of a
v1 source catalog remain valid until the next write migrates it. Future catalog
versions fail closed. Removing only derived catalog/index data and reindexing
recovers source and page search without original inputs or model calls. Never
remove drafts/recovery state as part of an index rebuild.

## Harness integration

Existing `register_read_tools`/`read_only_policy` retain their original grants.
Opt into additional tools with `register_query_tools` and `query_policy`:

- `ohmu_search`: literal lexical search with collection and result limit.
- `ohmu_neighbors`: `kind=page` with a page ID, or `kind=source` with pinned source
  identity and node ID. Lists use offset/limit (default 50, maximum 100).
- `ohmu_read_source`: pinned evidence read, default 16 KiB, maximum 64 KiB.

`ohmu_read` additionally accepts optional offset/limit while preserving the old
whole-page result shape when omitted. Query tools normalize arguments in prepare,
freeze a result digest and Wiki scope, then recheck both during execute. A change
is a conflict, never an implicit new action. New tool outputs are capped at
128 KiB of JSON. Neighbor lists are paginated independently with totals.
Registration alone grants no authority; legacy policy does not grant new tools.

No ingest/reindex tool is implicitly enabled. Network and maintenance capabilities
remain explicit host decisions. Long synchronous engine calls belong off an async
host's I/O loop; cancellation checks surround tool work but do not interrupt every
filesystem/index operation. Existing receipt durability and exact-action rules
remain owned by Nausicaa. See [NAUSICAA](NAUSICAA.md).
