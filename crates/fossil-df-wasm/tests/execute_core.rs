//! Native gate for the browser executor core: a CSV program staged through the
//! in-memory object-store seam runs end-to-end on `DataFusion` and yields the
//! W0b `GraphAr` files + a `RunStatus`. The wasm node smoke (B2) re-runs this exact
//! flow through the `#[wasm_bindgen]` wrapper to prove `execute_graph().collect()`
//! works under wasm-bindgen-futures.

//! ⚠️ **These three pass, and one of them passes for the wrong reason.**
//!
//! `fossil-df-wasm`'s `ExecutorSystem` installs no shape decoder and
//! `build_program` registers no shape document — both deliberate, and both
//! written before ruling 3 of 2026-08-11. So `resolve_target_shape` answers
//! `Unregistered` for the `executor.shex` the program names; that is
//! informational, NOT fatal, so the mapping still compiles — with an EMPTY
//! predicate table. Measured here on 2026-08-12:
//!
//! ```text
//! RunStatus vertex Person → columns = [("name", None)]
//! ```
//!
//! The column keeps the bare name the author wrote and LOSES its predicate IRI.
//! Nothing in this file asserts `rdf_uri`, which is why it goes green. Two
//! things break silently downstream: keasy's DCAT (`rdf_uri` is the wire
//! contract's whole point) and every edge, because `apply_output_shape`
//! classifies on `p.rdf_uri` and `None` matches no predicate. The `shex`
//! ARGUMENT cannot supply it: a bare property key means the last segment of a
//! predicate IRI a shape declares, so the IRI comes from
//! `TypeckOutput.predicates`, which comes from the REGISTERED document.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use fossil_df_wasm::{SourceInput, SourceKind, execute_core, program_sources_core};

/// The schema the browser fetched and hands to the executor. It is the SAME
/// text the program names, and that is the point: this host has one shape and
/// two consumers of it.
const EXECUTOR_SHEX: &str = include_str!("fixtures/executor.shex");

const PROGRAM: &str = "\
prefix ex: <https://example.org/>
type { Person, Order } = io.shex(\"executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")

Person : ex:Person from users
    @subject = `${ex:}person/${.id}`
    name = .name
";

#[tokio::test]
async fn csv_program_runs_through_the_in_memory_source_seam() {
    let bytes = std::fs::read("../fossil-df/tests/fixtures/users.csv").expect("fixture");
    let sources = vec![SourceInput {
        uri: "https://data.example.com/users.csv".to_string(),
        format: SourceKind::Csv,
        bytes,
    }];

    let out = execute_core(PROGRAM, Some(EXECUTOR_SHEX), sources, "s3://jobs/run-1", &empty_refs())
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

const TWO_SOURCE_PROGRAM: &str = "\
prefix ex: <https://example.org/>
type { Person, Order } = io.shex(\"executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")
orders := io.csv(\"https://data.example.com/orders.csv\")

Person : ex:Person from users
    @subject = `${ex:}person/${.id}`
    name = .name

Order : ex:Order from orders
    @subject = `${ex:}order/${.order_id}`
    placedBy = `${ex:}person/${.user_id}`
";

#[test]
fn program_sources_lists_each_distinct_source_with_its_format() {
    let srcs =
        program_sources_core(TWO_SOURCE_PROGRAM, Some(EXECUTOR_SHEX), &empty_refs()).expect("sources enumerated");
    let uris: Vec<&str> = srcs.iter().map(|(u, _)| u.as_str()).collect();
    assert!(uris.contains(&"https://data.example.com/users.csv"));
    assert!(uris.contains(&"https://data.example.com/orders.csv"));
    assert_eq!(srcs.len(), 2);
    assert!(srcs.iter().all(|(_, fmt)| *fmt == "csv"));
}

const CONN_PROGRAM: &str = "\
prefix ex: <https://example.org/>
type { Person, Order } = io.shex(\"executor.shex\")

users := io.csv(\"@mybucket/users.csv\")

Person : ex:Person from users
    @subject = `${ex:}person/${.id}`
    name = .name
";

#[tokio::test]
async fn at_conn_source_alias_resolves_through_the_ref_map() {
    // `@mybucket/users.csv` resolves to `{base}/users.csv` via the ref-map —
    // both `sources()` (enumeration) and `run()` (staging + read) must agree.
    let mut refs = std::collections::HashMap::new();
    refs.insert(
        "mybucket".to_string(),
        "https://data.example.com".to_string(),
    );

    let listed = program_sources_core(CONN_PROGRAM, Some(EXECUTOR_SHEX), &refs).expect("sources");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "https://data.example.com/users.csv");

    let resolved_uri = &listed[0].0;
    let sources = vec![SourceInput {
        uri: resolved_uri.clone(),
        format: SourceKind::Csv,
        bytes: std::fs::read("../fossil-df/tests/fixtures/users.csv").expect("fixture"),
    }];

    let out = execute_core(CONN_PROGRAM, Some(EXECUTOR_SHEX), sources, "s3://jobs/run-1", &refs)
        .await
        .expect("executor runs the @conn-aliased program");
    let person = out
        .run_status
        .vertices
        .iter()
        .find(|v| v.vertex_type == "Person");
    assert_eq!(person.and_then(|v| v.count), Some(3));
}

fn empty_refs() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}
