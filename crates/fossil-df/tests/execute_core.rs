//! Native gate for the executor: a CSV program read through a [`Storage`] of
//! in-memory stores runs end-to-end on `DataFusion`, writes the `fossil/1`
//! corpus under its destination, and answers a `RunReport`.
//! `packages/executor/tests/execute.test.ts` drives the same core through the
//! `#[wasm_bindgen]` wrapper under Node, which is where wasm-bindgen-futures is
//! proved.
//!
//! The shape document reaches the executor the way it reaches the checker —
//! reported by `missing_documents`, registered under its key — and the run's
//! output descriptor is decoded from that registration. The assertion on the
//! vertex table's `iri` is the guard that the document was READ: a bare header
//! name is bound positionally against it, and a run that skipped it writes no
//! type IRI.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use std::collections::HashMap;
use std::sync::Arc;

use fossil_df::Executor;
use fossil_sinks::manifest::Manifest;
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
        let bytes = std::fs::read(format!("tests/fixtures/{fixture}")).expect("fixture");
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

/// `fossil.json`, parsed out of what the run wrote.
fn manifest(files: &HashMap<String, Vec<u8>>) -> Manifest {
    serde_json::from_slice(files.get("fossil.json").expect("the run wrote fossil.json"))
        .expect("fossil.json is the manifest")
}

#[tokio::test]
async fn csv_program_runs_through_the_storage_seam() {
    let (mut storage, out) = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
    let report = executor(PROGRAM, HashMap::new())
        .execute(&mut storage, DEST)
        .await
        .expect("executor runs the CSV program");
    let files = written(&out).await;

    // One table per vertex type and the manifest over it; nothing else.
    let mut paths: Vec<&str> = files.keys().map(String::as_str).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["fossil.json", "vertex/Person.parquet"]);
    assert!(!files["vertex/Person.parquet"].is_empty());

    assert_eq!(report.dest, DEST);
    assert!(report.dropped.is_empty(), "no relation, nothing to drop");

    // 3 users → 3 vertices.
    let m = manifest(&files);
    assert_eq!(m.vertex_tables.len(), 1);
    let v = &m.vertex_tables[0];
    assert_eq!(v.name, "Person");
    assert_eq!(v.record_count, 3);

    // The document was READ, not merely named: a bare header name is bound
    // positionally against the shape document, so this IRI exists only if
    // `executor.shex` reached the checker.
    assert_eq!(
        v.iri.as_deref(),
        Some("https://example.org/Person"),
        "the vertex's type IRI comes from the registered document"
    );
    assert!(
        v.properties.iter().any(|p| p.name == "name"),
        "Person carries the name column: {:?}",
        v.properties,
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

/// **The report is where the run wrote and what the join dropped** — and the
/// manifest is not in it: it is `fossil.json`, beside the tables it names.
#[tokio::test]
async fn the_report_names_the_destination_and_the_drops() {
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
    let m = manifest(&files);

    assert_eq!(report.dest, DEST);
    let tables: Vec<&str> = m.edge_tables.iter().map(|e| e.name.as_str()).collect();
    let dropped: Vec<&str> = report.dropped.iter().map(|d| d.table.as_str()).collect();
    assert_eq!(
        tables, dropped,
        "one drop count per edge table, in its order"
    );
    assert_eq!(tables, ["Order_placedBy_Person"]);
    for table in m
        .vertex_tables
        .iter()
        .map(|v| &v.path)
        .chain(m.edge_tables.iter().map(|e| &e.path))
    {
        assert!(
            files.contains_key(table),
            "`{table}` is named and was not written"
        );
    }
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
    assert!(refused.to_string().contains("no store covers"), "{refused}");
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

    let (mut storage, out) = storage(&[(listed[0].0.as_str(), "users.csv")]).await;
    exec.execute(&mut storage, DEST)
        .await
        .expect("executor runs the @conn-aliased program");
    let m = manifest(&written(&out).await);
    let person = m.vertex_tables.iter().find(|v| v.name == "Person");
    assert_eq!(person.map(|v| v.record_count), Some(3));
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
        let (mut storage, out) =
            storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
        executor(UNION_PROGRAM, HashMap::new())
            .execute(&mut storage, DEST)
            .await
            .expect("the union runs on one future");
        assert_eq!(
            manifest(&written(&out).await).vertex_tables[0].record_count,
            3
        );
    });
}
