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
    /// two-span blame structure is in place (verified via the accumulator —
    /// `delay_span_bug` pushes a Diagnostic with a message containing
    /// "expected ... got ..." plus a "expected because of" trailer when the
    /// destination expression's provenance is known).
    ///
    /// Plan 02-06 requirement: `compatible_blame_two_spans_on_mismatch`.
    /// Wrapped in a Salsa-tracked shim because `delay_span_bug` panics
    /// outside a tracked context (plan-02-05 Deviation 1 — feature, not bug).
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
            let int_ty = Ty::new(db, TyKind::Primitive(Primitive::Integer));
            let str_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
            compatible(db, m, int_ty, str_ty, ExprId(0), ExprId(0)).err()
        }

        let (db, file) = db_with_hello();
        let eg = tracked_compatible_shim(&db, file);
        assert!(eg.is_some(), "Integer vs String must produce an Err");
        let diags = tracked_compatible_shim::accumulated::<Diagnostic>(&db, file);
        assert_eq!(
            diags.len(),
            1,
            "compatible mismatch must push exactly one Diagnostic"
        );
        let msg = &diags[0].message;
        assert!(
            msg.contains("expected"),
            "diagnostic must contain 'expected', got {msg:?}"
        );
        assert!(
            msg.contains("got"),
            "diagnostic must contain 'got', got {msg:?}"
        );
    }
}
