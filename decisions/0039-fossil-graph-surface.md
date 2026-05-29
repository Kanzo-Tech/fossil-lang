# ADR 0039: Separate the query surface (`fossil-graph`) from its transport bindings (MCP, HTTP, CLI, in-process TS)

**Date:** 2026-05-29
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Auto-memory `project_fossil_graph_reference_architecture.md`; deck `/Users/angel.ip/Desktop/09_Keasy.pptx` slide 22 ("Serverless · Larger-than-RAM · Chunking"); ADR-0001 (LSP framework — same logic/protocol split applied to `fossil-ide` vs `fossil-lsp`).

## Context

Keasy's discovery views and assistant-wizard hit the underlying GraphAr property graph through three different code paths today: (a) the browser viewer materialises `VertexRow[]` from a Mosaic+DuckDB-WASM coordinator (`web/src/components/discovery/use-graph-data*.ts`), (b) the server-side `/v1/jobs/:id/ask` endpoint runs text2sql with GraphAr conventions baked into the LLM prompt (`server/src/ai/routes.rs`), and (c) the `@fossil-lang/viewer` package re-implements the same Float32Array building for its standalone consumers. Each path duplicates GraphAr-layout knowledge — vertex table naming, `_id` column, `{Source}_{predicate}_{Target}` edge convention, label-column preferences, type-index palettes — and each path topped out at ~1M vertices because the materialise-then-pass-to-cosmos pattern breaks down before it.

The deck's headline claim is "Larger-than-RAM · Chunking · Serverless". Honouring that demands an architecture where the wire transport, the working set, and the render path are all bounded; honouring it ACROSS keasy, the playground, the CLI, and future AI-agent integrations (Claude Desktop, Cursor, MCP-capable hosts) demands that the GraphAr-layout knowledge live in ONE place, with every consumer reading the same contract.

Three families of solutions were considered:

1. **Pure text2sql endpoint** (the current path, hardened). Pros: maximum expressivity, single round-trip per question. Cons: GraphAr knowledge leaks into the LLM prompt, no multi-step retrieval, cost unbounded, no portable contract for non-LLM consumers (CLI, dashboards).
2. **MCP-monolithic** — name the entire layer after Anthropic's Model Context Protocol. Pros: aligns with AI-agent ecosystem. Cons: locks the surface to a single transport (JSON-RPC stdio/SSE), excludes CLI/HTTP/in-process consumers, ties branding to one vendor's standard. Same mistake as if `fossil-ide` were called `fossil-lsp`.
3. **Separate surface from transport** (this ADR). Define the verbs in a transport-agnostic crate, ship bindings as small adapters.

## Decision

We will introduce a crate `fossil-graph` that owns the query surface — a closed enum of 14 typed operations (`list_vertex_types`, `list_edge_types`, `describe_field`, `search_by_label`, `find_neighbors`, `find_path`, `aggregate`, `histogram`, `top_k`, `summarize_cluster`, `answer_with_communities`, `viewport`, `set_selection`, `execute_sql`) with serde-derived `Params` and `Result` structs and `schemars`-derived JSON Schemas. The crate carries no transport code: no JSON-RPC, no HTTP server, no WASM bindings. Transport adapters live in separate crates / packages:

- `fossil-mcp` — JSON-RPC over stdio + HTTP+SSE, the binding for AI agents (Claude Desktop, Cursor, the keasy-proxied assistant chat).
- `fossil-http` — REST + SSE, the binding keasy authenticates and proxies.
- `fossil-cli` — extends the existing CLI with `fossil query <verb> --params=…`.
- `@fossil-lang/graph` — TS in-process binding compiled to WASM via `fossil-wasm`; consumed by the playground (DuckDB-WASM local) and by the keasy browser viewer.

`fossil-graph` depends on `fossil-sinks` to share the GraphAr manifest types (single source of truth for vertex/edge metadata between write and read paths). Execution is delegated through a `DuckExecutor` trait (W2 deliverable) so the same verb logic compiles for native (via `fossil-runtime` + `duckdb`) and for WASM (via `fossil-wasm` + `duckdb-wasm`).

The verb count and naming are locked to the snake-case strings shown above. Adding a verb requires a new enum variant + a `Params`/`Result` pair + a snapshot test under `tests/schemas.rs`; removing a verb is a breaking change to every binding (Rust exhaustive match + TS-codegen consumers).

The `execute_sql` verb is the explicit text2sql escape hatch. Bindings MAY gate it behind a permission flag: the keasy proxy will refuse the verb for participant-role users while allowing the other 13.

## Consequences

**What becomes easier**

- Every consumer of GraphAr — browser, CLI, AI agent, server proxy — reads the same JSON Schema and reaches the same SQL. Updating a verb (e.g. adding a `vertex_types` filter to `histogram`) is a single edit in `fossil-graph` plus a snapshot review; every binding inherits the change at the next codegen step.
- The "larger-than-RAM" contract lives in one place — verb bodies that scan vertex/edge tables MUST translate to predicate-pushdown-friendly SQL. The writer (`fossil-sinks` W1/W3) is the upstream that lets these restrictions be observed, and the seam between writer and reader is the manifest struct, not a string prompt.
- AI agents reach the graph through a standard protocol (MCP) without keasy needing custom NL-to-SQL prompt engineering. The `system_prompt` for keasy's chat collapses to "use these tools"; the GraphAr knowledge is encoded in the tool descriptions emitted from `fossil-graph` JSON Schemas.
- The CLI gains parity with the browser and the agent — `fossil query top_k --vertex-type Person --order-by created_at --k 10` works against any GraphAr manifest, useful for scripting and CI assertions.

**What becomes harder**

- Adding a verb is a 4-touch change: enum variant + Params + Result + snapshot. Friction by design — the closed set is the contract.
- The TS binding (`@fossil-lang/graph`) must compile to WASM to call DuckDB-WASM, which means the verb bodies' Rust must be WASM-clean. We pay this price elsewhere already (the 9-crate WASM gate per ADR-0002); `fossil-graph` joins it once W2 implements execution. For W1 the impls are stubs so the WASM gate question is deferred.
- The keasy `/v1/jobs/:id/ask` endpoint (just hardened in commit `42c176c`) becomes transitional. The MCP-proxy migration in W4 deletes its bespoke text2sql prompt and replaces it with a JSON-RPC proxy to `fossil-mcp`. Until then, the two paths coexist and the bugfix prompt earns its keep.

**New risks**

- The closed verb set must be right. If a real workflow doesn't fit in the 14 verbs and the keasy host has the `execute_sql` escape hatch disabled for the user role, the workflow is blocked. Mitigation: a clear escalation policy — verbs are additive, ADRs document each add, the keasy proxy can flip the `execute_sql` gate per-role.
- JSON Schema generation via `schemars 0.8` is locked to draft-07 (interop with `openapi-typescript` consumers). When the TS codegen tooling moves to draft-2020-12 we revisit; until then this pin is intentional.
- `fossil-graph` reads GraphAr manifests authored by `fossil-sinks` — if the writer schema evolves, the reader must follow. Tests in `fossil-graph` MUST include a round-trip against an actual `fossil-sinks`-emitted manifest fixture (W2 deliverable) so a writer-side breaking change shows up red here, not silently in keasy.

## Implementation map

The W1 PR opens the crate with the enum and the 14 `Params`/`Result` pairs and the snapshot test gate. Execution is stubbed — all verbs return `GraphError::NotImplemented`. W2 lands `DuckExecutor` + native impl + the YAML manifest reader. W3 hardens the writer (`fossil-sinks`) so the larger-than-RAM verbs (`viewport`, `aggregate`, GraphRAG) can be implemented. W4 ships `fossil-mcp` + the keasy proxy migration. W5 ships `@fossil-lang/graph`.

The companion auto-memory `project_fossil_graph_reference_architecture.md` carries the full migration sequence, the line-of-code accounting, the writer-column inventory, and the risk register. This ADR is the durable upstream artefact.
