//! `OutputDescriptorKind` — enum dispatch for inside-Salsa-query descriptor
//! access.
//!
//! Phase 3 introduces this enum because [`crate::OutputDescriptor`] is a
//! trait (per ADR-0003) and Salsa 0.26 cannot intern or memoize trait objects
//! (`Box<dyn OutputDescriptor>`) — they lack structural equality. Inside a
//! `#[salsa::tracked]` query body (plan 03-05's `typecheck_mapping`), dispatch
//! goes through this enum's variants (concrete types), not through
//! `&dyn OutputDescriptor`. See ADR-0006.
//!
//! The trait stays as the OUTSIDE-Salsa surface API (e.g. for the playground
//! UI listing loaded descriptors).

use crate::AcceptAllDescriptor;
use fossil_graph_schema::GraphSchema;
use fossil_shex::ShExDescriptor;

/// Concrete-type dispatch surface for the bidirectional checker.
///
/// Each variant carries the descriptor's owned state so Salsa's `interned`
/// / tracked-struct interning can equate descriptors structurally. New
/// variants (e.g. future `Shacl`) are an architectural addition — they
/// require updating every match arm in `fossil-hir`'s typecheck path. That
/// churn is acceptable: adding a new output descriptor is a major
/// architectural change.
//
// `large_enum_variant`: the `ShEx(ShExDescriptor)` variant carries a
// `shex_ast::Schema` + a resolved `HashMap<String, ShapeBinding>` — large
// compared to the unit-struct `AcceptAll` variant. Boxing the larger
// variant would add an allocation per `ShExDescriptor` construction, which
// happens once per compile; the size asymmetry is intentional and stable.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum OutputDescriptorKind {
    /// `ShEx` schema, pre-resolved into per-shape constraint tables. The
    /// compile-time backward checker (`fossil-hir`) needs the rich resolved
    /// table; the executor reads only [`Self::to_graph_schema`].
    ShEx(ShExDescriptor),
    /// A canonical output model, already lowered (the SHACL path produces this
    /// directly — SHACL is RDF, walked into [`GraphSchema`] — and any future
    /// pre-resolved source can reuse it). The executor consumes it as-is;
    /// `fossil-hir`'s `ShEx` backward checker treats it like `AcceptAll`.
    Shacl(GraphSchema),
    /// Phase 1 stub — accepts any graph. Used when no shape target is loaded
    /// (the walking-skeleton case) or as the degraded fallback if a host
    /// can't resolve a `ShEx` schema.
    AcceptAll(AcceptAllDescriptor),
}

impl OutputDescriptorKind {
    /// Inherent `const` default: the descriptor an executor uses when the
    /// program declares no output shape (`fossil_engine`'s `output_shape`,
    /// `fossil_df_wasm`'s `build_program`). Backward checking is a no-op and
    /// the produced graph is accepted whole.
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
            Self::ShEx(_) => "shex",
            Self::Shacl(_) => "shacl",
            Self::AcceptAll(_) => "accept-all",
        }
    }

    /// `true` iff the descriptor is `AcceptAll` (no backward-shape
    /// constraints). Plan 03-05's typecheck reads this to short-circuit
    /// backward checking.
    #[must_use]
    pub const fn accepts_anything(&self) -> bool {
        matches!(self, Self::AcceptAll(_))
    }

    /// Lower this descriptor to the canonical, format-neutral [`GraphSchema`] —
    /// the single output model the executor (`apply_output_shape`) consumes,
    /// independent of the source schema language. `ShEx` lowers through its
    /// resolved table; SHACL is already a `GraphSchema`; `AcceptAll` is empty
    /// (no node/edge typing → every predicate stays a vertex property, the
    /// walking-skeleton behaviour).
    #[must_use]
    pub fn to_graph_schema(&self) -> GraphSchema {
        match self {
            Self::ShEx(d) => d.to_graph_schema(),
            Self::Shacl(gs) => gs.clone(),
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
    /// Mitigates plan 03-03 Serious #8.
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
    fn output_descriptor_kind_shex_variant() {
        // Minimal schema with one shape — `ex:Person` with `ex:name`.
        let schema_src = r#"{
          "@context": "http://www.w3.org/ns/shex.jsonld",
          "type": "Schema",
          "shapes": [
            {
              "type": "ShapeDecl",
              "id": "http://example.org/Person",
              "shapeExpr": {
                "type": "Shape",
                "expression": {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/name"
                }
              }
            }
          ]
        }"#;
        let shex = ShExDescriptor::from_reader(schema_src.as_bytes()).expect("schema parses");
        let kind = OutputDescriptorKind::ShEx(shex);
        assert_eq!(kind.name(), "shex");
        assert!(!kind.accepts_anything());
    }

    /// SC#5 structural property: the enum supports swapping descriptors
    /// (a parsed `ShEx` document against the no-contract fallback) without
    /// `fossil-hir` source changes. This test exercises the swap pattern —
    /// building both variants and matching on them in the same function
    /// body — which IS the swap surface.
    #[test]
    fn output_descriptor_kind_swap_does_not_require_fossil_hir_change() {
        let schema_src = r#"{
          "@context": "http://www.w3.org/ns/shex.jsonld",
          "type": "Schema",
          "shapes": []
        }"#;
        let accept_all = OutputDescriptorKind::ACCEPT_ALL_DEFAULT;
        let shex = OutputDescriptorKind::ShEx(
            ShExDescriptor::from_reader(schema_src.as_bytes()).expect("schema parses"),
        );
        let kinds: [&OutputDescriptorKind; 2] = [&accept_all, &shex];
        for k in kinds {
            // The match shape itself is the swap surface. New variants
            // require a new match arm in fossil-hir, intentionally.
            let _name: &'static str = match k {
                OutputDescriptorKind::ShEx(_) => "shex",
                OutputDescriptorKind::Shacl(_) => "shacl",
                OutputDescriptorKind::AcceptAll(_) => "accept-all",
            };
        }
    }

    /// `OutputDescriptorKind` must be `Send + Sync` because the executor
    /// carries one across the thread boundary its plan is run on.
    #[test]
    fn output_descriptor_kind_send_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OutputDescriptorKind>();
    }
}
