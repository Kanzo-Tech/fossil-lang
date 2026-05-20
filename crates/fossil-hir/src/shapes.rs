//! Target-shape resolution.
//!
//! Maps a mapping's declared shape IRI to a Phase-3-internal [`ResolvedShape`]
//! carrying the per-predicate constraint table converted from the `ShEx`
//! [`fossil_descriptors_output::ShapeBinding`].
//!
//! # Dispatch + the `Db::system()` limitation
//!
//! Backward shape checking dispatches on
//! [`fossil_descriptors_output::OutputDescriptorKind`]:
//!   - `ShEx(d)`     → `d.lookup_shape(&iri)` → `Option<&ShapeBinding>`
//!   - `AcceptAll(_)`→ `None` (skip backward checking)
//!
//! However, [`fossil_base::Db::system`] returns `&dyn fossil_base::System`,
//! which does NOT carry the `SystemWithDescriptors` vtable (the extension
//! trait lives in `fossil-descriptors-output` per ADR-0006, Option B). So the
//! tracked `typecheck_mapping` query CANNOT reach the host's descriptor through
//! the thin `Db` trait in Phase 3 v0.1. The hosts (CLI / WASM) both default to
//! `AcceptAll` anyway (plan 03-03), so [`resolve_target_shape`] returns `None`
//! in the standard query path — backward checking is a no-op for the
//! walking-skeleton and the ten-mapping invalidation fixture (neither declares
//! a `ShEx` schema).
//!
//! The backward-check LOGIC (constraint-table conversion, `OneOf` surfacing)
//! is exposed as plain-Rust helpers ([`ResolvedShape::from_binding`],
//! [`one_of_rejections`]) that plan 03-05's integration tests drive directly
//! with a constructed [`fossil_descriptors_output::ShExDescriptor`].
//! Widening `Db::system()` to surface `SystemWithDescriptors` (so a real host
//! `ShEx` schema flows into the query) is deferred to Phase 6 LSP wiring — it
//! is a `Db`-trait change out of plan 03-05's scope.

use fossil_descriptors_output::{
    Cardinality, OneOfRejection, ResolvedConstraint, ShExLoweringError, ShapeBinding,
};
use shex_ast::ShapeExpr;
use smol_str::SmolStr;

use crate::def_map::MappingLoc;
use crate::ty::{Primitive, ShapeId, Ty, TyKind};

/// One predicate constraint converted from a `ShEx` [`ResolvedConstraint`] into
/// Fossil type space.
#[derive(Debug, Clone)]
pub struct ShapeConstraint<'db> {
    /// The predicate IRI (fully-resolved string form).
    pub predicate: SmolStr,
    /// The expected value type, if the `ShEx` `valueExpr` was a
    /// `NodeConstraint` carrying a recognisable xsd `datatype`. `None` means
    /// "any value" (no datatype narrowing).
    pub value_ty: Option<Ty<'db>>,
    /// Cardinality decoded from the `ShEx` `(min, max)` encoding.
    pub cardinality: Cardinality,
}

/// A mapping's resolved target shape — Phase-3-internal.
#[derive(Debug, Clone)]
pub struct ResolvedShape<'db> {
    /// Interned shape id (a stable handle for `TyKind::Shape`).
    pub shape_id: ShapeId,
    /// Per-predicate constraint table.
    pub constraints: Vec<ShapeConstraint<'db>>,
    /// `OneOf` / cycle / unresolved-ref lowering errors filtered to this shape;
    /// plan 03-05's emitter surfaces these as diagnostics on the consuming
    /// mapping.
    pub errors: Vec<ShExLoweringError>,
}

impl<'db> ResolvedShape<'db> {
    /// Build a [`ResolvedShape`] from a resolved `ShEx` [`ShapeBinding`].
    ///
    /// Plain-Rust helper. `shape_id` is supplied by the caller (a stable id is
    /// minted per-mapping; Phase 3 v0.1 uses the mapping index since each
    /// mapping targets at most one shape).
    #[must_use]
    pub fn from_binding(
        db: &'db dyn fossil_base::Db,
        binding: &ShapeBinding,
        shape_id: ShapeId,
        errors: Vec<ShExLoweringError>,
    ) -> Self {
        let constraints = binding
            .constraints
            .iter()
            .map(|c| convert_constraint(db, c))
            .collect();
        Self {
            shape_id,
            constraints,
            errors,
        }
    }

    /// Find the constraint matching a predicate IRI, if any.
    #[must_use]
    pub fn constraint_for(&self, predicate_iri: &str) -> Option<&ShapeConstraint<'db>> {
        self.constraints
            .iter()
            .find(|c| c.predicate.as_str() == predicate_iri)
    }
}

/// Convert a single `ShEx` [`ResolvedConstraint`] to a [`ShapeConstraint`].
fn convert_constraint<'db>(
    db: &'db dyn fossil_base::Db,
    c: &ResolvedConstraint,
) -> ShapeConstraint<'db> {
    let value_ty = c
        .value_expr
        .as_ref()
        .and_then(|se| ty_from_shape_expr(db, se));
    ShapeConstraint {
        predicate: SmolStr::from(c.predicate.to_string()),
        value_ty,
        cardinality: c.cardinality,
    }
}

/// Map a `ShEx` `valueExpr` to a Fossil [`Ty`], if it is a `NodeConstraint`
/// carrying a recognisable xsd `datatype`. Returns `None` for any other shape
/// expression (treated as "any value").
fn ty_from_shape_expr<'db>(db: &'db dyn fossil_base::Db, se: &ShapeExpr) -> Option<Ty<'db>> {
    let ShapeExpr::NodeConstraint(nc) = se else {
        return None;
    };
    let datatype = nc.datatype()?;
    // `IriRef` implements `Display` — the prefixed form renders as `prefix:local`
    // and the IRI form as the full IRI. `primitive_from_xsd_iri` handles both.
    let iri_str = datatype.to_string();
    let prim = primitive_from_xsd_iri(&iri_str)?;
    Some(Ty::new(db, TyKind::Primitive(prim)))
}

/// Map an xsd datatype IRI (or `xsd:`-prefixed name) to a [`Primitive`].
///
/// Mirrors the CSVW datatype catalog (plan 03-02) so the forward (CSVW) and
/// backward (`ShEx`) sides agree on the primitive lattice. Recognises both the
/// full `http://www.w3.org/2001/XMLSchema#<name>` IRI and the `xsd:<name>`
/// prefixed form.
#[must_use]
pub fn primitive_from_xsd_iri(iri: &str) -> Option<Primitive> {
    let local = iri
        .rsplit(['#', '/'])
        .next()
        .unwrap_or(iri)
        .strip_prefix("xsd:")
        .unwrap_or_else(|| iri.rsplit(['#', '/']).next().unwrap_or(iri));
    match local {
        "string" => Some(Primitive::String),
        "integer" | "long" | "int" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" => Some(Primitive::Integer),
        "decimal" | "float" | "double" | "number" => Some(Primitive::Float),
        "boolean" => Some(Primitive::Bool),
        "date" => Some(Primitive::Date),
        "dateTime" | "dateTimeStamp" => Some(Primitive::DateTime),
        "time" => Some(Primitive::Time),
        "gYear" => Some(Primitive::GYear),
        "anyURI" => Some(Primitive::AnyURI),
        _ => None,
    }
}

/// Extract the [`OneOfRejection`]s (and other lowering errors) from a
/// descriptor's `lowering_errors()` for surfacing on the consuming mapping.
///
/// Plain-Rust helper — plan 03-05's integration tests use this together with a
/// constructed `ShExDescriptor` to verify SC#4 diagnostic emission.
#[must_use]
pub fn one_of_rejections(errors: &[ShExLoweringError]) -> Vec<&OneOfRejection> {
    errors
        .iter()
        .filter_map(|e| match e {
            ShExLoweringError::OneOfRejection(rej) => Some(rej),
            _ => None,
        })
        .collect()
}

/// Resolve a mapping's target shape in the standard tracked-query path.
///
/// Phase 3 v0.1: always returns `None` because [`fossil_base::Db::system`]
/// returns `&dyn System` without the `SystemWithDescriptors` extension vtable
/// (see module docs). Hosts default to `AcceptAll`, so this is correct for the
/// walking-skeleton + the ten-mapping invalidation fixture. The real `ShEx`
/// wiring (and the `Db`-trait widening it requires) lands in Phase 6.
#[must_use]
#[allow(clippy::unused_self, clippy::needless_pass_by_value)]
pub fn resolve_target_shape<'db>(
    _db: &'db dyn fossil_base::Db,
    _mapping: MappingLoc<'db>,
) -> Option<ResolvedShape<'db>> {
    None
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn primitive_from_xsd_iri_recognises_full_iris() {
        assert_eq!(
            primitive_from_xsd_iri("http://www.w3.org/2001/XMLSchema#string"),
            Some(Primitive::String)
        );
        assert_eq!(
            primitive_from_xsd_iri("http://www.w3.org/2001/XMLSchema#integer"),
            Some(Primitive::Integer)
        );
        assert_eq!(
            primitive_from_xsd_iri("http://www.w3.org/2001/XMLSchema#dateTime"),
            Some(Primitive::DateTime)
        );
    }

    #[test]
    fn primitive_from_xsd_iri_recognises_prefixed_form() {
        assert_eq!(
            primitive_from_xsd_iri("xsd:string"),
            Some(Primitive::String)
        );
        assert_eq!(
            primitive_from_xsd_iri("xsd:integer"),
            Some(Primitive::Integer)
        );
    }

    #[test]
    fn primitive_from_xsd_iri_returns_none_for_unknown() {
        assert_eq!(
            primitive_from_xsd_iri("http://example.org/CustomType"),
            None
        );
    }
}
