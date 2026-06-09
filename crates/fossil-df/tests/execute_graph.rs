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

    let graph = fossil_df::execute_graph(&db, file)
        .await
        .expect("execute_graph runs both phases");

    // Two vertex types (source order: Person, Order); one edge (placedBy).
    let vtypes: Vec<&str> = graph.vertices.iter().map(|v| v.type_name.as_str()).collect();
    assert_eq!(vtypes, ["Person", "Order"]);

    assert_eq!(graph.edges.len(), 1, "only placedBy resolves to an edge");
    let edge = &graph.edges[0];
    assert_eq!(edge.edge_type, "placedBy");
    assert_eq!(edge.src_type, "Order");
    assert_eq!(edge.dst_type, "Person");
    assert_eq!(edge.rdf_uri.as_deref(), Some("https://example.org/placedBy"));

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
