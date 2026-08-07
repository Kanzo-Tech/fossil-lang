//! `fossil-mir` — Mid-level IR (typed operator algebra).
//!
//! Ships the complete typed algebra (`Source`, `Project`, `Extend`, `Rename`,
//! `Filter`, `Join`, `Union`, `GroupBy`, `Aggregate`, `Distinct`, `TripleEmit`,
//! `Sink`) plus an [`op::Op::Empty`] node (R9 target) and a typed [`op::Expr`]
//! ADT. The HIR → MIR lowering [`lower::lower_to_mir_pg`] lowers the
//! property-graph shape (`Source`, `EmitVertex`, `EmitEdge`, `Sink`) from
//! `.fossil` source; the remaining operators are reachable through the R1-R10
//! rewriting and are exercised via direct `MirGraph` construction (ADR-0009).
//! `fossil-df` consumes the [`graph::MirGraph`] and executes it on `DataFusion`.
//!
//! # Failure discipline
//!
//! Lowering never substitutes a default for something it could not resolve, and
//! never panics (the LSP lowers on every keystroke). It taints instead — see
//! [`graph::MirGraph::error`].
//!
//! See `operator-algebra.md` for the full algebra spec.

pub mod eval;
pub mod graph;
pub mod lower;
pub mod op;
pub mod rewrite;
pub mod schema;
pub mod skeleton;

// Type re-exports follow the rust-analyzer convention used by `fossil-hir`:
// types at the crate root, query functions stay under their module path
// (`fossil_mir::lower::lower_to_mir_pg`) to avoid name shadowing with modules.
pub use eval::{partial_eval, static_truth};
pub use graph::MirGraph;
pub use lower::{apply_output_shape, lower_to_mir_pg};
pub use op::{AggFn, AggSpec, Expr, JoinKind, Op, SinkRef, SourceFormat, VProp};
pub use rewrite::rewrite;
pub use schema::{free_cols, schema_of};
pub use skeleton::{subject_template_skeleton, template_skeleton};
