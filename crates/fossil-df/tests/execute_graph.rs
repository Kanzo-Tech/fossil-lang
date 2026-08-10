//! E2E del backend DataFusion (paso 3, edge phase): un programa con dos
//! mappings (Person, Order) y un foreign-key template (`ex:placedBy` →
//! Person) → [`fossil_df::execute_graph`] → dos tablas vertex + una tabla edge
//! CSR/CSC con los `dense_id` resueltos por join en memoria.
//!
//! Valida la barrera C4 (todos los vértices antes de cualquier arista) y que el
//! edge-join calca el SQL del writer (`e.src_iri=s.subject`, `e.dst_iri=t.subject`,
//! CSR `ORDER BY src_dense,dst_dense` / CSC `ORDER BY dst_dense,src_dense`).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::UInt32Array;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::SessionContext;
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};

const PROGRAM: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"tests/fixtures/users.csv\")
orders := io.csv(\"tests/fixtures/orders.csv\")

Person : ex:Person from users
    iri = `${ex:}person/${.id}`
    ex:name = .name

Order : ex:Order from orders
    iri = `${ex:}order/${.order_id}`
    ex:placedBy = `${ex:}person/${.user_id}`
    ex:total = .amount
";

#[tokio::test]
async fn execute_graph_resolves_edges_to_dense_ids() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, PROGRAM.to_string(), "graph.fossil".to_string());

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .expect("execute_graph runs both phases");

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
async fn execute_graph_emits_manifests_and_run_status() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, PROGRAM.to_string(), "graph.fossil".to_string());

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .expect("execute_graph");

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

    // ── RunStatus: the wire contract keasy consumes for DCAT ──
    let status = graph.run_status("s3://bucket/job-1");
    assert_eq!(status.dest, "s3://bucket/job-1");

    let person = &status.vertices[0];
    assert_eq!(person.vertex_type, "Person");
    assert_eq!(person.file, "vertex/Person.parquet");
    assert_eq!(person.count, Some(3));
    assert_eq!(
        person.rdf_type.as_deref(),
        Some("https://example.org/Person")
    );
    let name = person
        .columns
        .iter()
        .find(|c| c.name == "name")
        .expect("Person has a name column");
    assert_eq!(name.rdf_uri.as_deref(), Some("https://example.org/name"));
    assert_eq!(
        name.xsd_datatype.as_deref(),
        Some("http://www.w3.org/2001/XMLSchema#string"),
    );

    let order = &status.vertices[1];
    assert_eq!(order.file, "vertex/Order.parquet");
    assert_eq!(order.count, Some(4));
    assert!(
        order.columns.iter().any(|c| c.name == "total"
            && c.rdf_uri.as_deref() == Some("https://example.org/total")),
        "Order carries the total property column with its predicate IRI",
    );

    let edge = &status.edges[0];
    assert_eq!(edge.edge_type, "placedBy");
    assert_eq!(
        edge.by_source,
        "edge/Order_placedBy_Person/by_source.parquet"
    );
    assert_eq!(
        edge.by_target,
        "edge/Order_placedBy_Person/by_target.parquet"
    );
    assert_eq!(edge.count, Some(4));
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
