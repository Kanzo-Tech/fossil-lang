//! Type provenance side table (CORE-11) + `expr_types` / `ty_origin` queries.
//!
//! Provenance is recorded in a SEPARATE Salsa
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
//!   Phase 2 literal subset (`StringLit` / `Template`);
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
//! # Phase 3 plan 03-04: real spans land
//!
//! Phase 2 shipped zero-width `Span { start: 0, end: 0 }` placeholders for
//! every literal-subset entry — the blame STRUCTURE was in place but the
//! byte ranges weren't useful for diagnostics. Phase 3 plan 03-04 lands
//! the [`crate::spans::Spans`] side table and this module now
//! populates [`Provenance::span`] from real `rowan::TextRange`s read via
//! [`crate::spans::spans`]. The Phase 2 limitation comment that previously
//! lived here is discharged.

use fossil_base::Span;
use smol_str::SmolStr;

use crate::body::ExprId;
use crate::def_map::{MappingLoc, def_map};
use crate::ty::Ty;

#[cfg(test)]
use crate::ty::TyKind;

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
/// Covers the seven Phase 2 provenance channels. Phase 3
/// may refactor `InputDescriptor` / `OutputDescriptor` to carry interned ids
/// instead of [`SmolStr`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ProvenanceKind {
    /// Type came from an input source descriptor (e.g. a CSVW column).
    InputDescriptor {
        source_name: SmolStr,
        column: SmolStr,
    },
    /// Type came from the output shape document (e.g. a property
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
    /// Type was synthesised inside an IMPLICIT closure body (CORE-07,
    /// type-system.md §7). The closure was implicitly created because the
    /// surrounding function-arg position expected `Fn(Record<R> -> τ)` and the
    /// arg expression contains free `.field` references (Fossil has no surface
    /// lambda syntax — implicit closure synthesis is the ONLY lambda form).
    ///
    /// `rendering` is the displayable form of the closure, e.g.
    /// `(row: Record<{id: String, name: String, age: Integer}>) => row.age >= 18`.
    /// LSP hover (plan 03-07) renders this above the field type so the synthesis
    /// is NEVER hidden from the user.
    ///
    /// CRITICAL (Risk Register): `rendering` MUST NEVER contain the substring
    /// `Unknown` or `InferenceId` — it is built via [`crate::render_ty_kind`]
    /// (the shared `TyDisplay`), never raw `{:?}` Debug. The internal
    /// `TyKind::Unknown(InferenceId)` synthesis-state placeholder normalises to
    /// `?` at the display boundary.
    SynthesizedClosureRendering { rendering: SmolStr },
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

/// Per-mapping type table — Phase 3 thin accessor over [`typecheck_mapping`].
///
/// Phase 3 (plan 03-05) INVERTS the Phase 2 dependency direction: the
/// bidirectional checker's [`crate::check::typecheck_mapping`] is now the
/// SOURCE OF TRUTH for per-expression types (over the full `HirExpr` space,
/// including `FieldRef` resolved against the CSVW source row). `expr_types` is
/// the projection — it returns `typecheck_mapping(db, mapping)?.expr_types(db)`,
/// or an empty table if the mapping had a type error (the error already
/// emitted ≥1 diagnostic).
///
/// Phase 2's literal-subset behaviour is preserved as a strict widening: a
/// `Template` RHS still synthesises `IriTemplate`, a `StringLit` still
/// synthesises `String`, etc. — the new path additionally resolves `FieldRef`
/// when a CSVW schema is declared (otherwise `FieldRef` synthesises no entry,
/// matching Phase 2).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn expr_types<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> ExprTypes<'db> {
    match crate::check::typecheck_mapping(db, mapping) {
        Ok(out) => out.expr_types(db),
        Err(_eg) => ExprTypes::new(db, Vec::new()),
    }
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
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
    name = User.name
";

    fn db_with_text(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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

    /// Property 1 of `hello.fossil` is `name = .name` — the RHS is a
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

    // `ty_origin_returns_iri_for_iri_literal_in_property` lived here, and with
    // it the two `#[cfg(test)]` helpers it was the only caller of. It built a
    // `HirExpr::PrefixedName` by hand because the lowering of `IDENT SHAPE_SEP
    // IDENT` returned `None` — a limitation its own doc-comment called
    // pre-existing and tracked elsewhere. It was not tracked: it was decided.
    // The CURIE is gone, so the variant is gone, so the only test that could
    // reach it was a test of a form the language does not have.

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
