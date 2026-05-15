//! Phase 1 type-check stub.
//!
//! Phase 3 (CORE-04) plumbs `ErrorGuaranteed` propagation and bidirectional
//! checking with forward CSVW + backward `ShEx`. This module owns the
//! [`ErrorMarker`] newtype around `fossil_base::ErrorGuaranteed` so that
//! `Ty::Error(ErrorMarker)` (defined in [`crate::ty`]) does not leak the
//! taint type through the public Ty surface yet — Phase 3 will widen it.

use fossil_base::ErrorGuaranteed;

/// Local newtype around [`fossil_base::ErrorGuaranteed`] used by
/// [`crate::ty::TyKind::Error`] to keep the public Ty surface narrow until
/// Phase 3 wires real diagnostic emission.
///
/// Kept `Copy + Eq + Hash` so that types containing it derive [`salsa::Update`]
/// without bespoke impls. Phase 1 has no path that constructs an
/// `ErrorMarker` (the `Error` variant exists for API completeness only); Phase
/// 3 adds the diagnostic-emitting constructors.
//
// SAFETY note for the future: `ErrorGuaranteed` is `Copy`, so the
// `salsa::Update` derive on enums/structs containing `ErrorMarker` reduces
// to trivial replacement. We implement `Update` manually below rather than
// `#[derive(salsa::Update)]` because `ErrorGuaranteed` lives in `fossil-base`
// and does not itself derive `salsa::Update` (it predates this crate). Wrapping
// it in a newtype here lets us provide the impl without touching `fossil-base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorMarker(ErrorGuaranteed);

// SAFETY: third-party-trait integration boundary (per ADR-0004).
// `ErrorMarker` is `Copy + Eq`, so the trivial-replace pattern is sound: the
// new value either equals the old (no change) or replaces it bit-for-bit
// (self-contained, no nested invariants). No safe alternative exists because
// `salsa::Update` requires `unsafe impl` even for trivially-safe bodies.
#[allow(unsafe_code)]
unsafe impl salsa::Update for ErrorMarker {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: caller guarantees `old_pointer` is a valid, aligned pointer
        // to an initialised `ErrorMarker` owned by Salsa storage.
        let old = unsafe { &mut *old_pointer };
        if *old == new_value {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

/// Phase 1 type-check stub. Returns `Result<(), ErrorMarker>` rather than the
/// originally-planned `Result<(), ErrorGuaranteed>` because Salsa-tracked
/// function return types must implement [`salsa::Update`], and
/// `fossil_base::ErrorGuaranteed` does not (and we deliberately do not amend
/// `fossil-base` mid-Phase-1). [`ErrorMarker`] is the local newtype that does
/// implement Update; it carries the same `ErrorGuaranteed` taint internally.
/// Phase 3 (CORE-04..07) reconciles this by either making `ErrorGuaranteed`
/// itself impl `salsa::Update` upstream or by formalising `ErrorMarker` as
/// the `fossil-hir`-side surface.
#[salsa::tracked]
#[allow(clippy::unnecessary_wraps)] // Phase 3 returns Err on real type errors.
pub fn typecheck_mapping<'db>(
    _db: &'db dyn fossil_base::Db,
    _mapping: crate::def_map::MappingLoc<'db>,
) -> Result<(), ErrorMarker> {
    // Phase 1: trivially OK. Phase 3 (CORE-04..07) wires real checking with
    // forward propagation from CSVW and backward checking against `ShEx`.
    Ok(())
}
