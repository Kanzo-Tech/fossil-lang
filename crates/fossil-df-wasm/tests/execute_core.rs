//! Native gate for the browser executor core: a CSV program read through a
//! [`Storage`] of in-memory stores runs end-to-end on `DataFusion`, writes the
//! `GraphAr` files under its destination, and answers a `RunReport`. `packages/executor/tests/execute.test.ts`
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
use std::sync::Arc;

use fossil_df_wasm::Executor;
use fossil_storage::{Access, Host, Scope, Storage, StorageCredential};
use futures::TryStreamExt;
use futures::future::BoxFuture;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};

const DEST: &str = "s3://jobs/run-1/";

#[derive(Debug)]
struct NoHost;

impl Host for NoHost {
    fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, String>> {
        Box::pin(async { Ok(HashMap::new()) })
    }
    fn credentials(
        &self,
        _: &Scope,
        _: Access,
    ) -> BoxFuture<'static, Result<Vec<StorageCredential>, String>> {
        Box::pin(async { Err("these tests vend nothing".to_string()) })
    }
}

/// A storage holding each `(locator, fixture)` in memory, and the store the
/// run writes into.
async fn storage(sources: &[(&str, &str)]) -> (Storage, Arc<InMemory>) {
    let mut storage = Storage::new(Arc::new(NoHost));
    let data = Arc::new(InMemory::new());
    storage
        .with_store(
            "https://data.example.com/",
            Arc::clone(&data) as Arc<dyn ObjectStore>,
        )
        .expect("route");
    for (locator, fixture) in sources {
        let key = locator
            .strip_prefix("https://data.example.com/")
            .expect("in the fixture authority");
        let bytes =
            std::fs::read(format!("../fossil-df/tests/fixtures/{fixture}")).expect("fixture");
        data.put(&Path::from(key), bytes.into())
            .await
            .expect("stage");
    }
    let out = Arc::new(InMemory::new());
    storage
        .with_store(DEST, Arc::clone(&out) as Arc<dyn ObjectStore>)
        .expect("route");
    (storage, out)
}

/// What the run wrote, by path under [`DEST`].
async fn written(out: &InMemory) -> HashMap<String, Vec<u8>> {
    let listed: Vec<_> = out.list(None).try_collect().await.expect("list");
    let mut files = HashMap::new();
    for meta in listed {
        let bytes = out
            .get(&meta.location)
            .await
            .expect("get")
            .bytes()
            .await
            .expect("bytes");
        let path = meta
            .location
            .as_ref()
            .strip_prefix("run-1/")
            .expect("under DEST")
            .to_string();
        files.insert(path, bytes.to_vec());
    }
    files
}

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
async fn csv_program_runs_through_the_storage_seam() {
    let (mut storage, out) = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
    let report = executor(PROGRAM, HashMap::new())
        .execute(&mut storage, DEST)
        .await
        .expect("executor runs the CSV program");
    let files = written(&out).await;

    // The TILED tree, which is the one `fossil run` writes: the tiles under the
    // declared prefix, the identity index beside them, and the staged
    // single-file payload GONE — it is never written now, where it used to be
    // written, read by the pass and removed from the map. Left in, it is a
    // second, stale copy of every vertex and `packages/corpus/guards`' `exactly-once`
    // fails a corpus for it.
    let paths: Vec<&str> = files.keys().map(String::as_str).collect();
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

    assert!(!files["vertex/Person/tiles.parquet"].is_empty());

    // 3 users → 3 vertices.
    assert_eq!(report.dest, DEST);
    assert_eq!(report.vertices.len(), 1);
    let v = &report.vertices[0];
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
    let uris: Vec<&str> = srcs.iter().map(|(u, _, _)| u.as_str()).collect();
    assert!(uris.contains(&"https://data.example.com/users.csv"));
    assert!(uris.contains(&"https://data.example.com/orders.csv"));
    assert_eq!(srcs.len(), 2);
    assert!(srcs.iter().all(|(_, fmt, _)| *fmt == "csv"));
}

/// **The browser's report IS the documents the browser shipped** — every
/// manifest, field for field, checked against the YAML the same run wrote.
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
    let (mut storage, out) = storage(&[
        ("https://data.example.com/users.csv", "users.csv"),
        ("https://data.example.com/orders.csv", "orders.csv"),
    ])
    .await;
    let report = executor(TWO_SOURCE_PROGRAM, HashMap::new())
        .execute(&mut storage, DEST)
        .await
        .expect("executor runs the two-source program");
    let files = written(&out).await;

    let shipped = |rel: &str| -> String {
        let bytes = files
            .get(rel)
            .unwrap_or_else(|| panic!("the run emitted `{rel}`"));
        String::from_utf8(bytes.clone()).expect("a manifest is UTF-8")
    };

    // The index, then every document it names — positionally, which is the
    // agreement `GraphInfo::vertices`/`edges` already carry.
    assert_eq!(
        shipped("graph.graph.yml"),
        report.graph.to_yaml().expect("the index serialises"),
        "the report's index is not the `graph.graph.yml` the run shipped"
    );
    assert!(!report.vertices.is_empty() && !report.edges.is_empty());
    for (rel, info) in report.graph.vertices.iter().zip(&report.vertices) {
        assert_eq!(
            shipped(rel),
            info.to_yaml().expect("a vertex document serialises"),
            "the report's entry for `{rel}` is not the document the run shipped"
        );
    }
    for (rel, info) in report.graph.edges.iter().zip(&report.edges) {
        assert_eq!(
            shipped(rel),
            info.to_yaml().expect("an edge document serialises"),
            "the report's entry for `{rel}` is not the document the run shipped"
        );
    }

    // And the equality is not vacuous on the field that found this: the pass
    // partitioned `Person`, so its document declares a channel with a measured
    // domain — and therefore so must the report.
    let person = report
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

/// A destination no store covers is refused before anything runs: the output
/// has nowhere to go, and a run that wrote nothing would still report.
#[tokio::test]
async fn a_destination_no_store_covers_is_refused() {
    let (mut storage, _) = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
    let refused = executor(PROGRAM, HashMap::new())
        .execute(&mut storage, "s3://elsewhere/run-2/")
        .await
        .expect_err("refused");
    assert!(refused.contains("no store covers"), "{refused}");
}

#[tokio::test]
async fn at_conn_source_alias_resolves_through_the_ref_map() {
    // `@mybucket/users.csv` resolves to `{base}/users.csv` via the ref-map —
    // both `sources()` (enumeration) and `execute()` (the read) must agree.
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
    assert_eq!(listed[0].1, "csv");

    let (mut storage, _) = storage(&[(listed[0].0.as_str(), "users.csv")]).await;
    let report = exec
        .execute(&mut storage, DEST)
        .await
        .expect("executor runs the @conn-aliased program");
    let person = report.vertices.iter().find(|v| v.vertex_type == "Person");
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

/// Two mappings of one type: `finalize_vertex` unions them, and a `UNION ALL` of
/// two scans has two partitions however few the session targets.
const UNION_PROGRAM: &str = "\
type { Person, Order } := io.shex(\"executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")

Early : Person from users.where(users.id < 3)
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Late : Person from users.where(users.id >= 3)
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name
";

/// **The browser's situation, natively: no Tokio runtime at all.** Every other
/// test here runs under `#[tokio::test]`, where a `DataFusion` operator that
/// spawns simply succeeds — which is how a union that panicked in the browser
/// and hung the tab passed this file. `futures::executor::block_on` has no
/// reactor, so a spawn anywhere in the plan fails here as it does in the tab.
#[test]
fn a_union_of_two_mappings_runs_with_no_tokio_runtime() {
    futures::executor::block_on(async {
        let (mut storage, _) =
            storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
        let report = executor(UNION_PROGRAM, HashMap::new())
            .execute(&mut storage, DEST)
            .await
            .expect("the union runs on one future");
        assert_eq!(report.vertices[0].vertex_count, 3);
    });
}
