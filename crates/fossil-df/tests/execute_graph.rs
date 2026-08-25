//! E2E del backend DataFusion (paso 3, edge phase): un programa con dos
//! mappings (Person, Order) y un foreign-key interpolado (`placedBy` →
//! Person) → [`fossil_df::execute_graph`] → dos tablas vertex + una tabla edge
//! CSR/CSC con los `dense_id` resueltos por join en memoria.
//!
//! Valida la barrera C4 (todos los vértices antes de cualquier arista) y que el
//! edge-join calca el SQL del writer (`e.src_iri=s.subject`, `e.dst_iri=t.subject`,
//! CSR `ORDER BY src_dense,dst_dense` / CSC `ORDER BY dst_dense,src_dense`).

//! **This was RED and the premise has since been repaired** — the note is kept
//! because the repair is the thing worth knowing.
//!
//! `placedBy` can only be an edge if the shape declares its range to be a shape
//! (`ex:placedBy @ex:Person` in `graph.shex`); the template-skeleton guess that
//! used to infer one is deleted. `expected_value_ty` turns a shape-ref
//! constraint into an expectation of `Iri`, and an interpolation used to
//! synthesise `String` everywhere but `@subject` — so the same document that
//! made the executor emit the edge made the checker refuse the body with
//! `expected Iri, got String`. `Checker::iri_position` is now set from the
//! EXPECTATION as well as from the identity's key, `IriTemplate <: Iri`, and one
//! typed document serves both halves. That is why this file registers
//! `graph.shex` for the checker and passes the same text as the descriptor.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use datafusion::arrow::array::UInt32Array;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::SessionContext;

mod support;

const PROGRAM: &str = "\
type { Person, Order } := io.shex(\"graph.shex\")

users := io.csv(\"tests/fixtures/users.csv\")
orders := io.csv(\"tests/fixtures/orders.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Order : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = \"https://example.org/person/{orders.user_id}\"
    total = orders.amount
";

const GRAPH_SHEX: &str = include_str!("fixtures/graph.shex");

fn descriptor() -> fossil_df::OutputDescriptorKind {
    fossil_df::OutputDescriptorKind::ShEx(
        fossil_shex::ShExDescriptor::from_shex_source(GRAPH_SHEX).expect("parse graph.shex"),
    )
}

#[tokio::test]
async fn execute_graph_resolves_edges_to_dense_ids() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &descriptor(),
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    // Two vertex types (source order: Person, Order); one edge (placedBy).
    let vtypes: Vec<&str> = graph.vertices.iter().map(|v| v.label.as_str()).collect();
    assert_eq!(vtypes, ["Person", "Order"]);

    assert_eq!(graph.edges.len(), 1, "only placedBy resolves to an edge");
    let edge = &graph.edges[0];
    assert_eq!(edge.label, "placedBy");
    assert_eq!(edge.src_type, "Order");
    assert_eq!(edge.dst_type, "Person");
    // The edge's predicate IRI lives in the canonical schema, not the data table.
    let schema_edge = graph.schema.edge("placedBy").expect("placedBy in schema");
    assert_eq!(
        schema_edge.iri.as_deref(),
        Some("https://example.org/placedBy")
    );
    assert_eq!(
        (
            schema_edge.source.as_str(),
            schema_edge.destination.as_str()
        ),
        ("Order", "Person"),
    );

    // dense ids (sorted by subject IRI):
    //   Person: person/1=0, person/2=1, person/3=2
    //   Order:  order/10=0, order/11=1, order/12=2, order/13=3
    // orders.csv user_ids: 10→3, 11→1, 12→2, 13→1
    //   ⇒ (src_dense, dst_dense) = (0,2),(1,0),(2,1),(3,0)
    assert_eq!(
        pairs(&edge.by_source),
        vec![(0, 2), (1, 0), (2, 1), (3, 0)],
        "CSR: ORDER BY src_dense, dst_dense",
    );
    assert_eq!(
        pairs(&edge.by_target),
        vec![(1, 0), (3, 0), (2, 1), (0, 2)],
        "CSC: ORDER BY dst_dense, src_dense",
    );
}

#[tokio::test]
async fn execute_graph_emits_one_manifest_and_the_report_repeats_it() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &descriptor(),
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    // ── Manifests: graph index + per-type YAML, W0b paths (Type casing) ──
    let manifests = graph.manifests().expect("manifests serialize");
    let paths: Vec<&str> = manifests.iter().map(|m| m.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "graph.graph.yml",
            "vertex/Person.vertex.yml",
            "vertex/Order.vertex.yml",
            "edge/Order_placedBy_Person/Order_placedBy_Person.edge.yml",
        ],
    );
    let person_yml = &manifests[1].yaml;
    assert!(person_yml.contains("type: Person"), "{person_yml}");
    assert!(
        person_yml.contains("iri: https://example.org/Person"),
        "{person_yml}"
    );
    assert!(person_yml.contains("name: dense_id"), "{person_yml}");
    assert!(person_yml.contains("version: gar/v1"), "{person_yml}");

    // ── The report: the same manifest, plus `dest` and the drops ──
    //
    // The point of the assertions below is not that the numbers are right — the
    // YAML above already says that — it is that the JSON a host reads and the
    // YAML a reader opens are ONE value. `RunStatus` was a second account of
    // this, and a second account can disagree with the bytes.
    let report = fossil_df::RunReport::of("s3://bucket/job-1", &graph);
    assert_eq!(report.dest, "s3://bucket/job-1");

    let (graph_info, vertices, edges) = graph.manifest();
    assert_eq!(report.graph, graph_info);
    assert_eq!(report.vertices, vertices);
    assert_eq!(report.edges, edges);
    assert_eq!(
        report.graph.vertices,
        paths[1..3],
        "the index names the per-type documents, in the order the lists carry them"
    );

    let person = &report.vertices[0];
    assert_eq!(person.vertex_type, "Person");
    assert_eq!(person.prefix, "vertex/Person/");
    assert_eq!(person.vertex_count, 3);
    assert_eq!(person.iri, "https://example.org/Person");
    assert_eq!(report.vertices[1].vertex_count, 4);

    let edge = &report.edges[0];
    assert_eq!(edge.edge_type, "placedBy");
    assert_eq!(edge.prefix, "edge/Order_placedBy_Person/");
    assert_eq!(edge.edge_count, 4);
    assert_eq!(
        edge.adj_lists
            .iter()
            .map(|a| a.prefix.as_str())
            .collect::<Vec<_>>(),
        ["by_source/", "by_target/"],
        "both orientations, each saying where its tiles are",
    );

    // Every `user_id` in `orders.csv` is a real person, so nothing dangled —
    // and `0` is stated rather than omitted.
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].prefix, edge.prefix);
    assert_eq!(report.dropped[0].dropped, 0);
}

/// Flatten edge batches into `(src_dense, dst_dense)` pairs, in row order.
fn pairs(batches: &[RecordBatch]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for batch in batches {
        let src = u32_col(batch, 0);
        let dst = u32_col(batch, 1);
        for i in 0..batch.num_rows() {
            out.push((src.value(i), dst.value(i)));
        }
    }
    out
}

fn u32_col(batch: &RecordBatch, i: usize) -> &UInt32Array {
    batch
        .column(i)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("dense column is u32")
}
