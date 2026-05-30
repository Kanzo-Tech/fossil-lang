//! `fossil-graph` — query surface over GraphAr+DuckDB.
//!
//! This crate defines the **superficie**: an enum of typed graph operations
//! ([`Operation`]) with per-variant `Params` and `Result` structs. Each
//! struct derives [`schemars::JsonSchema`] so the wire contract is published
//! once and consumed by every transport binding without hand-written JSON:
//!
//! ```text
//!         fossil-graph (this crate)        ← verbs + schemas
//!                  ▲
//!   ┌──────────────┼──────────────┬────────────────┐
//!   │              │              │                │
//! fossil-mcp   fossil-http   fossil-cli   @fossil-lang/graph
//!   (stdio       (HTTP+SSE      (terminal      (in-process TS,
//!    JSON-RPC      for          for             DuckDB-WASM)
//!    for AI        keasy        scripting)
//!    agents)       proxy)
//! ```
//!
//! The pattern follows fossil's existing split between logic and protocol
//! (ADR-0001: `fossil-ide` carries IDE features, `fossil-lsp` carries the
//! LSP wire). Per ADR-0039: MCP is one transport, not the protocol; calling
//! the entire surface "MCP" would lock fossil into an AI-agent framing when
//! the same verbs serve dashboards, federation, tests, and the CLI.
//!
//! ## Verbs (14)
//!
//! ```text
//! Schema:       list_vertex_types · list_edge_types · describe_field
//! Discovery:    search_by_label · find_neighbors · find_path
//! Aggregation:  aggregate · histogram · top_k
//! GraphRAG:     summarize_cluster · answer_with_communities
//! Viewport:     viewport · set_selection
//! Escape:       execute_sql                              ← text2sql lives HERE
//! ```
//!
//! Each verb is a unit-struct or unit-variant on [`Operation`]; the
//! `Params` and `Result` shapes are pure data with no transport coupling.
//! Transport bindings implement `dispatch(op, ctx) → Result` by matching on
//! the enum.
//!
//! ## Native-only (for now)
//!
//! This crate currently depends on [`fossil_sinks`] (manifest types — WASM
//! clean) and will gain a `DuckExecutor` trait in W2 for execution. The
//! native impl lives in `fossil-runtime`; the WASM impl in `fossil-wasm`
//! (TS-side calls a DuckDB-WASM connection). For W1 the impls are stubs
//! (`todo!()`), and the crate carries NO wasm32 tripwire because the verb
//! logic itself is WASM-safe.
//!
//! ## Larger-than-RAM contract (W3+)
//!
//! Verbs that scan the vertex/edge tables MUST emit SQL that `DuckDB` can
//! satisfy via predicate-pushdown over morton-sorted Parquet row groups.
//! Anti-patterns explicitly banned from the query path:
//!
//! - `row_number() OVER ()` without `PARTITION BY` — materialises the whole
//!   table to sort.
//! - JOINs whose hash-build side scans the whole vertex/edge table — must
//!   filter the build side first (`WHERE id IN (SELECT … FROM viewport)`).
//!
//! The writer (`fossil-sinks` W1+W3) is the upstream that lets these
//! restrictions be observed — it pre-computes `dense_id`, `x/y`, `cluster_id`,
//! `embedding`, and morton-sorts the Parquet so query SQL only needs to
//! WHERE+LIMIT.

pub mod error;
pub mod exec;
pub mod manifest;
pub mod operations;

pub use error::{GraphError, Result};
pub use exec::{DuckExecutor, dispatch};
pub use operations::Operation;
