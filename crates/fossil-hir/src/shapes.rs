//! Target-shape resolution.
//!
//! Maps a mapping's declared shape IRI to a Phase-3-internal [`ResolvedShape`]
//! carrying the per-predicate constraint table converted from the `ShEx`
//! [`fossil_descriptors_output::ShapeBinding`].
//!
//! # Dispatch on the host-supplied descriptor (R2 — ADR-0020)
//!
//! Backward shape checking dispatches on
//! [`fossil_descriptors_output::OutputDescriptorKind`]:
//!   - `ShEx(d)`     → `d.lookup_shape(&iri)` → `Option<&ShapeBinding>`
//!   - `AcceptAll(_)`→ `None` (skip backward checking)
//!
//! [`resolve_target_shape`] receives the descriptor kind as a PLAIN borrowed
//! ARGUMENT (read once at the top), supplied by the host through the
//! [`crate::HirDb`] extension trait (ADR-0020 — the R2 wiring that resolves the
//! Phase-3 deferral #3 / #8). It is NOT read via a `#[salsa::tracked]` query,
//! NOT interned, and NEVER a Salsa key — exactly the ADR-0018 "descriptor as
//! argument, not key" seam, so `MAX_PER_MAPPING_FAN_OUT` stays `1`. A host that
//! loads a `ShEx` schema now gets `Some(ResolvedShape)`; a host on the degraded
//! `AcceptAll` default (the walking-skeleton + ten-mapping invalidation
//! fixture) still gets `None` — backward checking remains a correct no-op
//! there. ADR-0006 keeps `fossil-base` descriptor-ignorant: the accessor lives
//! on the `fossil-hir`-owned [`crate::HirDb`], never on `fossil_base::Db`.
//!
//! The backward-check LOGIC (constraint-table conversion, `OneOf` surfacing)
//! is also exposed as plain-Rust helpers ([`ResolvedShape::from_binding`],
//! [`one_of_rejections`]) that the diagnostic corpus drives directly with a
//! constructed [`fossil_descriptors_output::ShExDescriptor`].

use fossil_descriptors_output::{
    Cardinality, OneOfRejection, OutputDescriptorKind, ResolvedConstraint, ShExLoweringError,
    ShapeBinding,
};
use rudof_iri::IriS;
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

/// Map a Fossil [`Primitive`] to its `GraphAr` data-type spelling — the same
/// vocabulary [`fossil_sinks::manifest::data_type_name`] emits. The forward
/// companion of [`primitive_from_xsd_iri`]; the single authority both the SQL
/// codegen and the `DataFusion` backend derive a vertex column's `data_type` from.
#[must_use]
pub const fn primitive_to_graphar(p: Primitive) -> &'static str {
    match p {
        Primitive::Integer => "int64",
        Primitive::Float => "double",
        Primitive::Bool => "bool",
        Primitive::Date => "date",
        Primitive::DateTime => "timestamp",
        Primitive::Time => "time",
        // String / AnyURI / GYear have no narrower GraphAr spelling.
        Primitive::String | Primitive::AnyURI | Primitive::GYear => "string",
    }
}

/// The canonical XSD datatype IRI for a Fossil [`Primitive`] — the output spec's
/// literal datatype, carried into the manifest for the host's governance layer
/// (DCAT). Mirrors [`primitive_to_graphar`] but in the RDF/XSD vocabulary.
#[must_use]
pub fn primitive_to_xsd(p: Primitive) -> String {
    let local = match p {
        Primitive::Integer => "integer",
        Primitive::Float => "double",
        Primitive::Bool => "boolean",
        Primitive::Date => "date",
        Primitive::DateTime => "dateTime",
        Primitive::Time => "time",
        Primitive::AnyURI => "anyURI",
        Primitive::GYear => "gYear",
        Primitive::String => "string",
    };
    format!("http://www.w3.org/2001/XMLSchema#{local}")
}

/// The XSD `string` IRI — the fallback datatype when a field's primitive is
/// unknown or the value is a string literal.
#[must_use]
pub fn default_xsd_string() -> String {
    "http://www.w3.org/2001/XMLSchema#string".to_string()
}

/// Peel `Optional`/`Seq` wrappers to the inner [`Primitive`], if any — the
/// datatype carried on a vertex property column.
#[must_use]
pub fn inner_primitive<'db>(db: &'db dyn fossil_base::Db, ty: Ty<'db>) -> Option<Primitive> {
    match ty.kind(db) {
        TyKind::Primitive(p) => Some(*p),
        TyKind::Optional(inner) | TyKind::Seq(inner) => inner_primitive(db, *inner),
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

/// Resolve a mapping's target shape against the host-supplied descriptor.
///
/// R2 wiring (ADR-0020 — resolves Phase-3 deferral #3 / #8). The descriptor
/// `kind` is a PLAIN borrowed argument (read once here), threaded in by the
/// host via [`crate::HirDb::output_descriptor_kind`]; it is never interned and
/// never a Salsa key, so `MAX_PER_MAPPING_FAN_OUT` stays `1`.
///
/// - `OutputDescriptorKind::ShEx(d)`: look up the mapping's fully-resolved
///   shape IRI in the descriptor. Returns `Some(ResolvedShape)` iff the
///   descriptor declares that shape (otherwise `None` — the mapping targets a
///   shape the schema does not define).
/// - `OutputDescriptorKind::AcceptAll(_)`: `None` — backward checking is a
///   correct no-op (the degraded fallback; the walking-skeleton + the
///   ten-mapping invalidation fixture both land here).
#[must_use]
pub fn resolve_target_shape<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    kind: &OutputDescriptorKind,
) -> Option<ResolvedShape<'db>> {
    // Read the descriptor as a plain value — AcceptAll short-circuits.
    let descriptor = match kind {
        OutputDescriptorKind::ShEx(d) => d,
        OutputDescriptorKind::AcceptAll(_) => return None,
    };

    // The mapping's fully-resolved target shape IRI (prefix already expanded by
    // `lower_to_hir`). A mapping with no shape clause yields no resolution.
    let file = mapping.file(db);
    let hir = crate::lower::lower_to_hir(db, file);
    let hir_mapping = hir.mappings(db).get(mapping.index(db))?;
    let shape_iri = hir_mapping.shape_iri.clone();
    if shape_iri.is_empty() {
        return None;
    }

    // Dispatch into the ShEx descriptor's resolved shape table.
    let iri = IriS::new_unchecked(shape_iri.as_str());
    let binding = descriptor.lookup_shape(&iri)?;

    // Filter the descriptor's lowering errors to this shape so the consuming
    // mapping surfaces only its own OneOf/cycle/unresolved-ref rejections.
    let errors: Vec<ShExLoweringError> = descriptor
        .lowering_errors()
        .iter()
        .filter(|e| lowering_error_targets(e, shape_iri.as_str()))
        .cloned()
        .collect();

    // Phase 3 v0.1 mints a stable per-mapping shape id from the mapping index
    // (each mapping targets at most one shape).
    let shape_id = ShapeId::placeholder(u32::try_from(mapping.index(db)).unwrap_or(u32::MAX));
    Some(ResolvedShape::from_binding(db, binding, shape_id, errors))
}

/// `true` iff a `ShEx` lowering error belongs to the shape identified by
/// `shape_iri`. `OneOfRejection` carries the offending shape IRI; other error
/// variants are conservatively associated with every shape (they describe
/// schema-wide problems the consuming mapping should still see).
fn lowering_error_targets(err: &ShExLoweringError, shape_iri: &str) -> bool {
    match err {
        ShExLoweringError::OneOfRejection(rej) => rej.shape_iri.as_str() == shape_iri,
        _ => true,
    }
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

    // --- ADR-0020 R2 wiring: resolve_target_shape consumes a host descriptor ---

    use std::sync::Arc;

    use fossil_descriptors_output::ShExDescriptor;

    /// A minimal `.fossil` source whose single mapping targets `ex:Person`,
    /// resolving to `http://example.org/Person` — the shape the `ShEx` fixture
    /// below declares.
    const SRC: &str = "\
prefix ex: <http://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    /// A `ShEx` schema declaring exactly `ex:Person` (full IRI
    /// `http://example.org/Person`) with one triple constraint `ex:name`.
    const SHEX_SRC: &str = r#"{
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

    fn new_db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    #[test]
    fn resolve_target_shape_returns_some_for_matching_shex_kind() {
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, SRC.to_string(), "person.fossil".to_string());
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let shex = ShExDescriptor::from_reader(SHEX_SRC.as_bytes()).expect("schema parses");
        let kind = OutputDescriptorKind::ShEx(shex);

        let resolved = resolve_target_shape(&db, mapping, &kind);
        assert!(
            resolved.is_some(),
            "a host ShEx descriptor declaring the mapping's target shape must \
             resolve to Some (ADR-0020 R2 wiring; Phase-3 deferral #3 resolved)"
        );
        let resolved = resolved.unwrap();
        assert!(
            resolved.constraint_for("http://example.org/name").is_some(),
            "the resolved shape must carry the ex:name constraint"
        );
    }

    #[test]
    fn resolve_target_shape_returns_none_for_accept_all() {
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, SRC.to_string(), "person.fossil".to_string());
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let kind = OutputDescriptorKind::ACCEPT_ALL_DEFAULT;
        assert!(
            resolve_target_shape(&db, mapping, &kind).is_none(),
            "AcceptAll is the degraded fallback — backward checking is a no-op"
        );
    }

    #[test]
    fn resolve_target_shape_returns_none_when_schema_omits_the_shape() {
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, SRC.to_string(), "person.fossil".to_string());
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        // A ShEx schema that declares NO shapes — the mapping's target is absent.
        let empty = r#"{
          "@context": "http://www.w3.org/ns/shex.jsonld",
          "type": "Schema",
          "shapes": []
        }"#;
        let shex = ShExDescriptor::from_reader(empty.as_bytes()).expect("schema parses");
        let kind = OutputDescriptorKind::ShEx(shex);
        assert!(
            resolve_target_shape(&db, mapping, &kind).is_none(),
            "a ShEx descriptor lacking the target shape must resolve to None"
        );
    }
}
