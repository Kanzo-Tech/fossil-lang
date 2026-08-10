//! E2E del backend DataFusion (paso 3, vertex phase): `hello.fossil` →
//! [`fossil_df::execute_vertex`] → ejecuta el plan sobre un CSV real y comprueba
//! la forma GraphAr W0b materializada (dense_id + subject + props + x/y/cluster).
//! Valida MIR-PG → `LogicalPlan` → `read_csv` → `Projection`/sort → `collect()`
//! → `dense_id` denso determinista.
//!
//! El cwd de `cargo test` es la raíz del crate, así que la fuente se referencia
//! como `tests/fixtures/users.csv` (relativa a `crates/fossil-df`).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::{Array, StringArray, UInt32Array};
use datafusion::prelude::SessionContext;
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_hir::def_map::def_map;

const HELLO: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

#[tokio::test]
async fn execute_vertex_materialises_graphar_shape() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("hello.fossil must contain one mapping");

    let ctx = SessionContext::new();
    let (vertex, node) = fossil_df::execute_vertex(
        &ctx,
        &db,
        mapping,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .expect("execute_vertex runs the DataFusion plan");

    // The node's metadata is the graph-schema contract; the table holds the data.
    assert_eq!(vertex.label, "Person");
    assert_eq!(node.label, "Person");
    assert_eq!(node.iri.as_deref(), Some("https://example.org/Person"));
    assert_eq!(node.properties[0].name, "name");

    let total: usize = vertex.batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 3, "users.csv has 3 data rows");

    let batch = vertex.batches.first().expect("at least one RecordBatch");
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        ["dense_id", "subject", "name", "x", "y", "cluster_id"],
        "writer-W0b column shape",
    );

    let dense = column::<UInt32Array>(batch, 0);
    let ids: Vec<u32> = (0..dense.len()).map(|i| dense.value(i)).collect();
    assert_eq!(ids, [0, 1, 2], "dense_id = 0..N-1 in subject order");

    // Sorted by subject IRI → user/1 < user/2 < user/3, names follow.
    let subject = column::<StringArray>(batch, 1);
    assert_eq!(subject.value(0), "https://example.org/user/1");
    let name = column::<StringArray>(batch, 2);
    let values: Vec<&str> = (0..name.len()).map(|i| name.value(i)).collect();
    assert_eq!(values, ["Alice", "Bob", "Carol"]);

    // The vertex table is registered for the edge phase.
    assert!(
        ctx.table_exist("Person").unwrap(),
        "vertex table registered under its type name",
    );
}

fn column<A: Array + 'static>(
    batch: &datafusion::arrow::record_batch::RecordBatch,
    i: usize,
) -> &A {
    batch
        .column(i)
        .as_any()
        .downcast_ref::<A>()
        .expect("column has the expected array type")
}
