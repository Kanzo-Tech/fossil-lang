//! `fossil-mir` — Mid-level IR (typed operator algebra).
//!
//! Phase 1 ships 3 of 11 operators (`Source`, `Extend`, `TripleEmit`) plus
//! the `Sink` terminal. The HIR → MIR lowering [`lower::lower_to_mir`]
//! produces a 4-node DAG for the canonical `examples/hello.fossil` mapping;
//! the codegen crate (`fossil-codegen`) consumes the [`graph::MirGraph`] and
//! emits `DuckDB` SQL.
//!
//! # Phase 2-9 contract (locked)
//!
//! Public Salsa query signature, [`graph::MirGraph`], and the [`op::Op`]
//! enum surface (variant set, field names, types) are stable for downstream
//! phases. Phase 4 (CORE-08..10) only ADDS variants (`Project`, `Rename`,
//! `Filter`, `Join`, `Union`, `GroupBy`, `Aggregate`, `Distinct`) and wires
//! the R1-R10 rewriting pass; existing variants do not change shape.
//!
//! See `operator-algebra.md` for the full algebra spec.

pub mod graph;
pub mod lower;
pub mod op;

// Type re-exports follow the rust-analyzer convention used by `fossil-hir`:
// types at the crate root, query functions stay under their module path
// (`fossil_mir::lower::lower_to_mir`) to avoid name shadowing with modules.
pub use graph::MirGraph;
pub use lower::lower_to_mir;
pub use op::{ExprLowered, Op, SinkRef, SourceFormat};
