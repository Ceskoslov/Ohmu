# Nausicaa integration contract

## Supported baseline

Repository: [Ceskoslov/Nausicaa](https://github.com/Ceskoslov/Nausicaa).
Initial supported commit: `a2ab5b365cfb29bedd48d29b69951510ae5d1ef5`.
Ohmu uses agent-harness-core 0.1.0 from that exact Git revision. The working
Nausicaa repository was inspected but is not changed by this delivery.

Existing seams used: Tool/ToolRegistry, CanonicalAction/PreparedToolCall,
CapabilityPolicy, DirectExecutor, ModelAdapter, LayeredContextCompiler,
JsonlEventStore and cancellation. ContextCompiler and existing provider transports
remain host extension points. Nausicaa task acceptance and task-ledger crates are
not general schedulers and are not required by Ohmu's read tool.

## Minimal composition

The complete runnable example is
[read_wiki.rs](../crates/ohmu-nausicaa/examples/read_wiki.rs).
Its essential registration is:

```rust,ignore
let wiki = Arc::new(ohmu_engine::Wiki::open(wiki_root)?);
let mut tools = agent_harness_core::ToolRegistry::new();
ohmu_nausicaa::register_read_tools(&mut tools, wiki)?;
// Host supplies model, event store, context compiler, policy and executor.
let runtime = agent_harness_core::AgentRuntime::new(
    model,
    event_store,
    context_compiler,
    tools,
    Arc::new(ohmu_nausicaa::read_only_policy()),
).with_executor(Arc::new(agent_harness_core::DirectExecutor));
```

The policy helper is for a dedicated wiki-only runtime. In an application with
other tools, add `.grant("ohmu_read", Access::Allow)` to that application's
existing policy instead of replacing all of its grants. The model receives only
policy-projected tools. A fabricated denied call is still denied at execution.

DirectExecutor executes this trusted in-process Rust implementation. It does not
enable shell/process tools or put every host operation inside an OS sandbox.
Use the host's existing executor when composing other tool implementations.

## Exact-action read behavior

The only S0 tool is:

```json
{"page_id":"welcome"}
```

Unknown fields and path-shaped IDs are rejected. `prepare` reads the page to freeze
its SHA-256 revision and returns a canonical ReadOnly/Safe action with page ID,
revision and wiki scope. `execute` validates that exact contract and reads only the
matching revision. An intervening edit fails rather than substituting new content.
Cancellation is checked before prepare and before/after execution. Filesystem work
is synchronous and bounded; this is not in-flight asynchronous file cancellation.

Result fields are `api_version`, `page_id`, `revision` and `content`. S0 returns a
whole page up to 256 KiB; it does not yet return source-document citations or
pagination. Imported text cannot grant capabilities or become directory rules.

The runtime owns execution-start events and durable receipts. Tests independently
read the JSONL file during the next model call and replay it after closing the
runtime. The example requires a fresh event file; a trusted host must ensure sole
ownership of a persistent journal across processes.

## Portable pinned Git consumption

An application outside the Nausicaa workspace should use the same source for the
core that Ohmu uses. Before Ohmu has a remote, use a relative local adapter path:

```toml
[dependencies]
ohmu-engine = { path = "../Ohmu/crates/ohmu-engine" }
ohmu-nausicaa = { path = "../Ohmu/crates/ohmu-nausicaa" }
agent-harness-core = { git = "https://github.com/Ceskoslov/Nausicaa.git", rev = "a2ab5b365cfb29bedd48d29b69951510ae5d1ef5" }
```

Adjust relative paths to the consumer manifest. Do not invent an Ohmu remote URL
or rely on a crates.io publication that does not exist. Keep the supported Git
URL/revision consistent across the host, adapter and optional provider crates.

## Local two-repository development

Cargo identifies packages by source as well as name/version. A host importing
core through a local path and an adapter importing it through Git otherwise
gets different ToolRegistry/Tool types. Patch at the **consuming workspace root**:

```toml
[patch."https://github.com/Ceskoslov/Nausicaa.git"]
agent-harness-core = { path = "../Nausicaa/crates/harness-core" }
```

For a consumer inside the Nausicaa workspace, the same entry uses
`path = "crates/harness-core"` in that workspace's root manifest. A root-local
adapter dependency from Nausicaa would be `../Ohmu/crates/ohmu-nausicaa`; a member
manifest needs a path relative to that member instead. The adapter depends only
on harness-core, not the Nausicaa facade or application, so this creates no cycle.

If the consumer enables optional provider crates from both Git and local sources,
patch those packages at the same root too. Current S0 has no provider dependency.
Patches in a dependency's manifest do not control the final application's graph.
Absolute host-specific overrides must not be committed; an ignored
`.cargo/config.toml` can hold a local `[patch]` table instead.

Run the executable compatibility probe from Ohmu:

```sh
python3 scripts/check_local_host.py --nausicaa ../Nausicaa
```

It creates a temporary independent workspace, imports the local core directly and
the local Ohmu adapter, applies the Git-source patch, compiles a typed registration
boundary and inspects Cargo metadata for exactly one harness core from the chosen
local path. The script writes build artifacts under Ohmu's ignored target tree,
does not edit Nausicaa, and does not replace Ohmu's committed Git-source lockfile.
It uses offline Cargo resolution, so fetch normal dependencies first.

## Planned tool and host extensions

| Tool | Contract | Stage |
| --- | --- | --- |
| ohmu_read | Bounded ID/revision read; later evidence pagination | S0 / M1 |
| ohmu_search | Lexical candidates with collection/revision/locators | M1 |
| ohmu_neighbors | Explicit incoming/outgoing links and document children | M1 |
| ohmu_ingest | Explicit local-write capability; URL fetching additionally declares external effects | M1 |
| ohmu_stage | Persist a structured draft; no published page changes | M2 |
| ohmu_apply | Exact changeset digest, base versions and operation ID; Ask by default | M2 |

Mixed network/write effects must be conservatively represented by the existing
EffectKind enum and separate grants; do not modify core's serialized enum just to
hide those effects behind a read classification. `prepare` validates and freezes
an import request but must not perform the network/write effect before approval.

The M3 host wraps existing context compilation with a small advisory Wiki usage
segment and per-query snapshot identifier. It reads content through tools rather
than loading Wiki/source directories as executable rules. It reuses the existing
ModelAdapter and optional native reqwest transport, with explicit token-window,
call-count and time budgets. Never recursively invoke AgentRuntime inside a Tool.

## Compatibility and upgrades

To upgrade: inspect Nausicaa API and replay changes; update the revision in the
workspace manifest; regenerate the lockfile; run protocol tests, example,
Git-source dependency check and the temporary local-host probe. Record the new
supported revision and checks in VALIDATION. Preserve Nausicaa's default-deny,
exact-action, cancellation and receipt-ordering invariants. A normal completed
turn is not proof that a knowledge update passed independent acceptance.

Ohmu owns its future schema migrations; this adapter does not reinterpret old
Nausicaa events. Unknown publication receipts will be reconciled by durable Ohmu
operation IDs. Without a known result, the host reports uncertainty rather than
automatically retrying a write.
