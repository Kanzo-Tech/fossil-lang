# fossil-graph

The query surface over GraphAr+DuckDB. **Transport-agnostic** — verbs and their JSON Schemas live here; the wire bindings (MCP, HTTP, CLI, in-process TS) live in sibling crates / packages and consume this surface.

This crate is the Rust analogue of what `fossil-ide` does for the editor side: the logic, not the protocol. ADR-0001 makes the same split for LSP (`fossil-ide` carries hover/completion/goto-def; `fossil-lsp` carries the JSON-RPC wire). ADR-0039 formalises the same shape for the graph query layer.

## The 6 verbs

```text
Read:         read · expand{into|all} · path
Aggregation:  aggregate            ← binning included; it is a grouping
Introspect:   schema               ← the lists, and field stats on request
Escape:       execute_sql          ← text2sql lives HERE
```

Seventeen once. Eleven left, and only four of them were deleted rather than absorbed: `search_by_label`, `summarize_cluster`, `answer_with_communities` and `set_selection` never had an implementation, and three of the four were never verbs. The rest collapsed into the six — the four `list_*`/`describe_*` into `schema`, `histogram` into `aggregate` (binning is grouping), `top_k` and `get_vertex` into `read` (both were rows of one type under a predicate, an order and a limit).

`viewport` and `materialize_graph` are gone with nowhere to go. **The camera is addressed, not queried** (ADR-0042): the LOD is not a filter but a different relation — a level-3 tile holds super-nodes that do not exist at level 0 — and a `WHERE` selects rows from a table rather than changing which table is read. Pruning is which bytes are read, and DuckDB is measured not to prune by predicate: a range join against the ids of a window costs more than not pruning at all. That is the tiles' job, and a tile is not a verb.

No verb draws. If a filter must change the picture it answers with ids, and the canvas masks its resident tiles with them.

Adding a verb is a 4-touch change: enum variant in `operations/mod.rs` + `Params` + `Result` + snapshot test under `tests/schemas.rs`. The closed-set discipline is the contract — and the bar for a seventh is ADR-0042's: a new path enters only when it serves a case the single one demonstrably cannot, and that demonstration is a measurement.

## Bindings

```text
                          fossil-graph (this crate)
                                   ▲
                                   │
       ┌──────────────┬────────────┴────────────┬──────────────────┐
   fossil-mcp     fossil-http               fossil-cli       @fossil-lang/graph
   (W4)           (future)                  (CLI extension)  (W5, in-process TS)
```

## Status

Every verb is implemented and snapshot-tested; the `DuckExecutor` seam has a native impl in `fossil-runtime` and a browser one in `fossil-graph-wasm`, and neither re-derives a verb's SQL.

**W3 landed in `fossil-runtime::layout`, not in `fossil-sinks`** — worth knowing, because looking for it in the writer finds nothing. The layout needs the resolved edge set in `dense_id` space, which only exists *after* the vertex and edge COPYs have run, and `fossil-sinks` emits SQL from pure data and cannot read N rows. `enrich_layout` fills `x`/`y` from a modularity community partition placed by phyllotaxis, and rewrites each vertex Parquet Morton-sorted.

That sort is still what the tiles will be addressed through, but it is no longer what a verb queries: ADR-0042 measured the bbox predicate reading the whole file anyway. **The hierarchy gives the levels of aggregation; the Morton order gives the ranges of bytes.** They are two orthogonal structures, and ADR-0042 §3 confused them into one.

See `decisions/0039-fossil-graph-surface.md` for the surface ADR and `decisions/0042-la-camara-se-direcciona.md` for what closed it at six.
