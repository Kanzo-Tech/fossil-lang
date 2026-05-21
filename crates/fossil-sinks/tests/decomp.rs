//! SC#4 decomposition fixture test (SINK-01 / SINK-04 / SINK-05).
//!
//! Constructs the worked-example two-shape `ShExDescriptor`
//! (`ex:Person { ex:name xsd:string ; ex:knows @ex:Person + }` + `ex:Company { ex:legalName
//! xsd:string }`) and asserts [`vertex_edge_decomp_from_kind`] yields exactly 2 vertex tables
//! (Person, Company) + 1 edge table (Person --knows--> Person), with cardinality-driven
//! `single_valued` flags and the `iri` column used verbatim as vertex_id / src_id / dst_id.
//!
//! Also snapshots the generated inner SELECT SQL via insta — under STABLE NAMED snapshots so the
//! file is `decomp__decomp_person_vertex_select.snap` — proving the deterministic `ORDER BY`
//! (Pitfall 5) against the actual generated SQL, not source comments. The COPY wrapping + real
//! Parquet file execution is plan 05-08.
// The doc comments embed ShEx schema fragments (`ex:Person`, `xsd:string`, `ShapeExpr::Ref`)
// that trip doc_markdown — they are spec syntax, not Rust items.
#![allow(clippy::literal_string_with_formatting_args, clippy::doc_markdown)]

use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_sinks::decomp::{
    PLACEHOLDER_RELATION, edge_select_sql, vertex_edge_decomp_from_kind, vertex_select_sql,
};
use fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;

/// The SC#4 two-shape ShEx schema in JSON form:
/// `ex:Person { ex:name xsd:string ; ex:knows @ex:Person + }` + `ex:Company { ex:legalName
/// xsd:string }`. `ex:knows` carries `min=1, max=-1` (OneOrMore) and a `ShapeExpr::Ref` to
/// `ex:Person` (the edge); `ex:name`/`ex:legalName` carry an `xsd:string` datatype (literal
/// properties, default cardinality Exact(1)).
const TWO_SHAPE_SCHEMA: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "EachOf",
          "expressions": [
            {
              "type": "TripleConstraint",
              "predicate": "http://example.org/name",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            },
            {
              "type": "TripleConstraint",
              "predicate": "http://example.org/knows",
              "valueExpr": "http://example.org/Person",
              "min": 1,
              "max": -1
            }
          ]
        }
      }
    },
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Company",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/legalName",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#string"
          }
        }
      }
    }
  ]
}"#;

fn two_shape_kind() -> OutputDescriptorKind {
    let desc = ShExDescriptor::from_reader(TWO_SHAPE_SCHEMA.as_bytes()).expect("schema parses");
    assert!(
        desc.lowering_errors().is_empty(),
        "fixture must lower cleanly: {:?}",
        desc.lowering_errors()
    );
    OutputDescriptorKind::ShEx(desc)
}

#[test]
fn two_shapes_decompose_to_two_vertices_and_one_edge() {
    let kind = two_shape_kind();
    let plan = vertex_edge_decomp_from_kind(&kind, PLACEHOLDER_RELATION, DEFAULT_CHUNK_SIZE);

    // Exactly 2 vertex tables (sorted by IRI: Company, Person).
    assert_eq!(plan.vertices.len(), 2, "{plan:#?}");
    let names: Vec<&str> = plan.vertices.iter().map(|v| v.type_name.as_str()).collect();
    assert!(names.contains(&"Person"), "{names:?}");
    assert!(names.contains(&"Company"), "{names:?}");

    // Exactly 1 edge table: Person --knows--> Person.
    assert_eq!(plan.edges.len(), 1, "{plan:#?}");
    let edge = &plan.edges[0];
    assert_eq!(edge.src_type, "Person");
    assert_eq!(edge.predicate, "knows");
    assert_eq!(edge.dst_type, "Person");

    // ex:name + ex:legalName are properties (literal objects), NOT edges.
    let person = plan
        .vertices
        .iter()
        .find(|v| v.type_name == "Person")
        .expect("Person vertex");
    let company = plan
        .vertices
        .iter()
        .find(|v| v.type_name == "Company")
        .expect("Company vertex");
    assert_eq!(person.properties.len(), 1, "knows is an edge, not a prop");
    assert_eq!(person.properties[0].name, "name");
    assert_eq!(person.properties[0].data_type, "string");
    assert_eq!(company.properties.len(), 1);
    assert_eq!(company.properties[0].name, "legalName");

    // Cardinality (SINK-05): name Exact(1) -> single_valued; knows + -> multi-valued.
    assert!(
        person.properties[0].single_valued,
        "ex:name Exact(1) collapses"
    );
    assert!(!edge.single_valued, "ex:knows + keeps all edges");

    // IRI verbatim as vertex_id / src_id / dst_id (SINK-04).
    assert_eq!(person.vertex_id_col, "iri");
    assert_eq!(company.vertex_id_col, "iri");
    assert_eq!(edge.src_id_expr, "iri");
    assert_eq!(edge.dst_id_expr, "knows");
}

/// Named snapshot of the Person vertex inner SELECT — proves the deterministic `ORDER BY`
/// (Pitfall 5) against the actual generated SQL. Snapshot file:
/// `tests/snapshots/decomp__decomp_person_vertex_select.snap`.
#[test]
fn person_vertex_select_snapshot() {
    let kind = two_shape_kind();
    let plan = vertex_edge_decomp_from_kind(&kind, PLACEHOLDER_RELATION, DEFAULT_CHUNK_SIZE);
    let person = plan
        .vertices
        .iter()
        .find(|v| v.type_name == "Person")
        .expect("Person vertex");
    let sql = vertex_select_sql(person);
    assert!(
        sql.contains("ORDER BY"),
        "deterministic ORDER BY required: {sql}"
    );
    assert!(
        sql.contains("DISTINCT ON (iri)"),
        "Exact(1) collapses: {sql}"
    );
    insta::assert_snapshot!("decomp_person_vertex_select", sql);
}

/// Named snapshot of the Person--knows-->Person edge inner SELECT (deterministic ORDER BY).
#[test]
fn person_knows_edge_select_snapshot() {
    let kind = two_shape_kind();
    let plan = vertex_edge_decomp_from_kind(&kind, PLACEHOLDER_RELATION, DEFAULT_CHUNK_SIZE);
    let edge = &plan.edges[0];
    let sql = edge_select_sql(edge);
    assert!(
        sql.contains("ORDER BY src_id, dst_id"),
        "deterministic ORDER BY required: {sql}"
    );
    assert!(
        !sql.contains("DISTINCT"),
        "OneOrMore edge keeps all rows: {sql}"
    );
    insta::assert_snapshot!("decomp_person_knows_edge_select", sql);
}
