//! Type provenance side table (CORE-11) + `expr_types` / `ty_origin` queries.
//!
//! Per Phase 2 RESEARCH.md §Q5: provenance is recorded in a SEPARATE Salsa
//! tracked struct keyed by `(MappingLoc, ExprId)`, NOT baked into [`Ty<'db>`].
//! Baking provenance into `Ty` would defeat structural interning (one entry per
//! AST position instead of one per shape; ~50-100× cardinality blow-up for a
//! 200-line fixture). The side-table approach preserves "structural equality →
//! pointer equality" at the type layer while still recording per-expression
//! origin spans for Phase 3's two-span diagnostic blame.
//!
//! # Public surface
//!
//! - [`Provenance`] + [`ProvenanceKind`]: where a synthesised type came from.
//! - [`ExprTypeEntry`]: `(expr_id, ty, provenance)` triple.
//! - [`ExprTypes`]: per-mapping interned vector of [`ExprTypeEntry`].
//! - [`expr_types`]: `MappingLoc -> ExprTypes` Salsa query, populates the
//!   Phase 2 literal subset (`StringLit` / `Template` / `PrefixedName`);
//!   `FieldRef` returns None (deferred to Phase 3 — needs source-row type
//!   from CSVW).
//! - [`ty_origin`]: convenience lookup `(MappingLoc, ExprId) -> Option<ExprTypeEntry>`.
//!
//! # Why `Option<ExprTypeEntry<'db>>` and not `Option<(Ty, Provenance)>`
//!
//! Salsa 0.26 requires every type appearing as a `#[salsa::tracked]` function's
//! return implement [`salsa::Update`]. Tuples like `(Ty<'db>, Provenance)` do
//! NOT automatically satisfy this bound. Reusing the existing
//! [`ExprTypeEntry`] struct (which derives [`salsa::Update`]) cleanly
//! satisfies the bound — and downstream consumers (the hover handler in
//! `fossil-ide` + the [`crate::check::compatible`] stub) destructure
//! `ExprTypeEntry { ty, provenance, .. }` cleanly. (Per planner checker
//! Blocker 5.)
//!
//! # Phase 2 simplification: provenance is `'db`-free
//!
//! [`ProvenanceKind`] uses [`smol_str::SmolStr`] for descriptor variants
//! (source name, shape IRI text, property IRI text) instead of `'db`-interned
//! ids. Phase 3 may refactor to interned ids once `OutputDescriptor` /
//! `InputDescriptor` resolution exists. This keeps Phase 2 provenance simple
//! and avoids threading `'db` through [`crate::body::HirBody`] →
//! [`ExprTypes`] → [`Provenance`].
//!
//! # Phase 2 limitation: zero-width spans
//!
//! [`Provenance::span`] is currently `Span { start: 0, end: 0 }` for every
//! literal-subset entry. Phase 3's lowering arena will plumb real source spans
//! through to the expression nodes. The LSP hover still works (Markdown body
//! shows the rendered `Ty` + a best-effort provenance description); the
//! [`crate::check::compatible`] two-span blame currently surfaces both spans
//! as zero-width but the blame STRUCTURE is in place for Phase 3 to populate.

use fossil_base::Span;
use smol_str::SmolStr;

use crate::body::{ExprId, body};
use crate::def_map::{MappingLoc, def_map};
use crate::lower::HirExpr;
use crate::ty::{Primitive, Ty, TyKind};

/// Where a synthesised [`Ty`] came from. Carries a source [`Span`] (where the
/// type was synthesised) + a categorical [`ProvenanceKind`] (semantic reason).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Provenance {
    /// Source span the type was synthesised from. Phase 2 ships zero-width
    /// spans (`Span { start: 0, end: 0 }`); Phase 3 populates real ranges via
    /// the lowering arena.
    pub span: Span,
    /// Categorical origin — discriminates "from an input descriptor field"
    /// from "synthesised by an operator" from "literal" etc.
    pub kind: ProvenanceKind,
}

/// Categorical type-origin reason.
///
/// Covers the seven Phase 2 provenance channels per RESEARCH.md §Q5. Phase 3
/// may refactor `InputDescriptor` / `OutputDescriptor` to carry interned ids
/// instead of [`SmolStr`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ProvenanceKind {
    /// Type came from an input source descriptor (e.g. a CSVW column).
    InputDescriptor {
        source_name: SmolStr,
        column: SmolStr,
    },
    /// Type came from an output shape descriptor (e.g. a `ShEx` property
    /// shape).
    OutputDescriptor {
        shape_iri: SmolStr,
        property_iri: SmolStr,
    },
    /// Type is the inferred result of a literal expression (string, IRI,
    /// template).
    Literal,
    /// Type is the return type of a function call (e.g. `string.upper(...)`).
    FnResult { name: SmolStr },
    /// Type is the result of a binary operator (e.g. `a ++ b`).
    BinaryOp { op: SmolStr },
    /// Type was synthesised for an anonymous closure parameter (e.g. inside
    /// `map(\x -> ...)`).
    SynthesizedClosureParam,
    /// Type came from the LHS of a pipeline (`x |> f` — `x`'s type flows into
    /// `f`'s expected param).
    PipelineLhs,
}

/// Per-mapping interned table of `(expr_id, ty, provenance)` triples.
///
/// Salsa-tracked so structural-equality re-derives memoise. Only Phase 2
/// literal-subset expressions populate entries; FieldRef and other non-
/// literal forms are absent (their `ty_origin` returns `None`).
#[salsa::tracked(debug)]
pub struct ExprTypes<'db> {
    #[returns(ref)]
    pub entries: Vec<ExprTypeEntry<'db>>,
}

/// One row of [`ExprTypes`]: the expression id, its inferred type, and the
/// reason / span the type came from.
///
/// Returned from [`ty_origin`] as `Option<ExprTypeEntry<'db>>` (see module
/// docs — Salsa Update bound on tuples is the why).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExprTypeEntry<'db> {
    pub expr_id: ExprId,
    pub ty: Ty<'db>,
    pub provenance: Provenance,
}

/// Per-mapping type inference for the Phase 2 literal subset.
///
/// Iterates the mapping's [`crate::body::HirBody::properties`]; for each
/// property whose RHS [`HirExpr`] has a literal-synthesisable type
/// (`StringLit` / `Template` / `PrefixedName`), pushes an [`ExprTypeEntry`].
/// `FieldRef` is absent — its type depends on the source row schema, which
/// is Phase 3 (forward CSVW propagation).
///
/// The `ExprId` for property `i` is `ExprId(i as u32)` — matches the
/// `expr_count` advancement in [`crate::body::body`].
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn expr_types<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> ExprTypes<'db> {
    let hir_body = body(db, mapping);
    let properties = hir_body.properties(db);
    let mut entries: Vec<ExprTypeEntry<'db>> = Vec::new();
    for (i, prop) in properties.iter().enumerate() {
        let expr_id = ExprId(u32::try_from(i).unwrap_or(u32::MAX));
        if let Some((ty, provenance)) = infer_literal_type(db, &prop.value) {
            entries.push(ExprTypeEntry {
                expr_id,
                ty,
                provenance,
            });
        }
    }
    ExprTypes::new(db, entries)
}

/// Lookup helper: `(MappingLoc, ExprId) -> Option<ExprTypeEntry>`.
///
/// Returns `Option<ExprTypeEntry<'db>>` (NOT a tuple) to satisfy Salsa 0.26's
/// `salsa::Update` bound on tracked-function return types. See the module-
/// level "Why `Option<ExprTypeEntry<'db>>`" section. Downstream consumers
/// destructure `entry.ty` and `entry.provenance` directly.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn ty_origin<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    expr_id: ExprId,
) -> Option<ExprTypeEntry<'db>> {
    let et = expr_types(db, mapping);
    et.entries(db)
        .iter()
        .find(|e| e.expr_id == expr_id)
        .cloned()
}

/// Phase 2 literal-subset type synthesis.
///
/// - `StringLit` → `Primitive(String)` with [`ProvenanceKind::Literal`].
/// - `Template` → `IriTemplate` with [`ProvenanceKind::Literal`] (Phase 2
///   tags every template as iri-context; Phase 4 lifts template parsing into
///   a real expression tree and can distinguish iri vs. string-template
///   contexts).
/// - `PrefixedName` → `Iri` with [`ProvenanceKind::Literal`].
/// - `FieldRef` → `None` — needs source-row type from `InputDescriptor`
///   (Phase 3 / RESEARCH.md §Q5).
///
/// Spans are zero-width pending Phase 3's lowering arena.
fn infer_literal_type<'db>(
    db: &'db dyn fossil_base::Db,
    expr: &HirExpr,
) -> Option<(Ty<'db>, Provenance)> {
    let zero_span = Span { start: 0, end: 0 };
    let lit_provenance = Provenance {
        span: zero_span,
        kind: ProvenanceKind::Literal,
    };
    match expr {
        HirExpr::StringLit(_) => Some((
            Ty::new(db, TyKind::Primitive(Primitive::String)),
            lit_provenance,
        )),
        HirExpr::Template(_) => Some((Ty::new(db, TyKind::IriTemplate), lit_provenance)),
        HirExpr::PrefixedName { .. } => Some((Ty::new(db, TyKind::Iri), lit_provenance)),
        HirExpr::FieldRef(_) => None,
    }
}

/// Look up the [`MappingLoc`] for the `n`th MAPPING in a file.
///
/// Used by the hover handler in `fossil-ide` to bridge CST position →
/// `MappingLoc` for the [`ty_origin`] query. Public-but-crate so tests in
/// this module can reuse it.
#[must_use]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn mapping_at<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    index: usize,
) -> Option<MappingLoc<'db>> {
    let dm = def_map(db, file);
    dm.mappings(db).get(index).copied()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    fn db_with_text(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "test.fossil".to_string());
        (db, file)
    }

    /// Property 0 of `hello.fossil` is the `iri = ...` template — a
    /// `Template` RHS. Phase 2 synthesises `IriTemplate` with `Literal`
    /// provenance.
    #[test]
    fn expr_types_returns_iri_template_for_iri_property() {
        let (db, file) = db_with_text(HELLO);
        let m = mapping_at(&db, file, 0).expect("hello has one mapping");
        let entry =
            ty_origin(&db, m, ExprId(0)).expect("property 0 (iri = template) must have an entry");
        assert_eq!(entry.ty.kind(&db), &TyKind::IriTemplate);
        assert_eq!(entry.provenance.kind, ProvenanceKind::Literal);
        assert_eq!(entry.expr_id, ExprId(0));
    }

    /// Property 1 of `hello.fossil` is `ex:name = .name` — the RHS is a
    /// `FieldRef`. Phase 2 returns `None` (deferred to Phase 3 — needs
    /// source-row type).
    #[test]
    fn ty_origin_returns_none_for_field_ref() {
        let (db, file) = db_with_text(HELLO);
        let m = mapping_at(&db, file, 0).expect("hello has one mapping");
        assert!(
            ty_origin(&db, m, ExprId(1)).is_none(),
            "FieldRef RHS must not synthesise a type in Phase 2 (deferred to Phase 3)"
        );
    }

    /// `HirExpr::PrefixedName` synthesises [`TyKind::Iri`] with `Literal`
    /// provenance. Tested at the [`infer_literal_type`] helper layer (rather
    /// than via the end-to-end Salsa query) because the Phase 1 lowering of
    /// `EXPR > IRI_EXPR > IDENT SHAPE_SEP IDENT` returns `None` (the
    /// `IRI_EXPR` branch of `lower_expr` currently expects an `ABS_IRI` token; the
    /// prefixed-name form is a pre-existing limitation tracked separately and
    /// outside the plan-02-06 scope boundary). Synthesising the `HirExpr`
    /// directly tests the type-inference layer regardless.
    #[test]
    fn ty_origin_returns_iri_for_iri_literal_in_property() {
        let (db, _file) = db_with_text(HELLO);
        let expr = HirExpr::PrefixedName {
            iri: smol_str::SmolStr::from("https://example.org/Foo"),
        };
        let (ty, prov) =
            infer_literal_type(&db, &expr).expect("PrefixedName must synthesise an Iri type");
        assert_eq!(ty, Ty::new(&db, TyKind::Iri));
        assert_eq!(prov.kind, ProvenanceKind::Literal);
    }

    /// Compile-time confirmation per planner checker Blocker 5: `ty_origin`
    /// returns `Option<ExprTypeEntry<'_>>` (NOT `Option<(Ty, Provenance)>`).
    /// The destructure pattern compiles iff the return type is the struct.
    #[test]
    fn ty_origin_returns_expr_type_entry_struct() {
        let (db, file) = db_with_text(HELLO);
        let m = mapping_at(&db, file, 0).expect("hello has one mapping");
        // The point of the test: this destructure compiles. If `ty_origin`
        // ever regressed to `Option<(Ty, Provenance)>` the pattern would
        // fail to type-check.
        if let Some(ExprTypeEntry {
            ty: _,
            provenance: _,
            expr_id: _,
        }) = ty_origin(&db, m, ExprId(0))
        {
            // Reachable for property 0 (Template → IriTemplate).
        }
    }

    /// Memoisation sanity check — `ExprTypes` is salsa-tracked, so repeated
    /// invocations on the same `MappingLoc` yield the same handle.
    #[test]
    fn expr_types_is_memoised_per_mapping() {
        let (db, file) = db_with_text(HELLO);
        let m = mapping_at(&db, file, 0).expect("hello has one mapping");
        let a = expr_types(&db, m);
        let b = expr_types(&db, m);
        assert_eq!(a, b);
    }
}
