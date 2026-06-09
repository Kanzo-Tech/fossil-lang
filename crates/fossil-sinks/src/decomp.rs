//! `ShEx`-driven vertex/edge decomposition (SINK-01 / SINK-04 / SINK-05, ADR-0018).
//!
//! This module turns a typed mapping plan + a target `ShEx` shape table into a [`SinkPlan`]:
//! the decomposed set of `GraphAr` **vertex tables** + **edge tables** that the chunked
//! `COPY ... (FORMAT PARQUET)` emission (plan 05-08) materializes.
//!
//! ## SC#4 option (b) — the descriptor is a function argument
//!
//! Per ADR-0018 (and `RESEARCH` §"ShEx-Driven Decomposition"), the [`ShExDescriptor`] /
//! [`OutputDescriptorKind`] is passed **directly** into [`vertex_edge_decomp`] as an argument —
//! it is NEVER read through `Db::system()`. The full `Db`-wiring (resolving the descriptor from
//! `db.system()` inside the type-checker path) stays deferred to Phase 6; that wiring "lights up"
//! this same seam by feeding the resolved [`OutputDescriptorKind`] in with **zero decomposition
//! changes**. The decomposition itself is plain Rust over the resolved structs — no `Box<dyn
//! Trait>`, no Salsa read, fan-out-safe (CLAUDE.md hard rule + Pitfall 6).
//!
//! ## The algorithm (`RESEARCH` §"The decomposition algorithm")
//!
//! For each [`ShapeBinding`] `S` (one shape = one vertex type):
//! - `vertex_id` := the resolved `iri` Extend column, used **verbatim** (SINK-04 — no hash, no
//!   sequential id).
//! - Partition `S.constraints` by object kind:
//!   - a **literal-object** constraint (`value_expr` is a `NodeConstraint` carrying a `datatype`)
//!     becomes a **vertex property** of `S`; its `data_type` comes from the datatype IRI mapped
//!     through [`crate::manifest::data_type_name`].
//!   - an **IRI-object** constraint whose `value_expr` references **another shape**
//!     (`ShapeExpr::Ref`) becomes an **edge** `S --predicate--> S'`.
//! - [`Cardinality`] drives merging (SINK-05): `Exact(_)` / `ZeroOrOne` ⇒ single-valued ⇒ collapse
//!   duplicate subjects (`SELECT DISTINCT ON (iri) ... ORDER BY iri`); `OneOrMore` / `ZeroOrMore` /
//!   `Range { max > 1 }` ⇒ keep all rows.
//!
//! `AcceptAll` (no shape target — the walking-skeleton case) falls back to the Phase-4 flat-triple
//! passthrough: a single pseudo-vertex carrying no decomposition, so non-`ShEx` compiles are
//! unaffected.

use arrow_schema::DataType;
use fossil_descriptors_output::{OutputDescriptorKind, ResolvedConstraint, ShapeBinding};
use fossil_mir::MirGraph;
use shex_ast::{NodeKind, ShapeExpr};

use crate::manifest::data_type_name;

/// The canonical column name for the resolved subject IRI in the mapping relation.
///
/// The `iri = ...` property of every Fossil mapping lowers to an `Extend` column conventionally
/// named `iri`; the decomposition uses it verbatim as `vertex_id` / `src_id` / `dst_id` (SINK-04).
pub const IRI_COLUMN: &str = "iri";

/// The default placeholder source relation used when the caller has not yet wired the real MIR
/// relation SQL (the unit-test / fixture path). Plan 05-08 substitutes the real relation subquery.
pub const PLACEHOLDER_RELATION: &str = "relation";

/// A fully-decomposed sink plan: the `GraphAr` vertex + edge tables a mapping produces under a
/// target `ShEx` shape, plus the chunk configuration.
///
/// This is the output of [`vertex_edge_decomp`]. The COPY-SQL emission + chunk materialization is
/// plan 05-08; this struct is the load-bearing decomposition product (SINK-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkPlan {
    /// One vertex table per `ShEx` shape (literal-object predicates → properties).
    pub vertices: Vec<VertexTable>,
    /// One edge table per inter-shape IRI-object predicate.
    pub edges: Vec<EdgeTable>,
    /// Rows per Parquet chunk (carried for 05-08's chunked COPY; default [`DEFAULT_CHUNK_SIZE`]).
    pub chunk_size: u64,
}

/// One decomposed vertex table — a `ShEx` shape projected to its `vertex_id` + literal properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexTable {
    /// The shape's local type name (e.g. `"Person"`), used as the `GraphAr` vertex `type`.
    pub type_name: String,
    /// The full RDF type IRI (the shape IRI, e.g. `"https://example.org/Person"`);
    /// `None` when no RDF type is known (the flat-triple passthrough). Carried
    /// into the manifest so the host's governance layer reads the output spec
    /// instead of re-deriving it.
    pub rdf_type: Option<String>,
    /// The column used verbatim as the primary `vertex_id` — always [`IRI_COLUMN`] (SINK-04).
    pub vertex_id_col: String,
    /// Literal-object properties: `(name, data_type, single_valued)`.
    ///
    /// `data_type` is a `GraphAr` spelling (`string`, `int64`, ...); `single_valued` is `true`
    /// when the `ShEx` cardinality is `Exact(_)` / `ZeroOrOne` (collapse duplicate subjects).
    pub properties: Vec<VertexProperty>,
    /// The source relation (subquery / view name) the vertex SELECT reads `FROM`.
    pub source_relation: String,
}

/// A single literal-object property column of a [`VertexTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexProperty {
    /// Property (column) name — the predicate's local part (e.g. `"name"`).
    pub name: String,
    /// `GraphAr` data-type spelling derived from the constraint's datatype IRI.
    pub data_type: String,
    /// The full RDF predicate IRI (e.g. `"https://example.org/name"`); `None`
    /// when not known. Part of the output spec the governance layer consumes.
    pub rdf_uri: Option<String>,
    /// The XSD datatype IRI of the literal (e.g.
    /// `"http://www.w3.org/2001/XMLSchema#string"`); `None` when not known.
    pub xsd_datatype: Option<String>,
    /// `true` ⇒ single-valued (`Exact`/`ZeroOrOne`) ⇒ collapse duplicate subjects;
    /// `false` ⇒ multi-valued ⇒ keep all rows.
    pub single_valued: bool,
}

/// One decomposed edge table — an inter-shape IRI-object predicate `src --predicate--> dst`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeTable {
    /// Source vertex type label.
    pub src_type: String,
    /// Edge type label (the predicate's local part, e.g. `"knows"`).
    pub predicate: String,
    /// Destination vertex type label.
    pub dst_type: String,
    /// SQL expression yielding the source `vertex_id` — the [`IRI_COLUMN`] verbatim (SINK-04).
    pub src_id_expr: String,
    /// SQL expression yielding the destination `vertex_id` — the IRI-object column verbatim.
    pub dst_id_expr: String,
    /// `true` ⇒ at most one edge per subject (`Exact`/`ZeroOrOne`); `false` ⇒ keep all
    /// (`OneOrMore`/`ZeroOrMore`).
    pub single_valued: bool,
    /// The source relation (subquery / view name) the edge SELECT reads `FROM`.
    pub source_relation: String,
}


/// Render the inner vertex SELECT for a [`VertexTable`] (the body 05-08 wraps in `COPY ... TO`).
///
/// Single-valued vertices collapse duplicate subjects with `SELECT DISTINCT ON (iri)`; every SELECT
/// carries a deterministic `ORDER BY iri` so the chunked-COPY parity digest is stable across
/// native↔WASM (Pitfall 5 — the deterministic `ORDER BY` is mandatory). The `iri` column is
/// projected verbatim as `id` (SINK-04).
///
/// Property columns follow the subject `id`; if the vertex has any single-valued property the whole
/// vertex projection collapses (one row per subject), matching the per-shape cardinality.
#[must_use]
pub fn vertex_select_sql(v: &VertexTable) -> String {
    let collapse = v.properties.iter().any(|p| p.single_valued) || v.properties.is_empty();
    let mut cols = format!("{IRI_COLUMN} AS id");
    for p in &v.properties {
        cols.push_str(", ");
        cols.push_str(&p.name);
    }
    let distinct = if collapse {
        format!("DISTINCT ON ({IRI_COLUMN}) ")
    } else {
        String::new()
    };
    format!(
        "SELECT {distinct}{cols} FROM {relation} ORDER BY {IRI_COLUMN}",
        relation = v.source_relation,
    )
}

/// Render the inner edge SELECT for an [`EdgeTable`] (the body 05-08 wraps in `COPY ... TO`).
///
/// Projects `src_id_expr AS src_id`, `dst_id_expr AS dst_id` verbatim (SINK-04). A deterministic
/// `ORDER BY src_id, dst_id` keeps the parity digest stable (Pitfall 5). Single-valued edges
/// collapse with `DISTINCT ON (src_id)`; multi-valued edges (`OneOrMore`/`ZeroOrMore`) keep all rows.
#[must_use]
pub fn edge_select_sql(e: &EdgeTable) -> String {
    let distinct = if e.single_valued {
        "DISTINCT ON (src_id) ".to_string()
    } else {
        String::new()
    };
    format!(
        "SELECT {distinct}{src} AS src_id, {dst} AS dst_id FROM {relation} ORDER BY src_id, dst_id",
        src = e.src_id_expr,
        dst = e.dst_id_expr,
        relation = e.source_relation,
    )
}

/// Decompose a mapping `plan` into a [`SinkPlan`] under a target shape descriptor (SC#4 option (b)).
///
/// The `kind` descriptor is passed **directly** (NOT read via `Db::system()`). `db` is the Salsa
/// handle needed only to read `plan.ops(db)` for the source-relation derivation; the decomposition
/// logic itself is plain Rust over the resolved [`ShapeBinding`]s. For the descriptor-only fixture
/// path (no real relation SQL yet), see [`vertex_edge_decomp_from_kind`].
///
/// - `AcceptAll` ⇒ a single flat-triple passthrough pseudo-vertex (Phase-4 behavior preserved).
/// - `ShEx(desc)` ⇒ one [`VertexTable`] per shape + one [`EdgeTable`] per inter-shape predicate.
#[must_use]
pub fn vertex_edge_decomp<'db>(
    plan: &MirGraph<'db>,
    db: &'db dyn fossil_base::Db,
    kind: &OutputDescriptorKind,
    chunk_size: u64,
) -> SinkPlan {
    // The plan's ops fix the source relation the vertex/edge SELECTs read FROM. v0.1 carries a
    // placeholder relation name (the real relation subquery is wired by 05-08's codegen seam);
    // reading `plan.ops(db)` here keeps the signature honest and the Phase-6 wiring identical.
    let _ops = plan.ops(db);
    vertex_edge_decomp_from_kind(kind, PLACEHOLDER_RELATION, chunk_size, None, false)
}

/// Descriptor-only decomposition core — the `db`-free seam shared by [`vertex_edge_decomp`] and the
/// unit-test fixture path.
///
/// `source_relation` is the SQL relation (subquery body or view name) the generated SELECTs read
/// `FROM`; the unit tests pass [`PLACEHOLDER_RELATION`], 05-08 passes the real MIR relation.
/// `target_shape`: when `Some(iri)`, emit ONLY the vertex (+ its edges) for the
/// shape whose IRI equals `iri` — used per mapping so a multi-shape descriptor
/// doesn't apply every shape to every mapping's relation. `None` emits all
/// shapes (the descriptor-only fixture path). Edge classification still sees ALL
/// shape IRIs so `@ref`s to other shapes are recognised as edges.
///
/// `unnest_multivalued`: when `true`, a multi-valued edge's IRI-object column is
/// a `LIST` (the RDF provider pivots `*`-cardinality predicates with
/// `list(object) FILTER (...)`), so the destination IRI is unrolled with
/// `UNNEST(...)` — one edge row per list element. Native scalar sources (CSV,
/// where each row already carries a single object) leave it `false`.
#[must_use]
pub fn vertex_edge_decomp_from_kind(
    kind: &OutputDescriptorKind,
    source_relation: &str,
    chunk_size: u64,
    target_shape: Option<&str>,
    unnest_multivalued: bool,
) -> SinkPlan {
    match kind {
        OutputDescriptorKind::AcceptAll(_) => SinkPlan {
            // No shape target: a single flat-triple passthrough pseudo-vertex preserves the Phase-4
            // flat-COPY behavior so non-ShEx compiles (the walking-skeleton) are unaffected.
            vertices: vec![VertexTable {
                type_name: "_triples".to_string(),
                rdf_type: None,
                vertex_id_col: IRI_COLUMN.to_string(),
                properties: Vec::new(),
                source_relation: source_relation.to_string(),
            }],
            edges: Vec::new(),
            chunk_size,
        },
        OutputDescriptorKind::ShEx(desc) => {
            // Pre-compute the set of shape IRIs so an IRI-object constraint can be classified as an
            // edge (points at another known shape) vs an opaque IRI literal.
            let shape_iris: Vec<String> = desc.shapes().map(|s| s.iri.to_string()).collect();

            let mut vertices = Vec::new();
            let mut edges = Vec::new();

            // Stable order: sort shapes by IRI so the SinkPlan (and its snapshot) is deterministic
            // regardless of HashMap iteration order.
            let mut bindings: Vec<&ShapeBinding> = desc.shapes().collect();
            bindings.sort_by(|a, b| a.iri.to_string().cmp(&b.iri.to_string()));
            // Per-mapping scoping: keep only the shape this mapping outputs, so a
            // multi-shape descriptor doesn't project every shape onto this
            // mapping's relation (which carries only its own columns).
            if let Some(target) = target_shape {
                bindings.retain(|b| b.iri.to_string() == target);
            }

            for binding in bindings {
                let src_type = local_name(&binding.iri.to_string());
                let mut properties = Vec::new();

                // Sort constraints by predicate for deterministic property/edge order.
                let mut constraints: Vec<&ResolvedConstraint> =
                    binding.constraints.iter().collect();
                constraints.sort_by(|a, b| a.predicate.to_string().cmp(&b.predicate.to_string()));

                for c in constraints {
                    let pred_iri = c.predicate.to_string();
                    let pred_local = local_name(&pred_iri);
                    let single = c.cardinality.is_single_valued();
                    match classify_object(c, &shape_iris) {
                        ObjectKind::Literal(dt) => properties.push(VertexProperty {
                            name: pred_local,
                            data_type: data_type_name(&dt),
                            rdf_uri: Some(pred_iri),
                            xsd_datatype: Some(arrow_to_xsd(&dt)),
                            single_valued: single,
                        }),
                        ObjectKind::Edge(dst_iri) => edges.push(EdgeTable {
                            src_type: src_type.clone(),
                            predicate: pred_local.clone(),
                            dst_type: local_name(&dst_iri),
                            src_id_expr: IRI_COLUMN.to_string(),
                            // The IRI-object column conventionally carries the predicate's local
                            // name as its Extend column (e.g. `knows` → the `knows` column holds
                            // the object IRI). Used verbatim as dst_id (SINK-04). A multi-valued
                            // provider column is a LIST → UNNEST unrolls it to one edge per element.
                            dst_id_expr: if unnest_multivalued && !single {
                                format!("UNNEST(\"{pred_local}\")")
                            } else {
                                pred_local.clone()
                            },
                            single_valued: single,
                            source_relation: source_relation.to_string(),
                        }),
                        ObjectKind::Skip => {}
                    }
                }

                vertices.push(VertexTable {
                    type_name: src_type,
                    rdf_type: Some(binding.iri.to_string()),
                    vertex_id_col: IRI_COLUMN.to_string(),
                    properties,
                    source_relation: source_relation.to_string(),
                });
            }

            SinkPlan {
                vertices,
                edges,
                chunk_size,
            }
        }
    }
}

/// The object-kind classification of a single [`ResolvedConstraint`].
enum ObjectKind {
    /// Literal object — a vertex property with this arrow datatype.
    Literal(DataType),
    /// IRI object pointing at another shape — an edge to that shape (carries the dst shape IRI).
    Edge(String),
    /// Unclassifiable in v0.1 (e.g. a value-set / unsupported node constraint) — skipped.
    Skip,
}

/// Classify a constraint's `value_expr` as a literal property, an inter-shape edge, or skip.
///
/// - `NodeConstraint` carrying a `datatype` ⇒ literal property (datatype IRI → arrow type).
/// - `Ref(label)` pointing at a known shape IRI ⇒ edge.
/// - `NodeConstraint` with `nodeKind: iri` (no datatype) and a shape context ⇒ treated as an
///   opaque IRI literal (`string` property) — conservative v0.1 fallback.
fn classify_object(c: &ResolvedConstraint, shape_iris: &[String]) -> ObjectKind {
    match &c.value_expr {
        Some(ShapeExpr::Ref(label)) => {
            // A Ref points at another shape ⇒ an edge to that shape. A forward ref to a
            // not-yet-declared shape still decomposes to an edge (dst type = the label's local
            // name); `shape_iris` membership is informational only here.
            let _ = shape_iris;
            ObjectKind::Edge(shape_label_iri(label))
        }
        Some(ShapeExpr::NodeConstraint(nc)) => nc.datatype().map_or_else(
            || {
                if matches!(nc.node_kind(), Some(NodeKind::Iri)) {
                    // IRI-valued but not pointing at a declared shape: opaque IRI literal.
                    ObjectKind::Literal(DataType::Utf8)
                } else {
                    ObjectKind::Skip
                }
            },
            |dt_iri| ObjectKind::Literal(datatype_iri_to_arrow(&dt_iri.to_string())),
        ),
        // No value_expr (a bare `.` triple constraint) ⇒ treat as a string literal property — the
        // most permissive choice that still produces a column.
        None => ObjectKind::Literal(DataType::Utf8),
        // ShapeOr / ShapeAnd / ShapeNot / External / Shape are unsupported value exprs in v0.1.
        Some(_) => ObjectKind::Skip,
    }
}

/// Map an [`arrow_schema::DataType`] back to its canonical XSD datatype IRI.
///
/// The inverse of [`datatype_iri_to_arrow`], used to carry the literal's RDF
/// datatype into the manifest's output spec (the governance layer reads it).
#[must_use]
pub fn arrow_to_xsd(dt: &DataType) -> String {
    let local = match dt {
        DataType::Int64 => "integer",
        DataType::Float64 => "double",
        DataType::Boolean => "boolean",
        DataType::Date32 => "date",
        DataType::Timestamp(..) => "dateTime",
        // Utf8 + everything else is an XSD string.
        _ => "string",
    };
    format!("http://www.w3.org/2001/XMLSchema#{local}")
}

/// Map an XSD datatype IRI string to the closest [`arrow_schema::DataType`] so the manifest's
/// `data_type` spelling matches what `DuckDB` COPY writes (`RESEARCH` §"Don't Hand-Roll").
fn datatype_iri_to_arrow(iri: &str) -> DataType {
    let local = local_name(iri);
    match local.as_str() {
        "integer" | "int" | "long" => DataType::Int64,
        "decimal" | "double" | "float" => DataType::Float64,
        "boolean" => DataType::Boolean,
        "date" => DataType::Date32,
        "dateTime" => DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
        // string + everything else falls back to Utf8 (GraphAr `string`).
        _ => DataType::Utf8,
    }
}

/// Resolve a `ShapeExprLabel` to its IRI string (the edge dst shape).
pub(crate) fn shape_label_iri(label: &shex_ast::ShapeExprLabel) -> String {
    match label {
        shex_ast::ShapeExprLabel::IriRef { value } => match value {
            prefixmap::IriRef::Iri(iri) => iri.to_string(),
            prefixmap::IriRef::Prefixed { prefix, local } => format!("{prefix}:{local}"),
        },
        shex_ast::ShapeExprLabel::BNode { value } => value.to_string(),
        shex_ast::ShapeExprLabel::Start => "Start".to_string(),
    }
}

/// Extract the local name from an IRI (the segment after the last `/`, `#`, or `:`).
///
/// `"http://example.org/Person"` ⇒ `"Person"`; `"ex:knows"` ⇒ `"knows"`.
/// Public so the codegen synthesis pass (mapping → `SinkPlan`) reuses the one
/// IRI-localisation rule instead of duplicating it.
pub fn local_name(iri: &str) -> String {
    iri.rsplit_once(['/', '#', ':'])
        .map_or(iri, |(_, tail)| tail)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::DEFAULT_CHUNK_SIZE;
    use fossil_descriptors_output::{AcceptAllDescriptor, Cardinality, ShExDescriptor};

    #[test]
    fn is_single_valued_drives_collapse() {
        assert!(Cardinality::Exact(1).is_single_valued());
        assert!(Cardinality::ZeroOrOne.is_single_valued());
        assert!(!Cardinality::OneOrMore.is_single_valued());
        assert!(!Cardinality::ZeroOrMore.is_single_valued());
        assert!(Cardinality::Range {
            min: 0,
            max: Some(1)
        }
        .is_single_valued());
        assert!(!Cardinality::Range {
            min: 1,
            max: Some(5)
        }
        .is_single_valued());
        assert!(!Cardinality::Range { min: 1, max: None }.is_single_valued());
    }

    #[test]
    fn local_name_extracts_trailing_segment() {
        assert_eq!(local_name("http://example.org/Person"), "Person");
        assert_eq!(local_name("ex:knows"), "knows");
        assert_eq!(local_name("http://example.org/ns#name"), "name");
    }

    #[test]
    fn datatype_iri_maps_to_arrow() {
        assert_eq!(
            datatype_iri_to_arrow("http://www.w3.org/2001/XMLSchema#string"),
            DataType::Utf8
        );
        assert_eq!(
            datatype_iri_to_arrow("http://www.w3.org/2001/XMLSchema#integer"),
            DataType::Int64
        );
    }

    #[test]
    fn accept_all_falls_back_to_flat_passthrough() {
        let kind = OutputDescriptorKind::AcceptAll(AcceptAllDescriptor);
        let plan = vertex_edge_decomp_from_kind(&kind, PLACEHOLDER_RELATION, DEFAULT_CHUNK_SIZE, None, false);
        assert_eq!(plan.vertices.len(), 1);
        assert!(plan.edges.is_empty());
        assert_eq!(plan.vertices[0].vertex_id_col, IRI_COLUMN);
    }

    /// `ex:Person { ex:name xsd:string }` ⇒ one vertex, one property, no edges.
    #[test]
    fn single_shape_decomposes_to_one_vertex() {
        const SCHEMA: &str = r#"{
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
                  "predicate": "http://example.org/name",
                  "valueExpr": {
                    "type": "NodeConstraint",
                    "datatype": "http://www.w3.org/2001/XMLSchema#string"
                  }
                }
              }
            }
          ]
        }"#;
        let desc = ShExDescriptor::from_reader(SCHEMA.as_bytes()).expect("schema parses");
        let kind = OutputDescriptorKind::ShEx(desc);
        let plan = vertex_edge_decomp_from_kind(&kind, PLACEHOLDER_RELATION, DEFAULT_CHUNK_SIZE, None, false);
        assert_eq!(plan.vertices.len(), 1);
        assert!(plan.edges.is_empty());
        let person = &plan.vertices[0];
        assert_eq!(person.type_name, "Person");
        assert_eq!(person.properties.len(), 1);
        assert_eq!(person.properties[0].name, "name");
        assert_eq!(person.properties[0].data_type, "string");
        assert!(person.properties[0].single_valued, "Exact(1) ⇒ collapse");
    }
}
