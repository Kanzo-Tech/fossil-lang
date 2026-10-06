//! E2E del backend `DataFusion` (paso 3, edge phase): un programa con dos
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
    fossil_df::OutputDescriptorKind::Lowered(
        fossil_shex::ShExDescriptor::from_shex_source(GRAPH_SHEX)
            .expect("parse graph.shex")
            .to_graph_schema(&fossil_graph_schema::Renames::default()),
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

    // dense ids, global — the types in schema order, each one contiguous
    // range, subject order inside it:
    //   Person: person/1=0, person/2=1, person/3=2
    //   Order:  order/10=3, order/11=4, order/12=5, order/13=6
    // orders.csv user_ids: 10→3, 11→1, 12→2, 13→1
    //   ⇒ (src_dense, dst_dense) = (3,2),(4,0),(5,1),(6,0)
    assert_eq!(
        pairs(&edge.batches),
        vec![(3, 2), (4, 0), (5, 1), (6, 0)],
        "global ids, ORDER BY src_dense, dst_dense",
    );
}

#[tokio::test]
async fn the_write_emits_one_manifest_over_one_table_per_type_and_relation() {
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

    let (written, files) = support::write_in_memory(&graph).await;
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "edge/Order_placedBy_Person.parquet",
            "fossil.json",
            "vertex/Order.parquet",
            "vertex/Person.parquet",
        ],
    );

    // The document on disk IS the value the write answered.
    let on_disk: fossil_sinks::manifest::Manifest =
        serde_json::from_slice(&files["fossil.json"]).expect("fossil.json parses");
    assert_eq!(on_disk, written.manifest);
    let text = String::from_utf8(files["fossil.json"].clone()).expect("UTF-8");
    assert!(text.contains("\"format\": \"fossil/1\""), "{text}");

    let m = &written.manifest;
    let names: Vec<&str> = m.vertex_tables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Person", "Order"], "the schema's order");
    let person = &m.vertex_tables[0];
    assert_eq!(person.path, "vertex/Person.parquet");
    assert_eq!(person.iri.as_deref(), Some("https://example.org/Person"));
    assert_eq!(
        (person.key.as_str(), person.identity.as_str()),
        ("dense_id", "subject")
    );
    assert_eq!(person.record_count, 3);
    assert_eq!(m.vertex_tables[1].record_count, 4);
    let columns: Vec<&str> = person.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(&columns[..2], ["dense_id", "subject"]);
    assert!(columns.contains(&"name"), "{columns:?}");

    let edge = &m.edge_tables[0];
    assert_eq!(edge.name, "Order_placedBy_Person");
    assert_eq!(edge.label, "placedBy");
    assert_eq!(edge.path, "edge/Order_placedBy_Person.parquet");
    assert_eq!(edge.iri.as_deref(), Some("https://example.org/placedBy"));
    assert_eq!(
        (edge.source.key.as_str(), edge.source.references.as_str()),
        ("src", "Order")
    );
    assert_eq!(
        (
            edge.destination.key.as_str(),
            edge.destination.references.as_str()
        ),
        ("dst", "Person")
    );
    assert_eq!(edge.record_count, 4);

    // Every `user_id` in `orders.csv` is a real person, so nothing dangled —
    // and `0` is stated rather than omitted.
    assert_eq!(written.dropped.len(), 1);
    assert_eq!(written.dropped[0].table, edge.name);
    assert_eq!(written.dropped[0].dropped, 0);
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
