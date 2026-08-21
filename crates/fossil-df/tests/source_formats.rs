//! Source-format parity: the executor reads `io.json` (and `io.parquet`) the
//! same as `io.csv`, dispatching on the MIR `SourceFormat`. A `Provider` source
//! (RDF) is host-decoded + scanned via the input seam — see `rdf_source.rs`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use datafusion::arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;

mod support;

const JSON_PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")

users := io.json(\"tests/fixtures/users.json\")

User : Person from users
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
";

/// The same three records, as the shape a `.json` file ordinarily holds.
const JSON_ARRAY_PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")

users := io.json(\"tests/fixtures/users-array.json\")

User : Person from users
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
";

const PERSON_SHEX: &str = include_str!("fixtures/person-name.shex");

/// Both shapes of JSON, and the point is that they answer the SAME.
///
/// `DataFusion`'s `read_json` is newline-delimited only, and the array form
/// failed with `Json error: Not valid JSON: EOF while parsing a list` — which
/// is the `sightings` conformance program, and which would have made `io.json`
/// silently mean NDJSON. Two fixtures, one relation.
///
/// The array path materialises and the NDJSON path streams, and that asymmetry
/// is the format's: you cannot find the end of record N in an array without
/// parsing from the start. It is not asserted here because a batch count is not
/// where that shows.
async fn three_users_from(program: &str, name: &str) -> Vec<String> {
    let (db, file) = support::db_with_shapes(program, name, &[("person.shex", PERSON_SHEX)]);
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
    .unwrap_or_else(|e| panic!("{name}: {e}; {:#?}", support::diagnostics(&db, file)));
    assert_eq!(node.label, "Person");
    vertex
        .batches
        .iter()
        .flat_map(|b| {
            let col = b
                .column(2)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("name col")
                .clone();
            (0..col.len()).map(move |i| col.value(i).to_string())
        })
        .collect()
}

#[tokio::test]
async fn a_json_array_and_ndjson_read_the_same() {
    let ndjson = three_users_from(JSON_PROGRAM, "nd.fossil").await;
    let array = three_users_from(JSON_ARRAY_PROGRAM, "array.fossil").await;
    assert_eq!(ndjson, ["Alice", "Bob", "Carol"], "the streaming spelling");
    assert_eq!(array, ndjson, "and the array says the same");
}

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
