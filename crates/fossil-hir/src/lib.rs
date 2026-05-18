//! `fossil-hir` — types + name resolution + minimal type check.
//!
//! Per ADR-0002 this crate collapses what was originally three crates
//! (`fossil-types + fossil-resolve + fossil-typeck`) into one, matching the
//! `ty_python_semantic` pattern from the research synthesis.
//!
//! # Phase 1 scope (this commit)
//!
//! - 5/11-kind [`Ty`] ADT (`Primitive`, `Iri`, `IriTemplate`, `Record`, `Error`).
//! - [`DefMap`] = prefix table + source bindings + mapping list, populated by
//!   the [`def_map`] Salsa query.
//! - Interned [`MappingLoc`]/[`SourceLoc`] location IDs (rust-analyzer pattern,
//!   per RESEARCH.md §"Architecture Patterns" Pattern 2 + 3).
//! - [`item_tree`] placeholder query (Phase 2 makes it stable across body
//!   edits per CORE-02).
//! - [`lower_to_hir`] for the canonical Phase 1 example (one mapping with
//!   header + 2 properties).
//! - [`check::typecheck_mapping`] no-op stub returning `Ok(())`.
//!
//! # Phase 2-9 contract (locked)
//!
//! Public Salsa query signatures (`def_map`, `item_tree`, `lower_to_hir`,
//! `typecheck_mapping`) and the public types in [`ty`], [`def_map`],
//! [`item_tree`], and [`lower`] are stable for downstream phases. Phase 2
//! expands grammar productions + `ItemTree` stability (CORE-02) + 11-kind Ty
//! (CORE-03). Phase 3 wires bidirectional checking + `ErrorGuaranteed`
//! propagation (CORE-04..07).

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
pub use check::ErrorMarker;
pub use def_map::{DefMap, MappingLoc, SourceLoc};
pub use item_tree::{ItemHeader, ItemTree, MappingHeader};
pub use lower::{HirExpr, HirFile, HirMapping, HirProperty, PropertyKey};
pub use ty::{Primitive, Record, Ty, TyKind};
