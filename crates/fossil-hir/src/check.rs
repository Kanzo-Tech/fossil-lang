//! Phase 2 type-check stub.
//!
//! Phase 1's local taint-wrapper newtype is REMOVED per plan 02-05 /
//! RESEARCH.md §Q6 — [`fossil_base::ErrorGuaranteed`] now provides
//! `unsafe impl salsa::Update` directly (at the third-party-trait
//! integration boundary per ADR-0004), so the local wrapper has no
//! purpose. `Ty::Error` carries `ErrorGuaranteed` directly.
//!
//! Phase 3 (CORE-04..07) plumbs the real bidirectional checking with forward
//! propagation from CSVW and backward checking against `ShEx`. Phase 2 ships
//! the stubbed query signature returning `Result<(), ErrorGuaranteed>`.

use fossil_base::ErrorGuaranteed;

#[salsa::tracked]
#[allow(clippy::unnecessary_wraps)] // Phase 3 returns Err on real type errors.
pub fn typecheck_mapping<'db>(
    _db: &'db dyn fossil_base::Db,
    _mapping: crate::def_map::MappingLoc<'db>,
) -> Result<(), ErrorGuaranteed> {
    // Phase 2: trivially OK. Phase 3 (CORE-04..07) wires real checking with
    // forward propagation from CSVW and backward checking against `ShEx`.
    Ok(())
}
