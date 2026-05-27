//! `HirDb` — the descriptor-aware `Db` extension trait (R2, ADR-0020).
//!
//! # The Phase-3 deferral this resolves
//!
//! Phase 3 (`shapes::resolve_target_shape`, deferred-items.md #3 + #8) left the
//! target-shape resolution returning `None` unconditionally because
//! [`fossil_base::Db::system`] hands back `&dyn fossil_base::System`, which does
//! NOT carry the `fossil_descriptors_output::SystemWithDescriptors` extension
//! vtable (ADR-0006 Option B keeps `fossil-base` descriptor-ignorant). A real
//! host `ShEx` schema therefore could not reach `typecheck_mapping`, so SC#4
//! target-side hover and SC#5 split-mapping were blocked.
//!
//! # R2, not R1 (the cycle-safe choice — ADR-0020, Wave-0 Spike B)
//!
//! The accessor lives on a `fossil-hir`-OWNED extension trait
//! ([`HirDb`]), NOT on `fossil_base::Db`. `fossil-hir` already depends on
//! `fossil-descriptors-output`, so naming
//! [`fossil_descriptors_output::OutputDescriptorKind`] here introduces no new
//! edge and no crate cycle. Putting the accessor on `fossil_base::Db` (R1)
//! would force `fossil-base` to depend on `fossil-descriptors-output`,
//! inverting the dependency direction and violating ADR-0006 — explicitly
//! rejected.
//!
//! # Why this is fan-out-safe (no Salsa key, no `Box<dyn>`)
//!
//! The descriptor is read as a PLAIN `&OutputDescriptorKind` value, never
//! interned, never passed as a `#[salsa::tracked]` query KEY, and never wrapped
//! in `Box<dyn Trait>`. This is exactly the ADR-0018 "descriptor as argument,
//! not key" seam. [`shapes::resolve_target_shape`] takes the borrowed kind as a
//! plain argument and reads it once at the top; the ten-mapping invalidation
//! fixture supplies `AcceptAll` → `None` → `MAX_PER_MAPPING_FAN_OUT` stays `1`.
//!
//! # The wiring contract
//!
//! A host that owns a concrete `Db` implementation (`FossilDb`, or a Phase-6
//! LSP/playground db wrapper) implements [`HirDb`] on that type, returning a
//! reference into its own descriptor storage (typically an
//! `Arc<OutputDescriptorKind>` field, mirroring the existing `Arc<dyn System>`
//! field). Callers in the target-side path
//! ([`shapes::resolve_target_shape`]) then receive the descriptor as a plain
//! argument the host pulls from [`HirDb::output_descriptor_kind`]. The orphan
//! rule prevents `fossil-hir` from implementing `HirDb` for `FossilDb`
//! directly when a host needs a non-default value, so the trait is the typed
//! carrier the host implements on ITS db; the plain-argument path through
//! `resolve_target_shape` is the primary mechanism v0.1 uses.
//!
//! [`shapes::resolve_target_shape`]: crate::shapes::resolve_target_shape

use fossil_descriptors_output::OutputDescriptorKind;

/// `Db` extension that exposes the host's output descriptor as a plain value.
///
/// Implemented by the host on its concrete `Db` type (e.g. a Phase-6 LSP db
/// wrapper). The default returns the degraded `AcceptAll` fallback
/// ([`OutputDescriptorKind::ACCEPT_ALL_DEFAULT`], per `decisions/rudof-wasm.md`
/// Path (b)), so a `Db` that has not loaded a `ShEx` schema transparently gets
/// the no-backward-checking behaviour the walking-skeleton + invalidation
/// fixture rely on.
pub trait HirDb: fossil_base::Db {
    /// The host's current output descriptor, borrowed as a plain value.
    ///
    /// Read ONCE at the top of the target-side path; never interned, never a
    /// Salsa key. Hosts that load a `ShEx` schema override this to return a
    /// reference into their own `Arc<OutputDescriptorKind>` storage.
    fn output_descriptor_kind(&self) -> &OutputDescriptorKind {
        &OutputDescriptorKind::ACCEPT_ALL_DEFAULT
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    // The canonical host-wrapper demonstration: a host implements `HirDb` on
    // its concrete `Db` type. `FossilDb` here stands in for that host db; the
    // default `output_descriptor_kind()` yields the degraded `AcceptAll`
    // fallback (a host that never loaded a `ShEx` schema). A real ShEx-loading
    // host overrides the method to return a reference into its own descriptor
    // storage.
    impl HirDb for fossil_base::FossilDb {}

    #[test]
    fn hir_db_default_is_accept_all() {
        let system: std::sync::Arc<dyn fossil_base::System> =
            std::sync::Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        assert!(db.output_descriptor_kind().accepts_anything());
    }
}
