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
//! - [`check::compatible`] stub demonstrating Phase 3's two-span blame
//!   pattern (plan 02-06) — pointer-equality only for Phase 2; Phase 3
//!   wires real subtyping + facets.
//! - [`provenance`] side table: `(MappingLoc, ExprId) -> ExprTypeEntry`
//!   per RESEARCH.md §Q5; [`provenance::expr_types`] populates the Phase 2
//!   literal subset (`StringLit` / `Template` / `PrefixedName`); [`provenance::ty_origin`]
//!   is the user-facing lookup returning `Option<ExprTypeEntry>` (per
//!   planner checker Blocker 5 — tuples don't auto-impl `salsa::Update`).
//! - [`spans`] side table: per-mapping real-span lookup
//!   (`(MappingLoc, ExprId) -> Span`) populated from `rowan::TextRange`s at
//!   query time. Replaces Phase 2's zero-width `Span { start: 0, end: 0 }`
//!   placeholders for the literal-subset provenance entries. The
//!   [`spans::spans`] tracked query reads `mapping_cst_node` (NOT
//!   `parse(file)`) to preserve the Phase 2 plan 02-07
//!   `MAX_PER_MAPPING_FAN_OUT = 1` invariant. ADR-0008 records the
//!   side-table-over-`HirExpr`-field choice (same rationale as Phase 2
//!   RESEARCH §Q5 for provenance).
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
pub mod didyoumean;
pub mod infer;
pub mod item_tree;
pub mod lower;
pub mod provenance;
pub mod shapes;
pub mod spans;
/// The stdlib catalog: every function the language declares, its signature and
/// how it compiles. It lives here because the checker resolves a call against
/// it, and the checker cannot depend on a crate that depends on the checker —
/// which is what `fossil-registry` was until ADR-0048.
pub mod stdlib;
pub mod ty;

// Type re-exports only; the query functions are intentionally kept under their
// module paths (`fossil_hir::def_map::def_map`, `fossil_hir::lower::lower_to_hir`,
// `fossil_hir::body::body`, etc.) to mirror the rust-analyzer convention and
// avoid name shadowing with the modules themselves.
pub use ast_id::{AstIdEntry, AstIdMap, FileAstId, MappingNode, SourceDefNode};
pub use body::{ExprId, HirBody};
pub use def_map::{DefMap, MappingLoc, SourceLoc};
pub use item_tree::{ItemHeader, ItemTree, MappingHeader};
pub use lower::{CmpOp, HirExpr, HirFile, HirMapping, HirProperty, PropertyKey};
pub use provenance::{
    ExprTypeEntry, ExprTypes, Provenance, ProvenanceKind, expr_types, mapping_at, ty_origin,
};
// The `spans()` query function is reachable as `fossil_hir::spans::spans`
// (the module path is intentional — the bare `spans` name would shadow the
// module). Mirrors the rust-analyzer convention of keeping Salsa query
// functions under their module paths.
pub use check::{BlamePos, Checker, TypeckOutput, compatible, typecheck_mapping};
pub use didyoumean::did_you_mean;
pub use spans::Spans;
pub use ty::display::render_ty_kind;
pub use ty::{FnSig, InferenceId, Record, RecordField, ShapeId, Ty, TyKind};
