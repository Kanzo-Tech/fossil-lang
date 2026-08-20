//! `fossil-hir` — types + name resolution + minimal type check.
//!
//! This crate collapses what was originally three crates
//! (`fossil-types + fossil-resolve + fossil-typeck`) into one, matching the
//! `ty_python_semantic` pattern.
//!
//! # What lives here
//!
//! - The [`Ty`] ADT (`Primitive`, `Seq`, `Record`, `Iri`, `IriTemplate`,
//!   `Error`, `Unknown`). `Optional` and `Fn` were
//!   two more and neither was ever constructed outside a test.
//! - [`DefMap`] = prefix table + source bindings + mapping list, populated by
//!   the [`def_map`] Salsa query.
//! - Interned [`MappingLoc`]/[`SourceLoc`] location IDs (the rust-analyzer
//!   pattern).
//! - [`item_tree`] signature-only query + [`body`] per-mapping body query
//!   (rust-analyzer invalidation-barrier pattern): the signature
//!   query is what everything else depends on, so editing one mapping's body
//!   re-runs that mapping and nothing else.
//! - [`lower_to_hir`] for header-only `HirFile` lowering; body content lives
//!   behind [`body`] (`body(db, MappingLoc) -> HirBody`).
//! - [`check::typecheck_mapping`], the ONE tracked checker entry per mapping.
//! - [`check::compatible`], the two-span blame pattern: real subtyping plus
//!   cardinality facets, blaming the constraint and the expression separately.
//! - [`provenance`] side table: `(MappingLoc, ExprId) -> ExprTypeEntry`.
//!   [`provenance::expr_types`] projects [`check::typecheck_mapping`]'s
//!   per-expression types; [`provenance::ty_origin`]
//!   is the user-facing lookup returning `Option<ExprTypeEntry>` — a struct
//!   and not a tuple, because tuples don't auto-impl `salsa::Update`.
//! - [`spans`] side table: per-mapping real-span lookup
//!   (`(MappingLoc, ExprId) -> Span`) populated from `rowan::TextRange`s at
//!   query time. The
//!   [`spans::spans`] tracked query reads `mapping_cst_node` (NOT
//!   `parse(file)`) to preserve the
//!   `MAX_PER_MAPPING_FAN_OUT = 1` invariant. The spans live in a side table
//!   rather than in a `span` field on every `HirExpr` so that lowering stays
//!   span-free and only the layers that emit diagnostics pay for them (same
//!   rationale as the provenance side table).
//!
//! # The locked surface
//!
//! Public Salsa query signatures (`def_map`, `item_tree`, `lower_to_hir`,
//! `body`, `typecheck_mapping`) and the public types in [`ty`], [`def_map`],
//! [`item_tree`], [`body`], and [`lower`] are stable for every crate
//! downstream: bidirectional checking and [`fossil_base::ErrorGuaranteed`]
//! propagation are built on top of them, not beside them.

pub mod ast_id;
pub mod body;
pub mod check;
pub mod def_map;
pub mod didyoumean;
pub mod documents;
/// The identity of a TYPE: one `@subject` template per shape, file-keyed.
///
/// The identity is unique per type — one `@subject` per shape, declared by the
/// program — and that is what turns an edge from a guess into a lookup. `crate::body`'s identity checks are keyed by
/// `MappingLoc` and so cannot see a second mapping; this is the file-level table
/// they cannot hold.
pub mod identity;
pub mod infer;
pub mod item_tree;
pub mod lower;
pub mod provenance;
/// What the compiler says when a program names a provider it cannot use. Three
/// sentences, shared by every path that raises one — this crate's checker and
/// `fossil-engine`'s run path. They were methods on `fossil_base::Provider`,
/// which put English Fossil compiler errors in the trait-and-db substrate.
pub mod refusals;
pub mod shapes;
pub mod spans;
/// The stdlib catalog: every function the language declares, its signature and
/// how it compiles. It lives here because the checker resolves a call against
/// it, and the checker cannot depend on a crate that depends on the checker —
/// which is what `fossil-registry` was until it was folded in here.
pub mod stdlib;
// The host-with-a-decoder this crate's tests use was a private `test_support`
// module here. It moved to `fossil_base::test_support` (feature
// `test-support`, a dev-dependency below) because `#[cfg(test)]` made it
// unreachable from any other crate's integration tests, and `fossil-mir` and
// `fossil-hir/tests/diagnostic_corpus.rs` both need the same one. The reason it
// is not the real ShEx decoder is unchanged: this crate names no schema
// language, and reaching for one as a dev-dependency puts it back.
pub mod ty;

// Type re-exports only; the query functions are intentionally kept under their
// module paths (`fossil_hir::def_map::def_map`, `fossil_hir::lower::lower_to_hir`,
// `fossil_hir::body::body`, etc.) to mirror the rust-analyzer convention and
// avoid name shadowing with the modules themselves.
pub use ast_id::{AstIdEntry, AstIdMap, FileAstId, MappingNode, SourceDefNode};
pub use body::{ExprId, HirBody};
pub use def_map::{DefMap, MappingLoc, SourceLoc};
pub use item_tree::{ItemHeader, ItemTree, MappingHeader};
pub use lower::{BinOp, FloatBits, HirExpr, HirFile, HirMapping, HirProperty, PropertyKey, UnOp};
pub use provenance::{
    ExprTypeEntry, ExprTypes, Provenance, ProvenanceKind, expr_types, mapping_at, ty_origin,
};
// The `spans()` query function is reachable as `fossil_hir::spans::spans`
// (the module path is intentional — the bare `spans` name would shadow the
// module). Mirrors the rust-analyzer convention of keeping Salsa query
// functions under their module paths.
pub use check::{
    BlamePos, Checker, TypeckOutput, compatible, render_split_suggestion, typecheck_mapping,
};
pub use didyoumean::did_you_mean;
pub use spans::Spans;
pub use ty::display::render_ty_kind;
pub use ty::{FnSig, InferenceId, Record, RecordField, ShapeId, Ty, TyKind};
