//! Native gate for the executor: a CSV program read through a [`Storage`] of
//! in-memory stores runs end-to-end on `DataFusion`, writes the `GraphAr`
//! graph into memory, and answers a `RunReport` and the files. Where it
//! writes (`Output`) and how much it reads (`limit`) are held here too.
//! `packages/executor/tests/execute.test.ts` drives the same core through the
//! `#[wasm_bindgen]` wrapper under Node, which is where wasm-bindgen-futures is
//! proved.
//!
//! The shape document reaches the executor the way it reaches the checker —
//! reported by `missing_documents`, registered under its key — and the run's
//! output descriptor is decoded from that registration. The assertion on the
//! vertex type's `name` property is the guard that the document was READ: a
//! property key is the last segment of a predicate IRI the document declares, and
//! a run that skipped it writes no property at all.

// The executor is single-threaded, so its futures are not `Send` and need not be.
#![allow(clippy::literal_string_with_formatting_args, clippy::future_not_send)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use fossil_df::executor::MEMORY;
use fossil_df::{Executor, Output, Ran, RunReport};
use fossil_storage::{Access, Host, Scope, Storage};
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};

mod support;

/// A storage holding each `(location, fixture)` in memory, under `host`.
async fn storage_under(host: Arc<dyn Host>, sources: &[(&str, &str)]) -> Storage {
    let mut storage = Storage::new(host);
    let data = Arc::new(InMemory::new());
    storage
        .with_store(
            "https://data.example.com/",
            Arc::clone(&data) as Arc<dyn ObjectStore>,
        )
        .expect("route");
    for (location, fixture) in sources {
        let key = location
            .strip_prefix("https://data.example.com/")
            .expect("in the fixture authority");
        let bytes = std::fs::read(format!("tests/fixtures/{fixture}")).expect("fixture");
        data.put(&Path::from(key), bytes.into())
            .await
            .expect("stage");
    }
    storage
}

/// [`storage_under`] a host that vends nothing.
async fn storage(sources: &[(&str, &str)]) -> Storage {
    storage_under(Arc::new(support::NoHost), sources).await
}

/// Run `exec` into memory, reading at most `limit` rows of each source: the
/// report and every file, by path.
async fn into_memory(
    exec: &Executor,
    storage: &mut Storage,
    limit: Option<usize>,
) -> (RunReport, HashMap<String, Vec<u8>>) {
    let Ran::Memory { report, files } = exec
        .execute(storage, &Output::Memory, limit)
        .await
        .expect("the run")
    else {
        panic!("a run into memory answers its files")
    };
    let files = files
        .into_iter()
        .map(|f| (f.path, f.bytes.to_vec()))
        .collect();
    (report, files)
}

/// The document every program below names.
const EXECUTOR_SHEX: &str = include_str!("fixtures/executor.shex");

/// `program` compiled, with every document it names registered — what
/// `resolveDocuments` leaves behind.
fn executor(program: &str, connections: BTreeMap<String, String>) -> Executor {
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

/// `fossil.graph.yml`, parsed out of what the run wrote.
fn graph(files: &HashMap<String, Vec<u8>>) -> graphar_info::GraphInfo {
    support::graph(&|path| files.get(path).cloned())
}

/// The vertices of `ty`, from its count file.
fn vertices(files: &HashMap<String, Vec<u8>>, ty: &str) -> u64 {
    support::vertex_count(&|path| files.get(path).cloned(), &graph(files), ty)
}

#[tokio::test]
async fn csv_program_runs_through_the_storage_seam() {
    let mut storage = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
    let (report, files) =
        into_memory(&executor(PROGRAM, BTreeMap::new()), &mut storage, None).await;

    // One vertex type: its chunk, its count, its information file, and the graph
    // document over them; nothing else.
    let mut paths: Vec<&str> = files.keys().map(String::as_str).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "Person.vertex.yml",
            "fossil.graph.yml",
            "mapping.ttl",
            "vertex/Person/subject_name/chunk0",
            "vertex/Person/vertex_count",
        ]
    );
    assert!(!files["vertex/Person/subject_name/chunk0"].is_empty());

    assert_eq!(report.location, MEMORY);
    assert!(report.dropped.is_empty(), "no relation, nothing to drop");

    // 3 users → 3 vertices.
    let g = graph(&files);
    assert_eq!(g.vertices.len(), 1);
    let v = &g.vertices[0];
    assert_eq!(v.r#type, "Person");
    assert_eq!(vertices(&files, "Person"), 3);

    // The document was READ, not merely named: a property key is the last
    // segment of a predicate IRI the shape declares, so `name` exists only if
    // `executor.shex` reached the checker.
    let properties: Vec<&str> = v.property_groups[0]
        .properties
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        properties,
        ["subject", "name"],
        "Person carries the name property, after its subject"
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
fn inputs_list_each_distinct_source_with_its_format() {
    let srcs: Vec<_> = executor(TWO_SOURCE_PROGRAM, BTreeMap::new())
        .inputs()
        .into_iter()
        .filter(|s| s.role == fossil_hir::lineage::Role::Data)
        .collect();
    let uris: Vec<&str> = srcs.iter().map(|s| s.location.as_str()).collect();
    assert!(uris.contains(&"https://data.example.com/users.csv"));
    assert!(uris.contains(&"https://data.example.com/orders.csv"));
    assert_eq!(srcs.len(), 2);
    assert!(srcs.iter().all(|s| s.format.as_deref() == Some("csv")));
}

/// **The report is where the run wrote and what the join dropped** — and the
/// graph is not in it: it is `fossil.graph.yml`, beside the tables it names.
#[tokio::test]
async fn the_report_names_the_destination_and_the_drops() {
    let mut storage = two_sources().await;
    let (report, files) = into_memory(
        &executor(TWO_SOURCE_PROGRAM, BTreeMap::new()),
        &mut storage,
        None,
    )
    .await;
    let g = graph(&files);

    assert_eq!(report.location, MEMORY);
    let tables: Vec<String> = g.edges.iter().map(graphar_info::EdgeInfo::triple).collect();
    let dropped: Vec<&str> = report.dropped.iter().map(|d| d.table.as_str()).collect();
    assert_eq!(
        tables, dropped,
        "one drop count per edge type, in its order"
    );
    assert_eq!(tables, ["Order_placedBy_Person"]);
    for document in g
        .vertices
        .iter()
        .map(graphar_info::VertexInfo::file_name)
        .chain(g.edges.iter().map(graphar_info::EdgeInfo::file_name))
    {
        assert!(
            files.contains_key(&document),
            "`{document}` is named and was not written"
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

/// Both sources of [`TWO_SOURCE_PROGRAM`], under a host that vends nothing.
async fn two_sources() -> Storage {
    storage(&[
        ("https://data.example.com/users.csv", "users.csv"),
        ("https://data.example.com/orders.csv", "orders.csv"),
    ])
    .await
}

/// **`limit` caps each source scan, not the graph.** Three users and four
/// orders, read two rows each: two vertices of each type.
#[tokio::test]
async fn a_limit_reads_at_most_that_many_rows_of_each_source() {
    let mut storage = two_sources().await;
    let (_, files) = into_memory(
        &executor(TWO_SOURCE_PROGRAM, BTreeMap::new()),
        &mut storage,
        Some(2),
    )
    .await;
    let counts: Vec<(String, u64)> = graph(&files)
        .vertices
        .iter()
        .map(|v| (v.r#type.clone(), vertices(&files, &v.r#type)))
        .collect();
    assert_eq!(
        counts,
        [("Person".to_string(), 2), ("Order".to_string(), 2)],
        "each source read two rows"
    );
}

/// **An edge whose endpoint fell outside the sample is dropped and counted.**
/// The first two orders were placed by users 3 and 1, and the first two users
/// are 1 and 2: the edge to user 3 resolves no vertex, and the report says so.
#[tokio::test]
async fn an_edge_to_a_vertex_outside_the_sample_is_dropped_and_counted() {
    let mut storage = two_sources().await;
    let (report, files) = into_memory(
        &executor(TWO_SOURCE_PROGRAM, BTreeMap::new()),
        &mut storage,
        Some(2),
    )
    .await;
    assert_eq!(
        report.dropped,
        [fossil_df::EdgeDrops {
            table: "Order_placedBy_Person".to_string(),
            dropped: 1,
        }]
    );
    assert_eq!(
        support::edge_count(
            &|path| files.get(path).cloned(),
            &graph(&files),
            "Order_placedBy_Person"
        ),
        1,
        "only the order by user 1 is an edge"
    );
}

/// **Into memory, a run asks its host for no `write`.** The sources are read
/// through the storage the host's run is handed; the graph goes into an
/// `InMemory`, and a host that may only read can still preview.
///
/// What this cannot prove: that a source is read through a credential the host
/// vends — a store built from one is a store with a network behind it. The
/// sources are routed by hand, and `packages/executor/tests/run.test.ts` reads
/// them through vended `read` against a stubbed S3.
#[tokio::test]
async fn a_run_into_memory_asks_the_host_for_no_write() {
    let host = Arc::new(support::Vending::new(1));
    let mut storage = storage_under(
        Arc::clone(&host) as Arc<dyn Host>,
        &[("https://data.example.com/users.csv", "users.csv")],
    )
    .await;
    let (report, files) =
        into_memory(&executor(PROGRAM, BTreeMap::new()), &mut storage, None).await;
    assert_eq!(report.location, MEMORY);
    assert!(files.contains_key("fossil.graph.yml"));
    assert!(
        host.asked()
            .iter()
            .all(|(_, access)| *access != Access::Write),
        "{:?}",
        host.asked()
    );
}

/// **Into a job, a run asks for `write` on it first, and writes under exactly
/// one prefix.** A host that vends two is refused before a source is read.
#[tokio::test]
async fn a_run_into_a_job_vended_two_prefixes_is_refused() {
    let host = Arc::new(support::Vending::new(2));
    let mut storage = storage_under(
        Arc::clone(&host) as Arc<dyn Host>,
        &[("https://data.example.com/users.csv", "users.csv")],
    )
    .await;
    let refused = executor(PROGRAM, BTreeMap::new())
        .execute(
            &mut storage,
            &Output::Storage {
                job: "run-1".to_string(),
            },
            None,
        )
        .await
        .expect_err("refused");
    assert_eq!(
        refused.problem,
        fossil_graph_schema::Problem::AmbiguousPrefix {
            scope: "job run-1".to_string(),
            count: 2,
        }
    );
    assert_eq!(
        host.asked(),
        [(Scope::Job("run-1".to_string()), Access::Write)]
    );
}

/// A program that does not compile is refused as `run/does-not-compile`, and
/// the failure carries the diagnostics, not a sentence telling the host to see
/// diagnostics it has no way to reach.
#[tokio::test]
async fn a_program_that_does_not_compile_fails_with_its_diagnostics() {
    let broken = PROGRAM.replace(
        "    @subject = \"https://example.org/person/{users.id}\"\n",
        "",
    );
    let mut storage = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
    let refused = executor(&broken, BTreeMap::new())
        .execute(&mut storage, &Output::Memory, None)
        .await
        .expect_err("a program with no subject does not run");
    assert_eq!(
        refused.problem.code(),
        "run/does-not-compile",
        "{refused:?}"
    );
    assert!(
        !refused.related.is_empty(),
        "the diagnostics ride the failure"
    );
    assert!(refused.related.iter().all(|r| r.span.is_some()));
}

/// **A source whose file is not in the store fails as `source/not-found`,
/// naming the file.** `DataFusion` reads a location that matches nothing as a
/// relation with no columns, so without the check the run would fail at the
/// first column the program read — `engine/failed` with
/// `Schema error: No field named "PlaceRow".id`, which reads as a program
/// mistake. One spelling per object-store format, the CSV one as keasy writes
/// it: through a connection, with a delimiter.
#[tokio::test]
async fn a_source_that_names_no_file_fails_as_source_not_found() {
    for (source, location) in [
        (
            "io.csv(\"@ldbc/static/place_0_0.csv\", delimiter = \"|\")",
            "https://data.example.com/ldbc/static/place_0_0.csv",
        ),
        (
            "io.json(\"https://data.example.com/users.json\")",
            "https://data.example.com/users.json",
        ),
        (
            "io.parquet(\"https://data.example.com/users.parquet\")",
            "https://data.example.com/users.parquet",
        ),
    ] {
        let program = PROGRAM.replace("io.csv(\"https://data.example.com/users.csv\")", source);
        let mut storage = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
        let refused = executor(
            &program,
            BTreeMap::from([(
                "ldbc".to_string(),
                "https://data.example.com/ldbc".to_string(),
            )]),
        )
        .execute(&mut storage, &Output::Memory, None)
        .await
        .expect_err("a source with no file does not run");
        assert_eq!(
            refused.problem,
            fossil_graph_schema::Problem::SourceNotFound {
                location: location.to_string()
            },
            "{source}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn at_conn_source_alias_resolves_through_the_ref_map() {
    // `@mybucket/users.csv` resolves to `{base}/users.csv` via the ref-map —
    // both `inputs()` (enumeration) and `execute()` (the read) must agree.
    let exec = executor(
        CONN_PROGRAM,
        BTreeMap::from([(
            "mybucket".to_string(),
            "https://data.example.com".to_string(),
        )]),
    );

    let listed: Vec<_> = exec
        .inputs()
        .into_iter()
        .filter(|i| i.role == fossil_hir::lineage::Role::Data)
        .collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].location, "https://data.example.com/users.csv");
    assert_eq!(listed[0].format.as_deref(), Some("csv"));

    let mut storage = storage(&[(listed[0].location.as_str(), "users.csv")]).await;
    let (_, files) = into_memory(&exec, &mut storage, None).await;
    assert_eq!(vertices(&files, "Person"), 3);
}

const CONN_DOCUMENT_PROGRAM: &str = "\
type { Person, Order } := io.shex(\"@vocab/executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name
";

/// A document is keyed by what the program wrote and located through the
/// connections, and it stays missing until it is registered.
#[test]
fn a_document_is_missing_until_registered() {
    let mut exec = Executor::new(CONN_DOCUMENT_PROGRAM);
    exec.set_connections(BTreeMap::from([(
        "vocab".to_string(),
        "https://shapes.example.com/v1".to_string(),
    )]));
    let missing = exec.missing_documents();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].key, "@vocab/executor.shex");
    assert_eq!(
        missing[0].location,
        "https://shapes.example.com/v1/executor.shex"
    );

    exec.register_document(&missing[0].key, EXECUTOR_SHEX);
    assert!(exec.missing_documents().is_empty());
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
        let mut storage = storage(&[("https://data.example.com/users.csv", "users.csv")]).await;
        let (_, files) = into_memory(
            &executor(UNION_PROGRAM, BTreeMap::new()),
            &mut storage,
            None,
        )
        .await;
        assert_eq!(vertices(&files, "Person"), 3);
    });
}
