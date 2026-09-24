//! `OutputDescriptorKind` — enum dispatch for inside-Salsa-query descriptor
//! access.
//!
//! This enum exists because [`crate::OutputDescriptor`] is a trait and Salsa
//! 0.26 cannot intern or memoize trait objects (`Box<dyn OutputDescriptor>`) —
//! they lack structural equality. Dispatch goes through this enum's variants
//! (concrete types), not through `&dyn OutputDescriptor`.
//!
//! The consumers are the HOSTS — `fossil-cli`, `fossil-df` and
//! `fossil-df-wasm` carry one into the executor. `fossil-hir` is not among
//! them and cannot be: it has no dependency on this crate.

use crate::AcceptAllDescriptor;
use fossil_graph_schema::GraphSchema;

/// Concrete-type dispatch surface for the bidirectional checker.
///
/// Each variant carries the descriptor's owned state so Salsa's `interned`
/// / tracked-struct interning can equate descriptors structurally. A new
/// variant is an architectural addition — every match arm in every host that
/// carries one has to answer for it, which is acceptable churn: adding an
/// output descriptor is a major architectural change.
#[derive(Debug)]
pub enum OutputDescriptorKind {
    /// A canonical output model, **already lowered** — whatever language it came
    /// from. The executor consumes it as-is.
    ///
    /// Every host decodes through the provider registry
    /// (`fossil_df::output_descriptor`), so this is the one shape-bearing
    /// variant whatever the document's language.
    Lowered(GraphSchema),
    /// Accepts any graph. Used when no shape target is loaded (the
    /// walking-skeleton case).
    AcceptAll(AcceptAllDescriptor),
}

impl OutputDescriptorKind {
    /// Inherent `const` default: the descriptor an executor uses when the
    /// program declares no output shape (`fossil_df::output_descriptor`).
    /// Backward checking is a no-op and the produced graph is accepted whole.
    ///
    /// This is const-evaluable because [`AcceptAllDescriptor`] is a unit
    /// struct (no fields, no heap, no non-const constructors). If a future
    /// variant adds non-const fields, the default has to switch to a
    /// `once_cell`/`OnceLock` static — but for v0.1 the inline-const pattern
    /// is the simplest.
    pub const ACCEPT_ALL_DEFAULT: Self = Self::AcceptAll(AcceptAllDescriptor);

    /// Stable, lowercase identifier matching the [`crate::OutputDescriptor`]
    /// trait's `name()`. The variant existence — not a stored string — IS
    /// the canonical name; this method maps it.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Lowered(_) => "lowered",
            Self::AcceptAll(_) => "accept-all",
        }
    }

    /// `true` iff the descriptor is `AcceptAll` (no backward-shape
    /// constraints).
    ///
    /// It said `fossil_hir`'s typecheck reads this to short-circuit backward
    /// checking. It does not, and cannot: `fossil-hir` has no dependency on this
    /// crate. Nothing in the tree calls this outside the tests below, and the
    /// same is true of [`Self::name`]: the executor reads only
    /// [`Self::to_graph_schema`].
    #[must_use]
    pub const fn accepts_anything(&self) -> bool {
        matches!(self, Self::AcceptAll(_))
    }

    /// Lower this descriptor to the canonical, format-neutral [`GraphSchema`] —
    /// the single output model the executor (`apply_output_shape`) consumes,
    /// independent of the source schema language. `Lowered` already is one;
    /// `AcceptAll` is empty
    /// (no node/edge typing → every predicate stays a vertex property, the
    /// walking-skeleton behaviour). The program's `@rename`s were applied when
    /// the document was decoded.
    #[must_use]
    pub fn to_graph_schema(&self) -> GraphSchema {
        match self {
            Self::Lowered(gs) => gs.clone(),
            Self::AcceptAll(_) => GraphSchema {
                nodes: Vec::new(),
                edges: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time test (the assignment itself is the assertion): proves
    /// [`OutputDescriptorKind::ACCEPT_ALL_DEFAULT`] is const-evaluable. If a
    /// future variant breaks const-constructibility, this fails to compile.
    #[test]
    fn accept_all_default_is_const_constructible() {
        const FOO: OutputDescriptorKind = OutputDescriptorKind::ACCEPT_ALL_DEFAULT;
        assert_eq!(FOO.name(), "accept-all");
    }

    #[test]
    fn output_descriptor_kind_accept_all_default() {
        let kind = OutputDescriptorKind::ACCEPT_ALL_DEFAULT;
        assert_eq!(kind.name(), "accept-all");
        assert!(kind.accepts_anything());
    }

    #[test]
    fn output_descriptor_kind_lowered_variant() {
        let kind = OutputDescriptorKind::Lowered(GraphSchema {
            nodes: Vec::new(),
            edges: Vec::new(),
        });
        assert_eq!(kind.name(), "lowered");
        assert!(!kind.accepts_anything());
    }

    /// `OutputDescriptorKind` must be `Send + Sync` because the executor
    /// carries one across the thread boundary its plan is run on.
    #[test]
    fn output_descriptor_kind_send_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OutputDescriptorKind>();
    }
}
