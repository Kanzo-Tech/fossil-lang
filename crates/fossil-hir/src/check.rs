//! Phase 2 type-check stub + [`compatible`] two-span blame stub.
//!
//! Phase 1's local taint-wrapper newtype is REMOVED per plan 02-05 /
//! RESEARCH.md §Q6 — [`fossil_base::ErrorGuaranteed`] now provides
//! `unsafe impl salsa::Update` directly (at the third-party-trait
//! integration boundary per ADR-0004), so the local wrapper has no
//! purpose. `Ty::Error` carries `ErrorGuaranteed` directly.
//!
//! Phase 3 (CORE-04..07) plumbs the real bidirectional checking with forward
//! propagation from CSVW and backward checking against `ShEx`. Phase 2 ships
//! the stubbed query signature returning `Result<(), ErrorGuaranteed>` plus
//! the [`compatible`] stub used to land the two-span blame STRUCTURE end-to-
//! end (per plan 02-06 — destructures [`crate::provenance::ExprTypeEntry`]
//! per checker Blocker 5, not a `(Ty, Provenance)` tuple).

use fossil_base::{ErrorGuaranteed, Span, delay_span_bug};

use crate::body::ExprId;
use crate::def_map::MappingLoc;
use crate::provenance::{ExprTypeEntry, ty_origin};
use crate::ty::Ty;

#[salsa::tracked]
#[allow(clippy::unnecessary_wraps)] // Phase 3 returns Err on real type errors.
pub fn typecheck_mapping<'db>(
    _db: &'db dyn fossil_base::Db,
    _mapping: MappingLoc<'db>,
) -> Result<(), ErrorGuaranteed> {
    // Phase 2: trivially OK. Phase 3 (CORE-04..07) wires real checking with
    // forward propagation from CSVW and backward checking against `ShEx`.
    Ok(())
}

/// Compatibility check between two types in a checker context.
///
/// Phase 2 stub: equality of interned Salsa ids (structural equality at the
/// type layer becomes pointer equality after interning). Phase 3 expands to
/// real subtyping + cardinality + facet checking.
///
/// On mismatch this function MUST emit a two-span diagnostic message that
/// names BOTH the source expression's origin span (where the actual type
/// was synthesised) AND the destination expression's origin span (where the
/// expected type was required). This is the rustc-style two-span blame
/// pattern. Phase 2 spans are zero-width pending the lowering arena
/// (RESEARCH.md §Q5); the BLAME STRUCTURE is what plan 02-06 lands.
///
/// The lookup goes through [`ty_origin`], which returns
/// `Option<ExprTypeEntry<'db>>` (NOT a tuple — see planner checker Blocker 5
/// and the [`crate::provenance`] module docs). We destructure
/// `entry.ty` / `entry.provenance` directly.
pub fn compatible<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    actual: Ty<'db>,
    expected: Ty<'db>,
    source_expr: ExprId,
    dest_expr: ExprId,
) -> Result<(), ErrorGuaranteed> {
    if actual == expected {
        return Ok(());
    }
    // Per checker Blocker 5: ty_origin returns Option<ExprTypeEntry<'db>>.
    let source_entry: Option<ExprTypeEntry<'db>> = ty_origin(db, mapping, source_expr);
    let dest_entry: Option<ExprTypeEntry<'db>> = ty_origin(db, mapping, dest_expr);
    let source_span = source_entry
        .as_ref()
        .map_or(Span { start: 0, end: 0 }, |e| e.provenance.span);
    let dest_trailer = dest_entry.as_ref().map_or(String::new(), |e| {
        format!(
            " (expected because of {:?} at {:?})",
            e.provenance.kind, e.provenance.span
        )
    });
    let msg = format!(
        "expected {:?}, got {:?}{}",
        expected.kind(db),
        actual.kind(db),
        dest_trailer,
    );
    Err(delay_span_bug(db, source_span, msg))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::def_map::def_map;
    use crate::ty::{Primitive, TyKind};
    use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
    use std::sync::Arc;

    const HELLO: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    fn db_with_hello() -> (FossilDb, SourceFile) {
        let system: Arc<dyn System> = Arc::new(NativeSystem);
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
        (db, file)
    }

    /// `compatible` returns `Ok(())` when actual == expected (same interned
    /// `Ty<'db>` id).
    #[test]
    fn compatible_returns_ok_for_same_type() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let m = *dm.mappings(&db).first().expect("hello has one mapping");
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let result = compatible(&db, m, int_ty, int_ty, ExprId(0), ExprId(0));
        assert!(result.is_ok());
    }

    /// `compatible` returns `Err(ErrorGuaranteed)` on type mismatch AND the
    /// two-span blame surfaces REAL byte ranges (per Phase 3 plan 03-04 /
    /// ADR-0008 — the [`crate::spans::Spans`] side table replaces the
    /// Phase 2 zero-width placeholders).
    ///
    /// Originally landed in Phase 2 plan 02-06 with zero-width spans
    /// (the blame STRUCTURE was the deliverable). Plan 03-04 upgrades
    /// the assertions to require non-zero source and destination spans
    /// — `compatible()` itself is unchanged; the upgrade is invisible
    /// to the checker because it goes through `ty_origin(...)`'s
    /// provenance, which now reads from the new `spans()` Salsa query.
    ///
    /// Wrapped in a Salsa-tracked shim because `delay_span_bug` panics
    /// outside a tracked context (plan-02-05 Deviation 1 — feature, not
    /// bug).
    ///
    /// Fixture orientation: property 0 of the `hello.fossil` mapping is
    /// a Template RHS (`iri = backtick-template`). Plan 03-04 ensures
    /// that this synthesises `IriTemplate` with `Literal` provenance and
    /// a real non-zero span. Passing expected=String against actual=
    /// IriTemplate (same type provenance synthesises for prop 0)
    /// triggers the mismatch path against a known-real-span source.
    #[test]
    fn compatible_blame_two_spans_on_mismatch() {
        #[salsa::tracked]
        #[allow(clippy::elidable_lifetime_names, clippy::needless_lifetimes)]
        fn tracked_compatible_shim<'db>(
            db: &'db dyn fossil_base::Db,
            file: SourceFile,
        ) -> Option<ErrorGuaranteed> {
            let dm = def_map(db, file);
            let m = *dm.mappings(db).first()?;
            // Use the same type provenance synthesises for property 0
            // (Template → IriTemplate) so the source-span lookup goes
            // through ty_origin → spans(M).get(ExprId(0)) → real span.
            let iri_template = Ty::new(db, TyKind::IriTemplate);
            let str_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
            compatible(db, m, iri_template, str_ty, ExprId(0), ExprId(0)).err()
        }

        let (db, file) = db_with_hello();
        let eg = tracked_compatible_shim(&db, file);
        assert!(eg.is_some(), "IriTemplate vs String must produce an Err");
        let diags = tracked_compatible_shim::accumulated::<Diagnostic>(&db, file);
        assert_eq!(
            diags.len(),
            1,
            "compatible mismatch must push exactly one Diagnostic"
        );
        let diag = &diags[0];
        let msg = &diag.message;
        assert!(
            msg.contains("expected"),
            "diagnostic must contain 'expected', got {msg:?}"
        );
        assert!(
            msg.contains("got"),
            "diagnostic must contain 'got', got {msg:?}"
        );

        // Phase 3 plan 03-04 upgrade — REAL source span:
        // `Diagnostic.span` is singular (NOT a Vec); `delay_span_bug`
        // takes one `Span` argument and the two-span blame is encoded
        // by embedding the destination span in the message text. The
        // source span MUST be a real non-zero byte range from
        // `spans(M).get(ExprId(0))` — NOT the Phase 2 zero-width
        // placeholder.
        let source_span = diag.span;
        assert!(
            source_span.end > source_span.start,
            "Phase 3 plan 03-04 requires REAL non-zero source spans on \
             compatible() blame; got {source_span:?} (would have been \
             Span {{ start: 0, end: 0 }} in Phase 2)."
        );

        // Phase 3 plan 03-04 upgrade — REAL dest span in the trailer:
        // The message embeds `(expected because of <kind> at <span>)`,
        // and the dest provenance span renders via `Debug` as
        // `Span { start: N, end: M }`. For dest_expr = ExprId(0), the
        // same real range applies — so `start: 0, end: 0` must NOT
        // appear.
        assert!(
            !msg.contains("start: 0, end: 0"),
            "Phase 3 plan 03-04 requires REAL non-zero dest spans in \
             the blame trailer; found zero-width span in: {msg:?}"
        );
    }
}
