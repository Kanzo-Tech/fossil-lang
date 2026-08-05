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
//! ## Verbs (6)
//!
//! ```text
//! Read:         read · expand{into|all} · path
//! Aggregation:  aggregate            ← binning included; it is a grouping
//! Introspect:   schema               ← the lists, and field stats on request
//! Escape:       execute_sql          ← text2sql lives HERE
//! ```
//!
//! Each verb is a variant on [`Operation`]; the `Params` and `Result` shapes
//! are pure data with no transport coupling. Transport bindings implement
//! `dispatch(op, ctx) → Result` by matching on the enum.
//!
//! **No verb draws.** ADR-0042: the camera is addressed, not queried — the LOD
//! is not a filter but a different relation, and a `WHERE` cannot change which
//! table it reads. A filter that must change the picture answers with ids and
//! the canvas masks its resident tiles with them.
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
//! ## What bounds a verb
//!
//! Every verb here is bounded by a `LIMIT` or by a `GROUP BY` whose
//! cardinality is capped, so cost is a function of the answer rather than of
//! the corpus. What is NOT available is pruning by predicate: ADR-0042
//! measured that `DuckDB` evaluates a range predicate per row instead of
//! skipping row groups, so a window expressed as a `WHERE` reads the whole
//! file. **Pruning is which bytes are read, and that is the tiles' job, not a
//! verb's.**

pub mod error;
pub mod exec;
pub mod manifest;
pub mod operations;

pub use error::{GraphError, Result};
pub use exec::{ColumnedRows, DuckExecutor, dispatch};
pub use operations::Operation;
