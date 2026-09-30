//! `fossil-graph` — query surface over GraphAr+DuckDB.
//!
//! This crate defines the **superficie**: an enum of typed graph operations
//! ([`Operation`]) with per-variant `Params` and `Result` structs. Each
//! struct derives [`schemars::JsonSchema`] so the wire contract is published
//! once and consumed by every transport binding without hand-written JSON:
//!
//! ```text
//!   fossil-graph (this crate)   ← verbs + schemas + the addressing plan
//!          ▲
//!   fossil-graph-wasm           ← wasm-bindgen; `@fossil-lang/corpus` on top,
//!                                  in-process TS over the host's DuckDB-WASM
//! ```
//!
//! **One binding, and the list is derived rather than remembered**: the crates
//! whose `[dependencies]` name this one are exactly `fossil-graph-wasm`. There
//! was a native MCP server beside it; it had no consumer and was deleted.
//!
//! ## Verbs (2)
//!
//! ```text
//! Introspect:   schema               ← the lists, and field stats on request
//! Escape:       execute_sql          ← text2sql lives HERE
//! ```
//!
//! Each verb is a variant on [`Operation`]; the `Params` and `Result` shapes
//! are pure data with no transport coupling. The binding runs one through
//! [`dispatch`], which matches on the enum.
//!
//! **No verb draws. The camera is addressed, not queried** — a zoom is a
//! different RELATION and not a filter, and a `WHERE` cannot change which table
//! it reads. The payload at `z = Z` and each rung of the cell tree below it are
//! separate artefacts, each tiled on one `dense_id` axis, so a camera computes a
//! zoom and tile addresses and asks for bytes; no bbox, no SQL, no `DuckDB` on
//! that path. A filter that must change the picture answers with ids and the
//! canvas masks its resident tiles with them — one mechanism, not a second
//! renderer.
//!
//! ## The executor is a trait
//!
//! This crate depends on [`fossil_sinks`] (manifest types — WASM clean) and on
//! nothing else of fossil's; `docs/content.test.ts` fails if that list is
//! ever anything but `["fossil-sinks"]`. It carries no wasm32 tripwire, because
//! the verb logic is WASM-safe.
//!
//! The one `DuckExecutor` impl is `fossil-graph-wasm`'s `JsExecutor`, not
//! `fossil-wasm`, which does not depend on this crate at all.
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
pub mod plan;

pub use error::{GraphError, Result};
pub use executor::{DuckExecutor, QueryResult, dispatch};
pub use operations::{Operation, RawSql, RawSqlAccess, Verb};
pub use plan::{Container, Direction, ReadPlan, resolve as resolve_corpus};
