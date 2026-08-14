//! Native gate for the browser executor core: a CSV program staged through the
//! in-memory object-store seam runs end-to-end on `DataFusion` and yields the
//! W0b `GraphAr` files + a `RunStatus`. The wasm node smoke (B2) re-runs this exact
//! flow through the `#[wasm_bindgen]` wrapper to prove `execute_graph().collect()`
//! works under wasm-bindgen-futures.

//! **One of these used to pass for the wrong reason, and `rdf_uri` is what says
//! it no longer does.**
//!
//! `fossil-df-wasm`'s `ExecutorSystem` installed no shape decoder and
//! `build_program` registered no shape document — both deliberate, and both
//! written before ruling 3 of 2026-08-11. So `resolve_target_shape` answered
//! `Unregistered` for the `executor.shex` the program names; that is
//! informational, NOT fatal, so the mapping still compiled — with an EMPTY
//! predicate table. Measured on 2026-08-12:
//!
//! ```text
//! RunStatus vertex Person → columns = [("name", None)]
//! ```
//!
//! The column kept the bare name the author wrote and LOST its predicate IRI,
//! and nothing here asserted `rdf_uri`, which is why it went green. Two things
//! broke silently downstream: keasy's DCAT (`rdf_uri` is the wire contract's
//! whole point) and every edge, because `apply_output_shape` classifies on
//! `p.rdf_uri` and `None` matches no predicate. The `shex` ARGUMENT cannot
//! supply either: a bare property key means the last segment of a predicate IRI
//! a shape declares, so the IRI comes from `TypeckOutput.predicates`, which
//! comes from the REGISTERED document — and once the header stopped carrying its
//! own CURIE, so did the vertex LABEL, which is how this finally became loud
//! (`vertex/.parquet`).
//!
//! `build_program` now registers the one text it holds under the name the
//! program writes. The assertion on `rdf_uri` below is the guard: it is the
//! cheapest thing that distinguishes "the document was read" from "the mapping
//! compiled anyway".

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use fossil_df_wasm::{SourceInput, SourceKind, execute_core, program_sources_core};

/// The schema the browser fetched and hands to the executor. It is the SAME
/// text the program names, and that is the point: this host has one shape and
/// two consumers of it.
const EXECUTOR_SHEX: &str = include_str!("fixtures/executor.shex");

const PROGRAM: &str = "\
type { Person, Order } := io.shex(\"executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name
";

#[tokio::test]
async fn csv_program_runs_through_the_in_memory_source_seam() {
    let bytes = std::fs::read("../fossil-df/tests/fixtures/users.csv").expect("fixture");
    let sources = vec![SourceInput {
        uri: "https://data.example.com/users.csv".to_string(),
        format: SourceKind::Csv,
        bytes,
    }];

    let out = execute_core(
        PROGRAM,
        Some(EXECUTOR_SHEX),
        sources,
        "s3://jobs/run-1",
        &empty_refs(),
    )
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

    // The document was READ, not merely named: `name` is a bare key, so its
    // predicate IRI exists only if `executor.shex` reached the checker. `None`
    // here is what a run that skipped registration produced, and it is
    // indistinguishable from success everywhere else in this file.
    assert_eq!(
        v.rdf_type.as_deref(),
        Some("https://example.org/Person"),
        "the vertex's type IRI comes from the registered document"
    );
    let name = v
        .columns
        .iter()
        .find(|c| c.name == "name")
        .expect("Person carries the name column");
    assert_eq!(
        name.rdf_uri.as_deref(),
        Some("https://example.org/name"),
        "a bare key's predicate IRI comes from the registered document"
    );
}

const TWO_SOURCE_PROGRAM: &str = "\
type { Person, Order } := io.shex(\"executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")
orders := io.csv(\"https://data.example.com/orders.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Order : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = \"https://example.org/person/{orders.user_id}\"
";

#[test]
fn program_sources_lists_each_distinct_source_with_its_format() {
    let srcs = program_sources_core(TWO_SOURCE_PROGRAM, Some(EXECUTOR_SHEX), &empty_refs())
        .expect("sources enumerated");
    let uris: Vec<&str> = srcs.iter().map(|(u, _)| u.as_str()).collect();
    assert!(uris.contains(&"https://data.example.com/users.csv"));
    assert!(uris.contains(&"https://data.example.com/orders.csv"));
    assert_eq!(srcs.len(), 2);
    assert!(srcs.iter().all(|(_, fmt)| *fmt == "csv"));
}

const CONN_PROGRAM: &str = "\
type { Person, Order } := io.shex(\"executor.shex\")

users := io.csv(\"@mybucket/users.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name
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

    let out = execute_core(
        CONN_PROGRAM,
        Some(EXECUTOR_SHEX),
        sources,
        "s3://jobs/run-1",
        &refs,
    )
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
