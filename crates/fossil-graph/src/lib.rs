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
//! fossil-mcp   fossil-cli   @fossil-lang/graph
//!   (stdio       (HTTP+SSE      (terminal      (in-process TS,
//!    JSON-RPC      for          for             DuckDB-WASM)
//!    for AI        keasy        scripting)
//!    agents)       proxy)
//! ```
//!
//! The pattern follows fossil's existing split between logic and protocol —
//! `fossil-ide` carries IDE features, `fossil-lsp` carries the LSP wire.
//! **MCP is one transport, not the protocol**: calling the entire surface
//! "MCP" would lock fossil into an AI-agent framing when the same verbs serve
//! dashboards, federation, tests, and the CLI.
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
//! **No verb draws. The camera is addressed, not queried** — the LOD is not a
//! filter but a different relation (a level-3 tile holds supernodes that do not
//! exist at level 0), and a `WHERE` cannot change which table it reads. The
//! camera computes a level and tile addresses and asks for bytes; no bbox, no
//! SQL, no `DuckDB` on that path. A filter that must change the picture answers
//! with ids and the canvas masks its resident tiles with them — one mechanism,
//! not a second renderer.
//!
//! ## The executor is a trait, and both impls exist
//!
//! This crate depends on [`fossil_sinks`] (manifest types — WASM clean) and on
//! nothing else of fossil's; `apps/docs/content.test.ts` fails if that list is
//! ever anything but `["fossil-sinks"]`. It carries no wasm32 tripwire, because
//! the verb logic is WASM-safe.
//!
//! The `DuckExecutor` trait exists and dispatch is implemented — this paragraph
//! said it "will gain" one "in W2" and that the impls were `todo!()` stubs. The
//! native impl is `fossil-mcp`'s `ConnectionExecutor`
//! (`crates/fossil-mcp/src/executor.rs`), NOT `fossil-layout`: that crate is the
//! layout post-pass and links no engine at all. The browser impl is
//! `fossil-graph-wasm`, not `fossil-wasm`.
//!
//! ## What bounds a verb
//!
//! Every verb here is bounded by a `LIMIT` or by a `GROUP BY` whose
//! cardinality is capped, so cost is a function of the answer rather than of
//! the corpus. What is NOT available is pruning by predicate: `DuckDB`
//! evaluates a range predicate per row instead of skipping row groups —
//! measured, and both spellings lost to not pruning at all (a range join
//! against the id runs, 237 ms; 179 `BETWEEN … OR …` predicates, 189 ms;
//! the unpruned join, 5 ms) — so a window expressed as a `WHERE` reads the
//! whole file. **Pruning is which bytes are read, and that is the tiles' job,
//! not a verb's.**

pub mod error;
pub mod executor;
pub mod manifest;
pub mod operations;

pub use error::{GraphError, Result};
pub use executor::{DuckExecutor, QueryResult, dispatch};
pub use operations::Operation;
