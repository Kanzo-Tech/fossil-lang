//! Native gate for the browser executor core: a CSV program staged through the
//! in-memory object-store seam runs end-to-end on `DataFusion` and yields the
//! W0b `GraphAr` files + a `RunStatus`. The wasm node smoke (B2) re-runs this exact
//! flow through the `#[wasm_bindgen]` wrapper to prove `execute_graph().collect()`
//! works under wasm-bindgen-futures.

#![cfg(not(target_arch = "wasm32"))]

use fossil_df_wasm::{execute_core, SourceInput, SourceKind};

const PROGRAM: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"https://data.example.com/users.csv\")

Person : ex:Person from users
    iri = `${ex:}person/${.id}`
    ex:name = .name
";

#[tokio::test]
async fn csv_program_runs_through_the_in_memory_source_seam() {
    let bytes = std::fs::read("../fossil-df/tests/fixtures/users.csv").expect("fixture");
    let sources = vec![SourceInput {
        uri: "https://data.example.com/users.csv".to_string(),
        format: SourceKind::Csv,
        bytes,
    }];

    let out = execute_core(PROGRAM, None, sources, "s3://jobs/run-1")
        .await
        .expect("executor runs the CSV program");

    // The W0b vertex Parquet + the three manifest YAMLs are produced as bytes.
    let paths: Vec<&str> = out.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert!(
        paths.contains(&"vertex/Person.parquet"),
        "expected a Person vertex parquet, got {paths:?}"
    );
    assert!(paths.contains(&"graph.graph.yml"));
    assert!(paths.contains(&"vertex/Person.vertex.yml"));

    // The Person parquet is non-empty (the encoder wrote a real file).
    let person = out
        .files
        .iter()
        .find(|f| f.rel_path == "vertex/Person.parquet")
        .unwrap();
    assert!(!person.bytes.is_empty());

    // RunStatus carries the vertex + its row count (3 users → 3 vertices).
    assert_eq!(out.run_status.dest, "s3://jobs/run-1");
    assert_eq!(out.run_status.vertices.len(), 1);
    let v = &out.run_status.vertices[0];
    assert_eq!(v.vertex_type, "Person");
    assert_eq!(v.count, Some(3));
}
