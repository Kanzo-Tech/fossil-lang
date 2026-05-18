//! `fossil-hir` — types + name resolution + minimal type check.
//!
//! Per ADR-0002 this crate collapses what was originally three crates
//! (`fossil-types + fossil-resolve + fossil-typeck`) into one, matching the
//! `ty_python_semantic` pattern from the research synthesis.
//!
//! # Phase 2 scope (current)
//!
//! - Full 11-kind [`Ty`] ADT (`Primitive`, `Optional`, `Seq`, `Record`,
//!   `Iri`, `IriTemplate`, `Shape`, `Fn`, `TripleTerm`, `Error`, `Unknown`)
//!   per CORE-03 + plan 02-05.
//! - [`DefMap`] = prefix table + source bindings + mapping list, populated by
//!   the [`def_map`] Salsa query.
//! - Interned [`MappingLoc`]/[`SourceLoc`] location IDs (rust-analyzer pattern,
//!   per RESEARCH.md §"Architecture Patterns" Pattern 2 + 3).
//! - [`item_tree`] signature-only query + [`body`] per-mapping body query
//!   (rust-analyzer invalidation-barrier pattern per ADR-0005, plan 02-04).
//! - [`lower_to_hir`] for header-only `HirFile` lowering; body content lives
//!   behind [`body`] (`body(db, MappingLoc) -> HirBody`).
//! - [`check::typecheck_mapping`] no-op stub returning
//!   `Result<(), ErrorGuaranteed>` (plan 02-05 migrated from the deleted
//!   Phase 1 local taint-wrapper newtype).
//!
//! # Phase 2-9 contract (locked)
//!
//! Public Salsa query signatures (`def_map`, `item_tree`, `lower_to_hir`,
//! `body`, `typecheck_mapping`) and the public types in [`ty`], [`def_map`],
//! [`item_tree`], [`body`], and [`lower`] are stable for downstream phases.
//! Phase 3 wires bidirectional checking + [`fossil_base::ErrorGuaranteed`]
//! propagation (CORE-04..07) atop the Phase 2 surface.

pub mod ast_id;
pub mod body;
pub mod check;
pub mod def_map;
pub mod item_tree;
pub mod lower;
pub mod ty;

// Type re-exports only; the query functions are intentionally kept under their
// module paths (`fossil_hir::def_map::def_map`, `fossil_hir::lower::lower_to_hir`,
// `fossil_hir::body::body`, etc.) to mirror the rust-analyzer convention and
// avoid name shadowing with the modules themselves.
pub use ast_id::{AstIdEntry, AstIdMap, FileAstId, MappingNode, SourceDefNode};
pub use body::{ExprId, HirBody};
pub use def_map::{DefMap, MappingLoc, SourceLoc};
pub use item_tree::{ItemHeader, ItemTree, MappingHeader};
pub use lower::{HirExpr, HirFile, HirMapping, HirProperty, PropertyKey};
pub use ty::{FnSig, InferenceId, Primitive, Record, RecordField, ShapeId, Ty, TyKind};
