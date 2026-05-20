//! `fossil-mir` — Mid-level IR (typed operator algebra).
//!
//! Phase 4 (CORE-08..10) ships the complete 11-operator typed algebra
//! (`Source`, `Project`, `Extend`, `Rename`, `Filter`, `Join`, `Union`,
//! `GroupBy`, `Aggregate`, `Distinct`, `TripleEmit`, `Sink`) plus an
//! [`op::Op::Empty`] node (R9 target) and a typed [`op::Expr`] ADT (replacing
//! the Phase 1 untyped `ExprLowered`). The HIR → MIR lowering
//! [`lower::lower_to_mir`] lowers only the 4 source-reachable operators
//! (`Source`, `Extend`, `TripleEmit`, `Sink`) from `.fossil` source — the
//! other 7 are exercised via direct `MirGraph` construction (ADR-0009). The
//! codegen crate (`fossil-codegen`) consumes the [`graph::MirGraph`] and emits
//! `DuckDB` SQL.
//!
//! # Phase 2-9 contract (locked)
//!
//! Public Salsa query signature ([`lower::lower_to_mir`]) and
//! [`graph::MirGraph`] are stable for downstream phases. Phase 4 ADDED the 7
//! remaining operators + `Op::Empty` + the typed `Expr` ADT and generalised
//! the lowering body; the locked query signature is unchanged.
//!
//! See `operator-algebra.md` for the full algebra spec.

pub mod graph;
pub mod lower;
pub mod op;
pub mod rewrite;
pub mod schema;

// Type re-exports follow the rust-analyzer convention used by `fossil-hir`:
// types at the crate root, query functions stay under their module path
// (`fossil_mir::lower::lower_to_mir`) to avoid name shadowing with modules.
pub use graph::MirGraph;
pub use lower::lower_to_mir;
pub use op::{AggFn, AggSpec, CmpOp, Expr, JoinKind, Op, SinkRef, SourceFormat};
pub use rewrite::rewrite;
pub use schema::{free_cols, schema_of};
