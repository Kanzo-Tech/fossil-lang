//! `fossil-descriptors-output` — output-side shape descriptors.
//!
//! An [`OutputDescriptor`] tells the type-checker what shape the produced RDF
//! graph must conform to (backward shape checking: target shape → expected
//! triple terms → mapping body constraints).
//!
//! Phase 1 ships the trait + an [`AcceptAllDescriptor`] stub that accepts any
//! graph (sufficient for the `hello.fossil` walking-skeleton demo, which has
//! no shape target). Phase 3 CORE-06 (plan 03-03) adds the real `ShEx` path
//! ([`ShExDescriptor`]) and the [`OutputDescriptorKind`] enum that lets the
//! bidirectional checker dispatch without violating the "no
//! `Box<dyn Trait>` inside Salsa queries" rule (ADR-0006).
//!
//! ## Trait stability
//!
//! The Phase 1 trait surface (`name`, `accepts_anything`) is the public-API
//! commitment to Phase 3-9. Phase 3 CORE-06 grows the trait with
//! `parse_descriptor`, `type_for_property(shape, predicate) -> PropertyType`,
//! `cardinality(shape, predicate)`, `is_closed(shape)` — additive only.
//!
//! ## `DescriptorError` duplication note
//!
//! Phase 1 deliberately does NOT share `DescriptorError` with
//! `fossil-descriptors-input` (decision locked by orchestrator: keep dep graph
//! tidy; refactor in Phase 3 if cross-crate sharing proves painful).

pub mod kind;
pub mod system_ext;

pub use kind::OutputDescriptorKind;
// The ShEx decode now lives in the neutral `fossil-shex` crate (shared with the
// input descriptor). Re-exported here so this crate's public API is unchanged.
pub use fossil_shex::{
    Cardinality, OneOfRejection, ResolvedConstraint, ShExDescriptor, ShExLoweringError,
    ShapeBinding, SuggestionSeed, generate_split_suggestion,
};
pub use system_ext::SystemWithDescriptors;

/// Output-side shape descriptor.
///
/// Implementations parse a raw descriptor blob (`ShEx`, future `SHACL`) and
/// expose shape membership / cardinality queries used by the type-checker
/// for backward shape inference.
pub trait OutputDescriptor: Send + Sync + std::fmt::Debug {
    /// Stable, lowercase, namespace-free identifier (e.g. `"shex"`, `"accept-all"`).
    ///
    /// Trait signature returns `&str` (not `&'static str`) so Phase 3+
    /// implementations can return dynamically-computed names.
    fn name(&self) -> &str;

    /// Phase 1: tells the type-checker whether the descriptor is a permissive
    /// "anything goes" pass-through (used for demos with no shape target).
    /// Phase 3 CORE-06 supersedes with shape-aware queries
    /// (`type_for_property`, `cardinality`, `is_closed`).
    fn accepts_anything(&self) -> bool;
}

/// Phase 1 stub: accept-all output descriptor.
///
/// Returns `true` for [`OutputDescriptor::accepts_anything`] — the type-checker
/// short-circuits backward shape inference. Phase 3 CORE-06 (plan 03-03)
/// keeps this type as the `AcceptAll` variant of [`OutputDescriptorKind`] —
/// it's the fallback when no `ShEx` schema is loaded.
///
/// Declared as a unit struct (`pub struct AcceptAllDescriptor;`) so the
/// [`OutputDescriptorKind::ACCEPT_ALL_DEFAULT`] inherent const is
/// const-evaluable.
#[derive(Debug, Default)]
pub struct AcceptAllDescriptor;

impl OutputDescriptor for AcceptAllDescriptor {
    // Phase 1 returns a literal; the trait signature stays `&str` for
    // Phase 3+ dynamic naming (see OutputDescriptor::name() doc).
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "accept-all"
    }

    fn accepts_anything(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_all_descriptor_says_yes() {
        let d = AcceptAllDescriptor;
        assert_eq!(d.name(), "accept-all");
        assert!(d.accepts_anything());
    }
}
