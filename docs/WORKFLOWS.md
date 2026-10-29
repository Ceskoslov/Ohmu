# Cited workflows

M3 application composition is implemented in `ohmu_nausicaa::workflow` and the
CLI `ask` / `compile` commands. It reuses Nausicaa AgentRuntime, ModelAdapter,
policy projection, cancellation and JsonlEventStore. No second agent loop or
provider client exists in the engine. Live quality remains unmeasured.

## Query consistency and authority

A workflow retains an immutable snapshot, then builds a query-local Tantivy index
with the same Jieba tokenizer and BM25 fields. Page reads, links and search stay
on this snapshot after editor changes. Sources must belong to its manifest;
immutable artifacts are verified on read. Construction has a 512 MiB normalized
text budget. Snapshot/index construction contributes to elapsed query time and
can be expensive; lexical warm latency excludes that setup and model latency.

Only four read tools are visible. Source text is advisory evidence; it cannot
install rules, approve capabilities or publish. Model output is separately
accepted after the harness turn: TurnCompleted alone is insufficient.

`ask` requires JSON with `facts`, `synthesis`, `contradictions` (arrays of text
and citations) and `evidence_gaps` (strings). Every claim needs original-source
citations. Each locator and quote is checked against immutable source bytes,
including physical PDF page boundaries. This proves quote identity, not semantic
support or inference correctness. Unsupported questions should return gaps.
Truncated, malformed or invented citations fail acceptance. Partial results
include collected tool evidence, usage, elapsed time and an explicit reason.

`compile` requires a ChangeProposal; every page needs validated original-source
citations and the exact query-snapshot base hash (null only for creation).
Acceptance stages it through the M2 engine and never publishes it. Live edits
can therefore invalidate a proposed update. Review using draft/diff and explicitly
apply later. An answer can be filed by asking compile to propose the supported
knowledge; its own Wiki prose is never treated as independent source evidence.

## Budgets and persistence

Defaults and hard upper bounds: 12 model calls, 120 seconds, top20 candidates,
three distinct search queries (initial plus two rewrites), three navigation calls
(total, a conservative hop bound), 48 tool executions and eight calls per model
response. Model passages are capped at 64 KiB. A 512 KiB serialized context guard
is a memory bound, not token accounting. Model deadlines/cancellation interrupt
async reads; bounded synchronous parsing/counter work can delay cancellation.
There are no automatic model retries. Budget failures preserve evidence and a
partial report, without accepting an unvalidated answer or staging a draft.

The caller supplies a **new**, exclusively owned JSONL journal path and acceptance
path. Acceptance destination creation is checked before model calls or staging. Durable
receipts precede the next model call. Independent acceptance is written to the
same basename with `.acceptance.json`; preserve both files. An interrupted process can leave an empty/incomplete acceptance
file: inspect durable drafts and journals before retrying. Rejected reports use
`accepted:false`, `partial:true`, `reason`; callers must inspect acceptance, not
infer it from process success. No unknown write is automatically replayed: inspect
M2 drafts/operations after any uncertain stage or receipt failure.

## Live CLI configuration

The CLI reuses Nausicaa's pinned OpenAI-compatible native reqwest transport.
Provider endpoints are explicit trusted host configuration, separate from public
source-import URL policy. Never put credentials in endpoint URLs. API keys are
read only on an explicitly invoked live command, from the named environment
variable; unauthenticated local endpoints require `--no-api-key`.

```sh
cargo run -p ohmu-cli -- ask 'What does the evidence establish?' \
  --wiki /path/to/wiki --endpoint https://provider.example/v1/chat/completions \
  --model PINNED_MODEL --context-window 65536 --output-tokens 4096 \
  --token-counter /absolute/path/to/trusted-model-counter \
  --events /path/to/new-query.jsonl --json
```

Replace `ask` with `compile` for draft generation. The endpoint above is a
placeholder. There is no universally correct built-in tokenizer. The trusted
counter executable receives the **final provider request body** on stdin and
returns only `{"input_tokens":N}` on stdout. It must count message framing,
tool schemas and extension fields for the selected model, or fail. The host
must install a vetted model-specific implementation; approximate bytes/4 or a
constant counter is unsuitable for live use. The executable is never exposed to
the model and is invoked directly without a shell, with a five-second deadline
and bounded output. Request bodies may contain private source text; trust the
counter as part of the host. API credentials are transport headers, not stdin.
The provider reserves/caps output and refuses an overflowing/uncountable request
before transport. Library hosts can supply RequestTokenCounter directly.

## Evaluation limits

Deterministic fixtures cover integration, scope, stale snapshot isolation,
missing/forged evidence, invented nodes, truncation, tool denial, budgets and
cancellation. They do not measure live answer support. Opt-in live evaluation
must pin endpoint/model/counter, use held-out human annotations and report quote
correctness, answer support, contradictions, abstention, usage/cost and latency
separately. No API credentials or paid requests are required by ordinary tests.
