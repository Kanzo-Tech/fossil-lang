//! Target-shape resolution.
//!
//! Maps a mapping's declared shape IRI to a Phase-3-internal [`ResolvedShape`]
//! carrying the per-predicate constraint table converted from the `ShEx`
//! [`fossil_descriptors_output::ShapeBinding`].
//!
//! # The document the program names (ADR-0055, F4)
//!
//! [`resolve_target_shape`] reads the shape document the PROGRAM brings in with
//! `type { … } = io.shex("…")`, through `System::read_file`, exactly as the
//! neighbouring [`crate::infer`] already reads the INPUT descriptor.
//!
//! It used to take the descriptor as a borrowed ARGUMENT instead, threaded in
//! by the host (ADR-0020's R2 wiring). The seam was sound — a plain argument is
//! never interned and never a Salsa key, so `MAX_PER_MAPPING_FAN_OUT` stayed
//! `1` — but the in-query caller had no descriptor to thread and passed
//! `OutputDescriptorKind::ACCEPT_ALL_DEFAULT`. One literal, and backward
//! checking was off for every program compiled through the checker: the axis
//! ADR-0055 found with exactly one value.
//!
//! Reading here keeps what the argument bought. A `System` read registers no
//! Salsa input dependency (see [`crate::infer`]'s module docs), so the fan-out
//! is unchanged and `tests/invalidation_regression.rs` still holds. And it buys
//! what the argument could not: a CSV-sourced program had nowhere to name a
//! document at all until `type … =` existed, which is the hole ADR-0055 named
//! and ADR-0057 filled.
//!
//! Naming no document is still `None` — backward checking is a correct no-op,
//! and ADR-0057's fifth amendment calls that a decision rather than a gap: the
//! program writes its corpus and nothing is checked. What is gone is that this
//! used to be the only outcome available.
//!
//! The backward-check LOGIC (constraint-table conversion, `OneOf` surfacing)
//! is also exposed as plain-Rust helpers ([`ResolvedShape::from_binding`],
//! [`one_of_rejections`]) that the diagnostic corpus drives directly with a
//! constructed [`fossil_descriptors_output::ShExDescriptor`].

use fossil_descriptors_output::{
    Cardinality, OneOfRejection, ResolvedConstraint, ShExLoweringError, ShapeBinding,
};
use rudof_iri::IriS;
use shex_ast::ShapeExpr;
use smol_str::SmolStr;

use fossil_graph_schema::Primitive;

use crate::def_map::MappingLoc;
use crate::ty::{ShapeId, Ty, TyKind};

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
    // and the IRI form as the full IRI. `from_xsd_iri` handles both.
    let iri_str = datatype.to_string();
    let prim = Primitive::from_xsd_iri(&iri_str)?;
    Some(Ty::new(db, TyKind::Primitive(prim)))
}

/// Map a Fossil [`Primitive`] to its `GraphAr` data-type spelling — the same
/// vocabulary [`fossil_sinks::manifest::data_type_name`] emits. A materializer
/// spelling, so it lives with the compiler and not on the lattice; the xsd
/// direction is [`Primitive::to_xsd_iri`], which does.
#[must_use]
pub const fn primitive_to_graphar(p: Primitive) -> &'static str {
    match p {
        Primitive::Integer => "int64",
        Primitive::Float => "double",
        Primitive::Bool => "bool",
        Primitive::Date => "date",
        Primitive::DateTime => "timestamp",
        Primitive::Time => "time",
        // String / AnyUri / GYear have no narrower GraphAr spelling.
        Primitive::String | Primitive::AnyUri | Primitive::GYear => "string",
    }
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

/// Resolve a mapping's target shape against the document the PROGRAM names.
///
/// The descriptor used to arrive as a borrowed argument the host threaded in,
/// and the in-query caller had nothing to thread, so it passed
/// `ACCEPT_ALL_DEFAULT` — one literal that turned backward checking off for
/// every program compiled through the checker. ADR-0055 said what to do
/// instead, and this is it: read the document here, through
/// `System::read_file`, exactly as the neighbouring [`crate::infer`] already
/// reads the INPUT descriptor.
///
/// That does not widen the Salsa key, which is why the argument existed. A
/// `System` read registers no Salsa input dependency (see `infer`'s module
/// docs), so `MAX_PER_MAPPING_FAN_OUT` stays `1` and
/// `tests/invalidation_regression.rs` still holds.
///
/// Returns `None` — backward checking is a correct no-op — when the program
/// names no document, when the mapping has no shape clause, or when the
/// document does not declare the shape the mapping targets.
#[must_use]
pub fn resolve_target_shape<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<ResolvedShape<'db>> {
    // The mapping's fully-resolved target shape IRI (prefix already expanded by
    // `lower_to_hir`). A mapping with no shape clause yields no resolution.
    let file = mapping.file(db);
    let hir = crate::lower::lower_to_hir(db, file);
    let hir_mapping = hir.mappings(db).get(mapping.index(db))?;
    let shape_iri = hir_mapping.shape_iri.clone();
    if shape_iri.is_empty() {
        return None;
    }

    // The document is whatever `type { … } = io.shex("…")` brought in. A
    // program that names none has no output contract — which ADR-0057's fifth
    // amendment calls a decision, not a hole: it writes its corpus and nothing
    // is checked.
    let dm = crate::def_map::def_map(db, file);
    let document = dm.output_shape_document(db)?;
    let resolved = crate::def_map::resolve_relative(db, file, document.as_str());
    let bytes = db.system().read_file(&resolved).ok()?;
    let descriptor =
        fossil_descriptors_output::ShExDescriptor::from_reader(bytes.as_slice()).ok()?;

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

// The `.fossil` sources below contain `${ex:}` / `${.id}` template placeholders
// and `type { … }` braces — LITERAL Fossil source, not Rust format-string args.
// Same allow, same reason, as the two `fossil-ide` integration tests.
#[allow(clippy::literal_string_with_formatting_args)]
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// Every `GraphAr` spelling is reachable, and the `String`-shaped corner of
    /// the lattice collapses on purpose. The xsd direction is not tested here —
    /// it is one function in `fossil-graph-schema`, tested there.
    #[test]
    fn graphar_spelling_covers_the_lattice() {
        assert_eq!(primitive_to_graphar(Primitive::Integer), "int64");
        assert_eq!(primitive_to_graphar(Primitive::DateTime), "timestamp");
        assert_eq!(primitive_to_graphar(Primitive::AnyUri), "string");
        assert_eq!(primitive_to_graphar(Primitive::GYear), "string");
    }

    // --- resolve_target_shape reads the document the program names (ADR-0055) ---

    use std::sync::Arc;

    /// A `.fossil` program whose single mapping targets `ex:Person` and which
    /// NAMES its output shape document. The document is the whole point: before
    /// this, a CSV-sourced program had nowhere to declare one, so the checker
    /// was handed `ACCEPT_ALL_DEFAULT` and checked nothing.
    fn src_naming(document: &str) -> String {
        format!(
            "prefix ex: <http://example.org/>\n\
             type {{ Person }} = io.shex(\"{document}\")\n\
             users := io.csv(\"x.csv\")\n\
             User : ex:Person from users\n    \
             iri = `${{ex:}}u/${{.id}}`\n    \
             ex:name = .name\n"
        )
    }

    /// The same program with no `type` line — it names no document at all.
    const SRC_WITHOUT_DOCUMENT: &str = "\
prefix ex: <http://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    fn new_db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    fn first_mapping(
        db: &fossil_base::FossilDb,
        src: String,
    ) -> (fossil_base::SourceFile, MappingLoc<'_>) {
        let file = fossil_base::SourceFile::new(db, src, "person.fossil".to_string());
        let mapping = crate::def_map::def_map(db, file).mappings(db)[0];
        (file, mapping)
    }

    #[test]
    fn the_target_shape_comes_from_the_document_the_program_names() {
        let db = new_db();
        let (_file, mapping) =
            first_mapping(&db, src_naming("tests/fixtures/output_shape/person.shex"));

        let resolved = resolve_target_shape(&db, mapping);
        assert!(
            resolved.is_some(),
            "the program names a document declaring its target shape, so backward \
             checking must have something to check against"
        );
        assert!(
            resolved
                .unwrap()
                .constraint_for("http://example.org/name")
                .is_some(),
            "the resolved shape must carry the ex:name constraint"
        );
    }

    /// Naming no document is a DECISION, not a hole (ADR-0057, fifth amendment):
    /// the program still writes its corpus, and nothing is checked. What is gone
    /// is that this used to be the only outcome, for every program.
    #[test]
    fn a_program_that_names_no_document_has_no_output_contract() {
        let db = new_db();
        let (_file, mapping) = first_mapping(&db, SRC_WITHOUT_DOCUMENT.to_string());
        assert!(resolve_target_shape(&db, mapping).is_none());
    }

    #[test]
    fn a_document_that_omits_the_target_shape_resolves_to_none() {
        let db = new_db();
        let (_file, mapping) = first_mapping(
            &db,
            src_naming("tests/fixtures/output_shape/no_shapes.shex"),
        );
        assert!(resolve_target_shape(&db, mapping).is_none());
    }
}
