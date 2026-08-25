//! Native sink (fase 1): `execute_graph` → `write_to_dir` lays the W0b GraphAr
//! tree on disk (vertex/edge Parquet + the 3 manifest YAMLs), and the Parquet
//! reads back with the rows the executor produced.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use std::fs;

use datafusion::prelude::SessionContext;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

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

/// The document the program names — shared with `execute_graph.rs`, and the
/// same text goes to the executor as the descriptor. `ex:placedBy @ex:Person`
/// is what makes `placedBy` an edge; under `ACCEPT_ALL_DEFAULT` it degrades to
/// a string column and the whole `edge/` half of the tree asserted below
/// silently stops existing.
const GRAPH_SHEX: &str = include_str!("fixtures/graph.shex");

#[tokio::test]
async fn write_to_dir_lays_out_the_graphar_tree() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);
    let descriptor = fossil_df::OutputDescriptorKind::ShEx(
        fossil_shex::ShExDescriptor::from_shex_source(GRAPH_SHEX).expect("parse graph.shex"),
    );

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &descriptor,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

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

    // **`is_primary` marks exactly one property and it is `subject`.**
    //
    // Read back through the structs rather than by substring, because the
    // question is which property carries the flag and a `contains` cannot say:
    // `is_primary: true` is in this file either way. Asserted here because this
    // is the test that holds the bytes a run wrote, and the flag is a claim
    // about the artefact.
    //
    // Nothing in the tree READS it — `packages/graph`'s reader keys on the
    // column name, and it says why: it could not trust a field two writers
    // spelled two ways. So this is the only thing that goes red if `dense_id`
    // takes the flag back, and it is deliberately the artefact's own manifest
    // and not `vertex_info`'s return value.
    let info: fossil_sinks::manifest::VertexInfo =
        serde_yaml_ng::from_str(&yaml).unwrap_or_else(|e| panic!("Person.vertex.yml: {e}\n{yaml}"));
    let primary: Vec<&str> = info
        .property_groups
        .iter()
        .flat_map(|g| &g.properties)
        .filter(|p| p.is_primary)
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        primary,
        ["subject"],
        "the identity is the subject IRI; dense_id is an address a re-layout gives away\n{yaml}"
    );

    // Edges round-trip: 4 orders → 4 CSR rows.
    let edges = read_parquet_rows(
        &dir.path()
            .join("edge/Order_placedBy_Person/by_source.parquet"),
    );
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
