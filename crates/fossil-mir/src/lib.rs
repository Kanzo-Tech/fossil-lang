//! `fossil-mir` — Mid-level IR (typed operator algebra).
//!
//! Ships the complete typed algebra (`Source`, `Project`, `Extend`, `Rename`,
//! `Filter`, `Join`, `Union`, `GroupBy`, `Aggregate`, `Distinct`, `EmitVertex`,
//! `EmitEdge`, `Sink`) plus an [`op::Op::Empty`] node and a typed [`op::Expr`]
//! ADT. The HIR → MIR lowering [`lower::lower_to_mir_pg`] lowers the
//! property-graph shape (`Source`, `EmitVertex`, `EmitEdge`, `Sink`) from
//! `.fossil` source; the remaining operators have no surface syntax and are
//! exercised via direct `MirGraph` construction.
//! `fossil-df` consumes the [`graph::MirGraph`] and executes it on `DataFusion`.
//!
//! # Failure discipline
//!
//! Lowering never substitutes a default for something it could not resolve, and
//! never panics (the LSP lowers on every keystroke). It taints instead — see
//! [`graph::MirGraph::error`].
//!
//! The algebra is Min Oo & Hartig's operational semantics for knowledge-graph
//! construction (arXiv 2503.10385; ESWC 2025) with a type on every operator's
//! schema; `/docs/design/algebra` is the page that argues it, and
//! [`op::Op`]'s own doc comments are the signatures.

pub mod graph;
pub mod lower;
pub mod op;
pub mod schema;

// Type re-exports follow the rust-analyzer convention used by `fossil-hir`:
// types at the crate root, query functions stay under their module path
// (`fossil_mir::lower::lower_to_mir_pg`) to avoid name shadowing with modules.
pub use graph::MirGraph;
pub use lower::{apply_output_shape, lower_to_mir_pg};
pub use op::{
    AggFn, AggSpec, Expr, JoinKind, JoinSide, Op, ProjectedColumn, SinkRef, SourceFormat, VProp,
};
pub use schema::{free_cols, schema_of};
