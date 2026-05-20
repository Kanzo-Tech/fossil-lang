//! `fossil-codegen` — MIR → `DuckDB` SQL + `GraphAr` manifest YAML.
//!
//! Phase 1 ships 3-op + Sink lowering using the sqlparser hybrid pattern
//! (RESEARCH.md Pitfall 6) — hand-formatted templates wrap a trivial inner
//! `SELECT` so we never round-trip the `DuckDB` `COPY (...) TO '...' (FORMAT
//! PARQUET)` statement through `sqlparser` (which does not preserve the
//! parenthesised options form).
//!
//! Phase 4 (CORE-10) extends codegen to all 11 operators + R1-R10-rewritten
//! MIR. Phase 5 (SINK-01..06) replaces [`manifest::manifest_template`] with
//! programmatic generation via `serde_yaml_ng`.
//!
//! # Phase 2-9 contract (locked)
//!
//! Public Salsa query signature [`sql::codegen_sql`] and [`sql::SqlPlan`]
//! tracked struct shape are stable for downstream phases. The SQL textual
//! output is locked by the snapshot test in
//! `tests/compile_hello.rs` (Phase 1 success criterion #5 from
//! ROADMAP).

pub(crate) mod ast;
pub mod manifest;
pub mod sql;

// Type re-exports follow the rust-analyzer convention used by `fossil-hir`
// and `fossil-mir`: types at the crate root, query functions stay under
// their module path (`fossil_codegen::sql::codegen_sql`).
pub use manifest::manifest_template;
pub use sql::{SqlPlan, codegen_graph, codegen_graph_for_test, codegen_sql};
