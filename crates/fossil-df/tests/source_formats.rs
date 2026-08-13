//! Source-format parity: the executor reads `io.json` (and `io.parquet`) the
//! same as `io.csv`, dispatching on the MIR `SourceFormat`. A `Provider` source
//! (RDF) is host-decoded + scanned via the input seam — see `rdf_source.rs`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use datafusion::arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;

mod support;

const JSON_PROGRAM: &str = "\
prefix ex: <https://example.org/>
type { Person } = io.shex(\"person.shex\")

users := io.json(\"tests/fixtures/users.json\")

User : ex:Person from users
    @subject = `${ex:}user/${.id}`
    name = .name
";

const PERSON_SHEX: &str = include_str!("fixtures/person-name.shex");

#[tokio::test]
async fn reads_an_ndjson_source() {
    let (db, file) =
        support::db_with_shapes(JSON_PROGRAM, "json.fossil", &[("person.shex", PERSON_SHEX)]);
    let mapping = *fossil_hir::def_map::def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");

    let ctx = SessionContext::new();
    let (vertex, node) = fossil_df::execute_vertex(
        &ctx,
        &db,
        mapping,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "io.json reads via read_json: {e}; {:#?}",
            support::diagnostics(&db, file)
        )
    });

    assert_eq!(node.label, "Person");
    let total: usize = vertex.batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 3, "users.json has 3 records");

    let batch = vertex.batches.first().unwrap();
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        ["dense_id", "subject", "name", "x", "y", "cluster_id"]
    );

    let name = batch
        .column(2)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("name col");
    let values: Vec<&str> = (0..name.len()).map(|i| name.value(i)).collect();
    assert_eq!(values, ["Alice", "Bob", "Carol"]);
}
