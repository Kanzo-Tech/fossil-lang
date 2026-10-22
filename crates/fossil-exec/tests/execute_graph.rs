//! The `DataFusion` backend's edge phase, end to end: a program with two
//! mappings (Person, Order) and an interpolated foreign key (`placedBy` →
//! Person) → [`fossil_exec::execute_graph`] → two vertex tables and one edge
//! table, its `dense_id`s resolved by an in-memory join.
//!
//! It holds the barrier (every vertex before any edge) and the edge join's
//! order (`ORDER BY src_dense, dst_dense`).
//!
//! `placedBy` is an edge because the shape declares its range to be a shape
//! (`ex:placedBy @ex:Person` in `graph.shex`); nothing infers one from
//! templates. The checker reads the same document: an interpolation where a
//! reference is expected is the identity of a node, so one typed document
//! serves both halves. That is why this file registers `graph.shex` for the
//! checker and passes the same text as the descriptor.

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

#[tokio::test]
async fn execute_graph_resolves_edges_to_dense_ids() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);

    let ctx = SessionContext::new();
    let graph = fossil_exec::execute_graph(&ctx, &db, file)
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
async fn the_write_emits_one_information_file_per_type_and_relation() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);

    let ctx = SessionContext::new();
    let graph = fossil_exec::execute_graph(&ctx, &db, file)
        .await
        .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    let (written, files) = support::write_in_memory(&graph).await;
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "Order.vertex.yml",
            "Order_placedBy_Person.edge.yml",
            "Person.vertex.yml",
            "edge/Order_placedBy_Person/ordered_by_source/adj_list/part0/chunk0",
            "edge/Order_placedBy_Person/ordered_by_source/edge_count0",
            "edge/Order_placedBy_Person/ordered_by_source/offset/chunk0",
            "edge/Order_placedBy_Person/ordered_by_source/vertex_count",
            "fossil.graph.yml",
            "mapping.ttl",
            "vertex/Order/subject_total/chunk0",
            "vertex/Order/vertex_count",
            "vertex/Person/subject_name/chunk0",
            "vertex/Person/vertex_count",
        ],
    );

    // The documents on disk ARE the value the write answered.
    let read = |path: &str| files.get(path).cloned();
    let on_disk = support::graph(&read);
    assert_eq!(on_disk, written.info);
    assert_eq!(on_disk.name, "fossil");
    assert_eq!(on_disk.prefix, None, "the data is under the document");
    assert!(on_disk.extra_info.is_empty());
    assert_eq!(on_disk.version.to_string(), "gar/v1");

    let names: Vec<&str> = on_disk.vertices.iter().map(|v| v.r#type.as_str()).collect();
    assert_eq!(names, ["Person", "Order"], "the schema's order");
    let person = &on_disk.vertices[0];
    assert_eq!(person.prefix, "vertex/Person/");
    assert_eq!(person.property_groups.len(), 1, "one group per type");
    let columns: Vec<&str> = person.property_groups[0]
        .properties
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(columns, ["subject", "name"]);
    assert!(person.property_groups[0].properties[0].is_primary);
    assert!(!person.property_groups[0].properties[0].is_nullable);
    assert_eq!(support::vertex_count(&read, &on_disk, "Person"), 3);
    assert_eq!(support::vertex_count(&read, &on_disk, "Order"), 4);
    assert_eq!(person.chunk_size, 3, "one chunk");

    let edge = &on_disk.edges[0];
    assert_eq!(edge.triple(), "Order_placedBy_Person");
    assert_eq!(
        (
            edge.src_type.as_str(),
            edge.edge_type.as_str(),
            edge.dst_type.as_str()
        ),
        ("Order", "placedBy", "Person")
    );
    assert!(edge.directed);
    assert!(edge.property_groups.is_empty(), "an edge has no property");
    assert_eq!(edge.adj_lists.len(), 1);
    assert_eq!(
        edge.adj_lists[0].r#type,
        graphar_info::AdjListType::OrderedBySource
    );
    assert_eq!(
        (edge.src_chunk_size, edge.dst_chunk_size),
        (4, 3),
        "the chunk sizes of the end types"
    );
    assert_eq!(support::edge_count(&read, &on_disk, &edge.triple()), 4);

    // Every `user_id` in `orders.csv` is a real person, so nothing dangled —
    // and `0` is stated rather than omitted.
    assert_eq!(written.dropped.len(), 1);
    assert_eq!(written.dropped[0].table, edge.triple());
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
