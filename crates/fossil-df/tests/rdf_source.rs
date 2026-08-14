//! Host input seam (RDF): an `io.rdf` program runs end-to-end on DataFusion.
//! The host reads the Turtle bytes and registers the pivoted relation
//! ([`register_provider_sources`]); [`execute_graph`] then scans it exactly like
//! a CSV — RDF stays at the I/O border, the executor never parses it.
//!
//! Verifies the seam derives the pivot from the MIR (the `foaf:name`/`foaf:age`
//! props → relation columns), selects subjects by `rdf:type` (only the two
//! Persons, not the Org), and produces the W0b vertex shape with deterministic
//! dense ids.

#![cfg(not(target_arch = "wasm32"))]

use datafusion::arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;

mod support;

const PROGRAM: &str = "\
type { Person } := io.shex(\"rdf-person.shex\")

people := io.rdf(\"tests/fixtures/people.ttl\")

Person : Person from people
    @subject = people.subject
    name = people.name
    age = people.age
";

const RDF_PERSON_SHEX: &str = include_str!("fixtures/rdf-person.shex");

#[tokio::test]
async fn io_rdf_runs_end_to_end_via_the_host_seam() {
    let (db, file) = support::db_with_shapes(
        PROGRAM,
        "rdf.fossil",
        &[("rdf-person.shex", RDF_PERSON_SHEX)],
    );

    // The seam fossil exposes: enumerate the provider sources (MIR-derived) so
    // the host knows what to read + how to pivot it.
    let accept_all = fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT;
    let bindings =
        fossil_df::provider_bindings(&db, file, &accept_all, &std::collections::HashMap::new());
    assert_eq!(bindings.len(), 1, "one io.rdf source");
    let b = &bindings[0];
    assert_eq!(b.binding, "people");
    assert_eq!(b.uri, "tests/fixtures/people.ttl");
    assert_eq!(b.type_iri, "https://example.org/Person");
    let cols: Vec<(&str, &str)> = b
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.predicate.as_str()))
        .collect();
    assert_eq!(
        cols,
        [
            ("name", "http://xmlns.com/foaf/0.1/name"),
            ("age", "http://xmlns.com/foaf/0.1/age"),
        ],
        "pivot columns derived from the mapping's props (value ColRef + predicate IRI)"
    );

    // The host reads the bytes (here: from the filesystem) and registers the
    // decoded relation. Then the executor scans it.
    let ctx = SessionContext::new();
    fossil_df::register_provider_sources(
        &ctx,
        &db,
        file,
        &accept_all,
        &std::collections::HashMap::new(),
    )
    .expect("register io.rdf source");

    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &accept_all,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    // One vertex type; no edges. Subject selection by rdf:type drops the Org.
    let vtypes: Vec<&str> = graph.vertices.iter().map(|v| v.label.as_str()).collect();
    assert_eq!(vtypes, ["Person"]);
    assert!(graph.edges.is_empty());

    let vertex = &graph.vertices[0];
    let total: usize = vertex.batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 2, "alice + bob are Persons; acme is an Org");

    let batch = vertex.batches.first().unwrap();
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        ["dense_id", "subject", "name", "age", "x", "y", "cluster_id"]
    );

    let col = |i: usize| {
        let a = batch
            .column(i)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        (0..a.len())
            .map(|r| (!a.is_null(r)).then(|| a.value(r).to_string()))
            .collect::<Vec<_>>()
    };
    // Sorted by subject IRI (deterministic dense id): alice before bob.
    assert_eq!(
        col(1),
        [
            Some("https://example.org/alice".to_string()),
            Some("https://example.org/bob".to_string()),
        ],
    );
    assert_eq!(col(2), [Some("Alice".to_string()), Some("Bob".to_string())]);
    // Bob has no foaf:age → null.
    assert_eq!(col(3), [Some("30".to_string()), None]);
}

// ── Multi-shape + multi-valued edges driven by a ShEx descriptor ────────────

// The run_rdf.rs case at the fossil-df level: a destructuring `io.rdf` binds two
// shapes from one file; `hasProject` is a multi-valued (`*`) shape-ref →
// a typed KB→Project edge that UNNESTs to two edges (kb/1 points at two projects).
//
// The destructuring binds the SOURCE names, and `type { … } := io.shex(…)` binds
// the SHAPE names positionally against the same document — so `KB : KB from KB`
// reads type, then binding, and the two `KB`s are two namespaces, not one name
// written twice.
const KB_PROGRAM: &str = "\
type { KB, Project } := io.shex(\"kb-graph.shex\")

{ KB, Project } := io.rdf(\"tests/fixtures/kb_graph.ttl\")

KB : KB from KB
    @subject = KB.subject
    label = KB.label
    hasProject = KB.hasProject

Project : Project from Project
    @subject = Project.subject
    title = Project.title
";

const KB_GRAPH_SHEX: &str = include_str!("fixtures/kb-graph.shex");

const KB_SHEX: &str = r#"{ "@context": "http://www.w3.org/ns/shex.jsonld", "type": "Schema", "shapes": [
  {"type":"ShapeDecl","id":"https://ex.org/KB","shapeExpr":{"type":"Shape","expression":{"type":"EachOf","expressions":[
     {"type":"TripleConstraint","predicate":"https://ex.org/label","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}},
     {"type":"TripleConstraint","predicate":"https://ex.org/hasProject","valueExpr":"https://ex.org/Project","min":0,"max":-1}
  ]}}},
  {"type":"ShapeDecl","id":"https://ex.org/Project","shapeExpr":{"type":"Shape","expression":{
     "type":"TripleConstraint","predicate":"https://ex.org/title","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}}}}
] }"#;

#[tokio::test]
async fn io_rdf_shex_descriptor_yields_typed_multivalued_edges() {
    use fossil_descriptors_output::OutputDescriptorKind;
    use fossil_shex::ShExDescriptor;

    let (db, file) =
        support::db_with_shapes(KB_PROGRAM, "kb.fossil", &[("kb-graph.shex", KB_GRAPH_SHEX)]);

    let descriptor = OutputDescriptorKind::ShEx(
        ShExDescriptor::from_reader(KB_SHEX.as_bytes()).expect("parse ShEx"),
    );

    let ctx = SessionContext::new();
    fossil_df::register_provider_sources(
        &ctx,
        &db,
        file,
        &descriptor,
        &std::collections::HashMap::new(),
    )
    .expect("register both io.rdf shapes");
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &descriptor,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    // Two vertex types, each from ITS shape: KB carries `label` (NOT hasProject —
    // that's an edge), Project carries `title`.
    let kb = graph.schema.node("KB").expect("KB node");
    let kb_cols: Vec<&str> = kb.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        kb_cols,
        ["label"],
        "hasProject left KB's columns (it's an edge)"
    );
    let kb_count: usize = graph
        .vertices
        .iter()
        .find(|v| v.label == "KB")
        .map(|v| v.batches.iter().map(|b| b.num_rows()).sum())
        .expect("KB vertex");
    assert_eq!(kb_count, 1, "one KB");

    let proj_count: usize = graph
        .vertices
        .iter()
        .find(|v| v.label == "Project")
        .map(|v| v.batches.iter().map(|b| b.num_rows()).sum())
        .expect("Project vertex");
    assert_eq!(proj_count, 2, "two Projects");

    // The typed, multi-valued edge KB→Project UNNESTs to two edges.
    let edge = graph
        .edges
        .iter()
        .find(|e| e.label == "hasProject")
        .expect("hasProject edge");
    assert_eq!(
        (edge.src_type.as_str(), edge.dst_type.as_str()),
        ("KB", "Project")
    );
    let edge_count: usize = edge.by_source.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        edge_count, 2,
        "kb/1 → {{proj/1, proj/2}} unrolls to 2 edges"
    );
}
