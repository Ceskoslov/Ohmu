# Reproducible scale evaluation

The synthetic corpus generator and runner are implemented; local measured results
are below. This is lexical retrieval evaluation, independent
of model quality or live model latency.

```sh
cargo build -p ohmu-engine --example scale --release --locked --offline
python3 scripts/benchmark.py target/scale-10000 --sources 10000 --iterations 100
```

The destination must not exist. `--prepared` measures an existing generated,
ingested and indexed corpus. The generator retains `corpus.json`: relative input
names, formats, language, raw byte lengths and SHA-256 hashes. Corpus identity is
independent of Wiki root and import timestamps. Inputs are deterministic; there
is no external download or model call. Generated Wiki/runtime data belongs under
ignored target/. Commit only aggregate results and the generator.

The full fixture contains 6,000 Markdown, 3,000 static HTML and 1,000 text PDFs,
plus 1,000 linked Wiki pages and the initial welcome page. Markdown/HTML are mixed
Chinese/English; PDFs use English Helvetica text and physical pages. Target body
sizes are 80% 1 KiB, 15% 4 KiB, 4% 16 KiB and 1% 64 KiB. Wrappers/PDF page counts
change actual raw/normalized lengths; report their measured distribution instead
of treating target sizes as bytes on disk. The text is repetitive synthetic
research prose, not a natural-corpus relevance or parser-accuracy benchmark.

`prepare` times sequential durable ingestion, first indexing, complete rebuild
and no-op incremental indexing. It records section count, actual raw/normalized
sizes and logical disk bytes. Retired indexes remain available, so total Wiki
bytes exceed the active index. `measure` records strict live search and pinned
snapshot search separately. Each uses top20 over both collections with the same
eight selective/broad/Chinese/English/missing-term queries. Warm percentiles use
100 samples. The runner also starts eight independent processes for process-cold
P50/P95. OS caches are not flushed; this is **not storage-cold performance**.
Snapshot freeze and query-local index build are separately timed and never hidden
inside warm lexical latency. No model latency or cost is fabricated.

The Python runner records platform, CPU, visible memory, filesystem and child peak
RSS (compiler excluded). A WSL guest cannot prove physical storage topology or
reserve host resources. Do not equate logical CPU threads with physical cores.
The original target is warm top20 P95 ≤250 ms on 8 cores / 16 GiB / local SSD.
A different machine's result is evidence with a stated scope, not certification
of that exact hardware gate. Failed targets require a measured optimization or
an explicit revised ADR before release.

## Local result, 2026-10-29

Release build, Rust 1.98.0, AMD Ryzen 5 7500F (6 physical cores, 12 threads),
15.42 GiB visible guest RAM, WSL2 Linux 6.6.87.2, glibc 2.39 and ext4 virtual disk.
Physical host storage topology was not identified. Corpus manifest:
`sha256:89caca3265e5ba34e98659f1189cd0cb3563865ec4ac69474dcd4e7370ea9d11`.
Raw and normalized sizes: minimum 1,140 B; medians 1,335 / 1,182 B;
P95 16,513 B; maxima 263,689 / 262,544 B. There are 23,002 indexed sections.

| Measurement | Result |
| --- | --- |
| Durable sequential import, 10,000 sources | 421.59 s |
| First index / complete rebuild | 2.115 / 2.086 s |
| No-op incremental index | 1.086 s |
| Active index / complete Wiki logical bytes | 10,335,118 / 137,078,414 B |
| Prepare process peak RSS | 146,120 KiB |
| Query process peak RSS | 119,296 KiB |
| Original strict live warm P50 / P95 (100 samples) | 301.80 / 328.24 ms |
| Optimized strict live warm P50 / P95 (100 samples) | 208.88 / 220.93 ms |
| Process-cold P50 / P95 (8 samples, OS cache warm) | 276.28 / 294.15 ms |
| Frozen query warm P50 / P95 (100 samples) | 3.10 / 9.94 ms |
| Frozen query index construction | 0.958 s |
| First snapshot retention / repeated same snapshot | 7.235 / 0.167 s |

ADR-010 explains the optimization: retain each-query authoritative content/hash
validation, use shared locks and before/after metadata race detection to avoid a
second full content parse. No content freshness check is replaced by a persistent
metadata cache. An intermediate measurement overlapping development builds had
P95 253.76 ms; the final disclosed measurement above ran after those builds.
The local host is not an isolated dedicated benchmark machine. These are
synthetic-corpus results, not natural-corpus relevance or arbitrary-PDF accuracy.

The numeric 250 ms warm target passes on this disclosed host. The exact planned
8-core/16 GiB/local SSD reference configuration and true storage-cold performance
remain unmeasured, so no universal hardware guarantee is claimed. Snapshot setup
is a real per-workflow cost; quoted 9.94 ms excludes it. Live-model latency,
answer support and cost remain separate opt-in evaluation work.

Aggregate machine-readable evidence is in [the results directory](results/README.md).
The per-input manifest and generated Wiki are intentionally excluded from Git.
