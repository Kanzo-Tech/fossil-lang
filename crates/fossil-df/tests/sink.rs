//! Native sink (fase 1): `execute_graph` → `write_to_dir` lays the W0b GraphAr
//! tree on disk (vertex/edge Parquet + the 3 manifest YAMLs), and the Parquet
//! reads back with the rows the executor produced.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::sync::Arc;

use datafusion::prelude::SessionContext;
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

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
async fn write_to_dir_lays_out_the_graphar_tree() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, PROGRAM.to_string(), "graph.fossil".to_string());

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(&ctx, &db, file, &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT, &std::collections::HashMap::new()).await.expect("execute_graph");

    let dir = tempfile::tempdir().expect("tempdir");
    graph.write_to_dir(dir.path()).expect("write_to_dir");

    // The full W0b tree exists.
    for rel in [
        "graph.graph.yml",
        "vertex/Person.parquet",
        "vertex/Person.vertex.yml",
        "vertex/Order.parquet",
        "vertex/Order.vertex.yml",
        "edge/Order_placedBy_Person/by_source.parquet",
        "edge/Order_placedBy_Person/by_target.parquet",
        "edge/Order_placedBy_Person/Order_placedBy_Person.edge.yml",
    ] {
        assert!(dir.path().join(rel).exists(), "missing {rel}");
    }

    // The vertex Parquet reads back with the right rows + W0b columns.
    let rows = read_parquet_rows(&dir.path().join("vertex/Person.parquet"));
    assert_eq!(rows, 3, "users.csv → 3 Person vertices");

    let yaml = fs::read_to_string(dir.path().join("vertex/Person.vertex.yml")).unwrap();
    assert!(yaml.contains("type: Person"), "{yaml}");
    assert!(yaml.contains("name: dense_id"), "{yaml}");

    // Edges round-trip: 4 orders → 4 CSR rows.
    let edges = read_parquet_rows(&dir.path().join("edge/Order_placedBy_Person/by_source.parquet"));
    assert_eq!(edges, 4);
}

fn read_parquet_rows(path: &std::path::Path) -> usize {
    let file = fs::File::open(path).expect("open parquet");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .expect("parquet reader")
        .build()
        .expect("build reader");
    reader.map(|b| b.expect("batch").num_rows()).sum()
}
