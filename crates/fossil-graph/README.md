# fossil-graph

The query surface over GraphAr+DuckDB. **Transport-agnostic** — verbs and their JSON Schemas live here; the wire bindings (MCP, HTTP, CLI, in-process TS) live in sibling crates / packages and consume this surface.

This crate is the Rust analogue of what `fossil-ide` does for the editor side: the logic, not the protocol. ADR-0001 makes the same split for LSP (`fossil-ide` carries hover/completion/goto-def; `fossil-lsp` carries the JSON-RPC wire). ADR-0039 formalises the same shape for the graph query layer.

## The 14 verbs

```text
Schema:       list_vertex_types · list_edge_types · describe_field
Discovery:    search_by_label · find_neighbors · find_path
Aggregation:  aggregate · histogram · top_k
GraphRAG:     summarize_cluster · answer_with_communities
Viewport:     viewport · set_selection
Escape:       execute_sql                              ← text2sql lives HERE
```

Adding a verb is a 4-touch change: enum variant in `operations/mod.rs` + `Params` + `Result` + snapshot test under `tests/schemas.rs`. The closed-set discipline is the contract.

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

W1 ships the surface: every `Params`/`Result` pair is implemented and snapshot-tested, but the execution side is stubbed (`GraphError::NotImplemented`). W2 lands the `DuckExecutor` trait + native impl in `fossil-runtime`.

**W3 landed in `fossil-runtime::layout`, not in `fossil-sinks`** — worth knowing, because looking for it in the writer finds nothing. The layout needs the resolved edge set in `dense_id` space, which only exists *after* the vertex and edge COPYs have run, and `fossil-sinks` emits SQL from pure data and cannot read N rows. `enrich_layout` fills `x`/`y` from a weakly-connected-components partition placed by phyllotaxis, and rewrites each vertex Parquet Morton-sorted so a bbox query prunes on row-group statistics. `viewport` accordingly returns real positions, the edges both of whose endpoints are visible, and a `GROUP BY cluster_id` aggregate mode below the LOD threshold.

Still open on the writer side: Leiden to split the giant component (W3.2), ForceAtlas2 seeded from the current placement (W3.3), and embeddings (W3.4). The last is why `search_by_label` stays `NotImplemented`: its `score` is specified as a cosine similarity, and a substring match wearing that name would be a lie in the wire contract every binding reads.

See `decisions/0039-fossil-graph-surface.md` for the full ADR.
