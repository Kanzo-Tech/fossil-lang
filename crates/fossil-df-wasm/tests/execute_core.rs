//! Native gate for the browser executor core: a CSV program staged through the
//! in-memory object-store seam runs end-to-end on `DataFusion` and yields the
//! `GraphAr` files + a `RunReport`. `packages/executor/tests/execute.test.ts`
//! drives the same core through the `#[wasm_bindgen]` wrapper under Node, which
//! is where wasm-bindgen-futures is proved.

//! **One of these used to pass for the wrong reason, and the shape IRI is what
//! says it no longer does.**
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
//! and nothing here asserted it, which is why it went green. Two things broke
//! silently downstream: keasy's DCAT and every edge, because
//! `apply_output_shape` classifies on `p.rdf_uri` and `None` matches no
//! predicate. The `shex` ARGUMENT cannot supply either: a bare property key
//! means the last segment of a predicate IRI a shape declares, so the IRI comes
//! from `TypeckOutput.predicates`, which comes from the REGISTERED document —
//! and once the header stopped carrying its own CURIE, so did the vertex LABEL,
//! which is how this finally became loud (`vertex/.parquet`).
//!
//! `build_program` now registers the one text it holds under the name the
//! program writes. The assertion on `VertexInfo::iri` below is the guard: it is
//! the cheapest thing that distinguishes "the document was read" from "the
//! mapping compiled anyway".
//!
//! **It is a weaker guard than the one it replaces, and that is a fact about
//! the manifest, not about this file.** `RunStatus` carried a `rdf_uri` per
//! COLUMN; `fossil_sinks::manifest::Property` carries `name`, `data_type`,
//! `is_primary` and `is_nullable` and no predicate. So the empty shape IRI is
//! what is checkable here, and a shape that resolved its type IRI while losing a
//! property's would pass. The two came from the same registration and failed
//! together when they failed; nothing enforces that they still would.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use fossil_df_wasm::{SourceInput, execute_core, program_sources_core, source_row};

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
        format: source_row("csv").expect("the csv row"),
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

    // The TILED tree, which is the one `fossil run` writes: the tiles under the
    // declared prefix, the identity index beside them, and the staged
    // single-file payload GONE — left in, it is a second, stale copy of every
    // vertex and `apps/corpus`'s `exactly-once` fails a corpus for it.
    let paths: Vec<&str> = out.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert!(
        paths.contains(&"vertex/Person/tiles.parquet"),
        "expected the tiled Person payload, got {paths:?}"
    );
    assert!(
        paths.contains(&"vertex/Person/index/tiles.parquet"),
        "expected the identity index beside it, got {paths:?}"
    );
    assert!(
        !paths.contains(&"vertex/Person.parquet"),
        "the staged vertex Parquet is the pass's INPUT and must not be published: {paths:?}"
    );
    assert!(paths.contains(&"graph.graph.yml"));
    assert!(paths.contains(&"vertex/Person.vertex.yml"));

    let person = out
        .files
        .iter()
        .find(|f| f.rel_path == "vertex/Person/tiles.parquet")
        .unwrap();
    assert!(!person.bytes.is_empty());

    // 3 users → 3 vertices.
    assert_eq!(out.report.dest, "s3://jobs/run-1");
    assert_eq!(out.report.vertices.len(), 1);
    let v = &out.report.vertices[0];
    assert_eq!(v.vertex_type, "Person");
    assert_eq!(v.vertex_count, 3);

    // The document was READ, not merely named: a bare header name is bound
    // positionally against the shape document, so this IRI exists only if
    // `executor.shex` reached the checker. The empty string is what a run that
    // skipped registration produced, and it is indistinguishable from success
    // everywhere else in this file.
    assert_eq!(
        v.iri, "https://example.org/Person",
        "the vertex's type IRI comes from the registered document"
    );
    assert!(
        v.property_groups[0]
            .properties
            .iter()
            .any(|p| p.name == "name"),
        "Person carries the name column: {:?}",
        v.property_groups,
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
    // The wire `format` round-trips: what `sources()` emits is a catalogue row
    // name and `source_row` reads it back, which is the half of the host
    // contract nothing else looks at.
    assert_eq!(listed[0].1, "csv");
    assert_eq!(
        source_row(&listed[0].1)
            .expect("the emitted name is a row")
            .name,
        "csv"
    );

    let resolved_uri = &listed[0].0;
    let sources = vec![SourceInput {
        uri: resolved_uri.clone(),
        format: source_row("csv").expect("the csv row"),
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
        .report
        .vertices
        .iter()
        .find(|v| v.vertex_type == "Person");
    assert_eq!(person.map(|v| v.vertex_count), Some(3));
}

fn empty_refs() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}
