//! Native gate for the browser executor core: a CSV program staged through the
//! in-memory object-store seam runs end-to-end on `DataFusion` and yields the
//! `GraphAr` files + a `RunReport`. `packages/executor/tests/execute.test.ts`
//! drives the same core through the `#[wasm_bindgen]` wrapper under Node, which
//! is where wasm-bindgen-futures is proved.

//! The shape document reaches the executor the way it reaches the checker —
//! reported by `missing_documents`, registered under its key — and the run's
//! output descriptor is decoded from that registration. The assertion on
//! `VertexInfo::iri` is the guard that the document was READ: a bare header
//! name is bound positionally against it, and a run that skipped it writes an
//! empty type IRI.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use std::collections::HashMap;

use fossil_df_wasm::{Executor, SourceInput, source_row};

/// The document every program below names.
const EXECUTOR_SHEX: &str = include_str!("fixtures/executor.shex");

/// `program` compiled, with every document it names registered — what
/// `resolveDocuments` leaves behind.
fn executor(program: &str, connections: HashMap<String, String>) -> Executor {
    let mut exec = Executor::new(program);
    exec.set_connections(connections);
    for missing in exec.missing_documents() {
        exec.register_document(&missing.key, EXECUTOR_SHEX);
    }
    assert!(exec.missing_documents().is_empty());
    exec
}

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

    let out = executor(PROGRAM, HashMap::new())
        .execute(sources, "s3://jobs/run-1")
        .await
        .expect("executor runs the CSV program");

    // The TILED tree, which is the one `fossil run` writes: the tiles under the
    // declared prefix, the identity index beside them, and the staged
    // single-file payload GONE — it is never written now, where it used to be
    // written, read by the pass and removed from the map. Left in, it is a
    // second, stale copy of every vertex and `apps/corpus`'s `exactly-once`
    // fails a corpus for it.
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
        "nothing stages a single-file vertex payload; the tiles are the first write: {paths:?}"
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
        v.properties().iter().any(|p| p.name == "name"),
        "Person carries the name column: {:?}",
        v.projections,
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
    let srcs = executor(TWO_SOURCE_PROGRAM, HashMap::new())
        .sources()
        .expect("sources enumerated");
    let uris: Vec<&str> = srcs.iter().map(|(u, _)| u.as_str()).collect();
    assert!(uris.contains(&"https://data.example.com/users.csv"));
    assert!(uris.contains(&"https://data.example.com/orders.csv"));
    assert_eq!(srcs.len(), 2);
    assert!(srcs.iter().all(|(_, fmt)| *fmt == "csv"));
}

/// **The browser's report IS the documents the browser shipped** — every
/// manifest, field for field, checked against the YAML in the same
/// `ExecOutput`.
///
/// It is written as an equality over the whole set and not as an assertion
/// about two named fields, because the defect it guards is not about a field.
/// `RunReport::of` snapshots `graph.manifest()`, so a report built at the wrong
/// moment states the manifest as it was THEN: every key declared afterwards is
/// simply absent from the JSON, with nothing anywhere going red. That is how the
/// browser came to omit `cells:` and then `channels:` — the report was built
/// before `declare_pyramids`/`declare_channels`, so the tab handed its host an
/// account of a corpus missing two measured facts the YAML it uploaded beside it
/// carried, while `fossil run --output-json` carried both. A test naming those
/// two fields would have caught those two fields; this one catches the third.
///
/// The two-source program is used so neither half is vacuous: it materialises a
/// vertex document and an edge document, and `Person` is a type the layout pass
/// reaches — so the report has something to lose.
#[tokio::test]
async fn the_report_is_the_manifest_the_browser_shipped() {
    let sources = vec![
        SourceInput {
            uri: "https://data.example.com/users.csv".to_string(),
            format: source_row("csv").expect("the csv row"),
            bytes: std::fs::read("../fossil-df/tests/fixtures/users.csv").expect("fixture"),
        },
        SourceInput {
            uri: "https://data.example.com/orders.csv".to_string(),
            format: source_row("csv").expect("the csv row"),
            bytes: std::fs::read("../fossil-df/tests/fixtures/orders.csv").expect("fixture"),
        },
    ];
    let out = executor(TWO_SOURCE_PROGRAM, HashMap::new())
        .execute(sources, "s3://jobs/run-1")
        .await
        .expect("executor runs the two-source program");

    let shipped = |rel: &str| -> String {
        let file = out
            .files
            .iter()
            .find(|f| f.rel_path == rel)
            .unwrap_or_else(|| panic!("the run emitted `{rel}`"));
        String::from_utf8(file.bytes.clone()).expect("a manifest is UTF-8")
    };

    // The index, then every document it names — positionally, which is the
    // agreement `GraphInfo::vertices`/`edges` already carry.
    assert_eq!(
        shipped("graph.graph.yml"),
        out.report.graph.to_yaml().expect("the index serialises"),
        "the report's index is not the `graph.graph.yml` the run shipped"
    );
    assert!(!out.report.vertices.is_empty() && !out.report.edges.is_empty());
    for (rel, info) in out.report.graph.vertices.iter().zip(&out.report.vertices) {
        assert_eq!(
            shipped(rel),
            info.to_yaml().expect("a vertex document serialises"),
            "the report's entry for `{rel}` is not the document the run shipped"
        );
    }
    for (rel, info) in out.report.graph.edges.iter().zip(&out.report.edges) {
        assert_eq!(
            shipped(rel),
            info.to_yaml().expect("an edge document serialises"),
            "the report's entry for `{rel}` is not the document the run shipped"
        );
    }

    // And the equality is not vacuous on the field that found this: the pass
    // partitioned `Person`, so its document declares a channel with a measured
    // domain — and therefore so must the report.
    let person = out
        .report
        .vertices
        .iter()
        .find(|v| v.vertex_type == "Person")
        .expect("Person is in the report");
    let channels = person
        .channels
        .as_ref()
        .expect("the pass measured a channel for Person, so the report declares one");
    assert!(
        channels.iter().any(|c| c.domain.is_some()),
        "a categorical channel carries the domain the pass measured: {channels:?}"
    );
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
    let exec = executor(
        CONN_PROGRAM,
        HashMap::from([(
            "mybucket".to_string(),
            "https://data.example.com".to_string(),
        )]),
    );

    let listed = exec.sources().expect("sources");
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

    let out = exec
        .execute(sources, "s3://jobs/run-1")
        .await
        .expect("executor runs the @conn-aliased program");
    let person = out
        .report
        .vertices
        .iter()
        .find(|v| v.vertex_type == "Person");
    assert_eq!(person.map(|v| v.vertex_count), Some(3));
}

const CONN_DOCUMENT_PROGRAM: &str = "\
type { Person, Order } := io.shex(\"@vocab/executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name
";

/// A document is keyed by what the program wrote and located through the
/// connection map, and until it is registered the run has no output contract to
/// run against — it refuses rather than writing an untyped corpus.
#[test]
fn a_document_is_missing_until_registered_and_the_run_waits_for_it() {
    let mut exec = Executor::new(CONN_DOCUMENT_PROGRAM);
    exec.set_connections(HashMap::from([(
        "vocab".to_string(),
        "https://shapes.example.com/v1".to_string(),
    )]));
    let missing = exec.missing_documents();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].key, "@vocab/executor.shex");
    assert_eq!(
        missing[0].locator,
        "https://shapes.example.com/v1/executor.shex"
    );

    let refused = exec
        .sources()
        .expect_err("no output shape is registered yet");
    assert!(refused.contains("not registered"), "{refused}");

    exec.register_document(&missing[0].key, EXECUTOR_SHEX);
    assert!(exec.missing_documents().is_empty());
    assert_eq!(exec.sources().expect("sources").len(), 1);
}
