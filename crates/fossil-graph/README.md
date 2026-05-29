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

W1 ships the surface: every `Params`/`Result` pair is implemented and snapshot-tested, but the execution side is stubbed (`GraphError::NotImplemented`). W2 lands the `DuckExecutor` trait + native impl in `fossil-runtime`. W3 unlocks the viewport / GraphRAG verbs by enriching the writer (`fossil-sinks`).

See `decisions/0039-fossil-graph-surface.md` for the full ADR.
