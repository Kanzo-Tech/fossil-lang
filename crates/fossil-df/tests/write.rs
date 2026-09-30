//! **What a `fossil/1` corpus promises, checked on the corpus** — by a second
//! engine.
//!
//! Every assertion reads the written files with plain SQL over `DuckDB`, and
//! never through `fossil-df`'s own types: a test that reads back through the
//! writer's types proves the types are self-consistent and nothing else. The
//! manifest is parsed as JSON into the format's structs, because those ARE the
//! format; the Parquet is read by `read_parquet`.
//!
//! The promises, in the order `/docs/format` states them:
//!
//! 1. one Parquet per vertex type and per relation, at the path `fossil.json`
//!    names, and nothing else under the destination;
//! 2. `dense_id` is global and gapless — `0..V` once each over the union of the
//!    vertex tables — and every vertex table is sorted by it;
//! 3. the columns are the ones `properties` lists, in its order, the writer's
//!    five first;
//! 4. `subject` is unique within a table;
//! 5. an edge table is `src`, `dst`, sorted by both, and every endpoint is a
//!    `dense_id` of the table its `references` names;
//! 6. row groups of 122,880 rows, and the sorted key delta-encoded;
//! 7. `dense_id` order is `ST_Hilbert` order over the union's bounding box —
//!    asked of a `duckdb` binary with the spatial extension, and skipped with a
//!    message when there is none;
//! 8. a value travels with its row and an edge connects the identities it was
//!    written from — asked through `subject`, since `dense_id` is an address;
//! 9. `fossil.json` is the last object written, and a write that fails before
//!    it leaves no `fossil.json` at all.

#![cfg(not(target_arch = "wasm32"))]
// Counts cross into `DuckDB`'s `BIGINT` and back; every one here is small. The
// executor is single-threaded, so its futures are not `Send` and need not be.
#![allow(
    clippy::literal_string_with_formatting_args,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::future_not_send
)]

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use duckdb::Connection;
use fossil_df::Executor;
use fossil_sinks::manifest::{Manifest, ROW_GROUP_ROWS};
use fossil_storage::{Access, Host, Scope, Storage, StorageCredential};
use futures::TryStreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

const DEST: &str = "s3://jobs/run-1/";
const SHEX: &str = include_str!("fixtures/executor.shex");

const PROGRAM: &str = "\
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

const PEOPLE_ONLY: &str = "\
type { Person, Order } := io.shex(\"executor.shex\")

users := io.csv(\"https://data.example.com/users.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name
";

#[derive(Debug)]
struct NoHost;

impl Host for NoHost {
    fn connections(
        &self,
    ) -> BoxFuture<'static, Result<HashMap<String, String>, fossil_graph_schema::Foreign>> {
        Box::pin(async { Ok(HashMap::new()) })
    }
    fn credentials(
        &self,
        _: &Scope,
        _: Access,
    ) -> BoxFuture<'static, Result<Vec<StorageCredential>, fossil_graph_schema::Foreign>> {
        Box::pin(async {
            Err(fossil_graph_schema::Foreign::named(
                "Error",
                "these tests vend nothing",
            ))
        })
    }
}

/// An in-memory store that records the order of every put, and refuses the
/// put numbered `fail_at` (from zero) and every one after it.
#[derive(Debug)]
struct Recording {
    inner: InMemory,
    puts: Mutex<Vec<String>>,
    fail_at: usize,
}

impl fmt::Display for Recording {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Recording")
    }
}

#[async_trait]
impl ObjectStore for Recording {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        {
            let mut puts = self.puts.lock().expect("lock");
            if puts.len() >= self.fail_at {
                return Err(object_store::Error::Generic {
                    store: "Recording",
                    source: "refused on purpose".into(),
                });
            }
            puts.push(location.to_string());
        }
        self.inner.put_opts(location, payload, opts).await
    }
    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }
    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.inner.get_opts(location, options).await
    }
    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.inner.delete_stream(locations)
    }
    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }
    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }
    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

/// `users` people and `orders` orders, each order placed by a person, the ids
/// a fixed pseudo-random walk so a run is repeatable.
fn sources(users: u32, orders: u32) -> Vec<(&'static str, Vec<u8>)> {
    let mut people = String::from("id,name\n");
    for i in 0..users {
        let _ = writeln!(people, "{i},person {i}");
    }
    let mut placed = String::from("order_id,user_id,amount\n");
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for o in 0..orders {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Mostly near the order's own number, so the graph has communities.
        let near = (u64::from(o) * u64::from(users) / u64::from(orders.max(1)) + state % 7)
            % u64::from(users.max(1));
        let _ = writeln!(placed, "{o},{near},{}", state % 1000);
    }
    vec![
        ("users.csv", people.into_bytes()),
        ("orders.csv", placed.into_bytes()),
    ]
}

/// Run `program` over `files`, into a [`Recording`] store that fails at put
/// number `fail_at`. Answers the run's result and the store.
async fn run(
    program: &str,
    files: Vec<(&'static str, Vec<u8>)>,
    fail_at: usize,
) -> (
    Result<fossil_df::RunReport, fossil_graph_schema::Failure>,
    Arc<Recording>,
) {
    let mut storage = Storage::new(Arc::new(NoHost));
    let data = Arc::new(InMemory::new());
    storage
        .with_store(
            "https://data.example.com/",
            Arc::clone(&data) as Arc<dyn ObjectStore>,
        )
        .expect("route");
    for (name, bytes) in files {
        data.put(&Path::from(name), bytes.into())
            .await
            .expect("stage");
    }
    let out = Arc::new(Recording {
        inner: InMemory::new(),
        puts: Mutex::new(Vec::new()),
        fail_at,
    });
    storage
        .with_store(DEST, Arc::clone(&out) as Arc<dyn ObjectStore>)
        .expect("route");
    let mut exec = Executor::new(program);
    for missing in exec.missing_documents() {
        exec.register_document(&missing.key, SHEX);
    }
    let result = exec.execute(&mut storage, DEST).await;
    (result, out)
}

/// What the run wrote, copied into a fresh directory so `DuckDB` can read it.
async fn materialise(out: &Recording, name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("fossil_write_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let listed: Vec<_> = out.inner.list(None).try_collect().await.expect("list");
    for meta in listed {
        let bytes = out
            .inner
            .get(&meta.location)
            .await
            .expect("get")
            .bytes()
            .await
            .expect("bytes");
        let rel = meta
            .location
            .as_ref()
            .strip_prefix("run-1/")
            .expect("under DEST");
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, bytes).expect("write");
    }
    root
}

fn manifest(root: &std::path::Path) -> Manifest {
    serde_json::from_slice(&std::fs::read(root.join("fossil.json")).expect("fossil.json"))
        .expect("fossil.json is the manifest")
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn text(conn: &Connection, sql: &str) -> String {
    conn.query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

/// Every file under `root`, relative, sorted.
fn tree(root: &std::path::Path) -> Vec<String> {
    fn walk(at: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(at).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .expect("under root")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn the_corpus_keeps_the_promises_it_makes_to_a_stranger() {
    let (result, out) = run(PROGRAM, sources(400, 1_200), usize::MAX).await;
    let report = result.expect("the run");
    let root = materialise(&out, "promises").await;
    let m = manifest(&root);
    let conn = Connection::open_in_memory().expect("duckdb");
    let at = |path: &str| format!("'{}'", root.join(path).display());

    // 1. The files are exactly the manifest's.
    let mut named: Vec<String> = m
        .vertex_tables
        .iter()
        .map(|v| v.path.clone())
        .chain(m.edge_tables.iter().map(|e| e.path.clone()))
        .collect();
    named.push("fossil.json".to_string());
    named.sort();
    assert_eq!(
        tree(&root),
        named,
        "the destination holds what fossil.json names and nothing else"
    );
    assert_eq!(report.dropped.len(), m.edge_tables.len());

    // 2. `dense_id` is global, gapless, and each table is sorted by it.
    let all = m
        .vertex_tables
        .iter()
        .map(|v| at(&v.path))
        .collect::<Vec<_>>()
        .join(", ");
    let total: i64 = m.vertex_tables.iter().map(|v| v.record_count as i64).sum();
    assert_eq!(total, 1_600, "400 people and 1,200 orders");
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(DISTINCT dense_id) FROM read_parquet([{all}])")
        ),
        total
    );
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT min(dense_id) FROM read_parquet([{all}])")
        ),
        0
    );
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT max(dense_id) FROM read_parquet([{all}])")
        ),
        total - 1
    );
    for v in &m.vertex_tables {
        let p = at(&v.path);
        assert_eq!(
            scalar(&conn, &format!("SELECT count(*) FROM read_parquet({p})")),
            v.record_count as i64,
            "{} declares its count",
            v.name
        );
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT dense_id, lag(dense_id) OVER (ORDER BY file_row_number) AS prev \
                     FROM read_parquet({p}, file_row_number = true)) WHERE prev >= dense_id"
                )
            ),
            0,
            "{} is sorted by dense_id",
            v.name
        );

        // 3. The columns are the properties, in order.
        let described = text(
            &conn,
            &format!(
                "SELECT string_agg(column_name, ',' ORDER BY rowid) FROM (SELECT column_name, row_number() OVER () AS rowid FROM (DESCRIBE SELECT * FROM read_parquet({p})))"
            ),
        );
        let declared: Vec<&str> = v.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            described,
            declared.join(","),
            "{} declares its columns",
            v.name
        );
        assert_eq!(
            &declared[..5],
            ["dense_id", "subject", "x", "y", "cluster_id"]
        );

        // 4. `subject` is unique within the table.
        assert_eq!(
            scalar(
                &conn,
                &format!("SELECT count(DISTINCT subject) FROM read_parquet({p})")
            ),
            v.record_count as i64
        );
    }

    // 5. An edge table's endpoints are dense ids of the tables it references,
    //    and it is sorted by (src, dst).
    for e in &m.edge_tables {
        let p = at(&e.path);
        let source = m
            .vertex_tables
            .iter()
            .find(|v| v.name == e.source.references)
            .expect("the source table");
        let destination = m
            .vertex_tables
            .iter()
            .find(|v| v.name == e.destination.references)
            .expect("the destination table");
        assert_eq!(
            text(
                &conn,
                &format!(
                    "SELECT string_agg(column_name, ',') FROM (DESCRIBE SELECT * FROM read_parquet({p}))"
                )
            ),
            "src,dst"
        );
        assert_eq!(
            scalar(&conn, &format!("SELECT count(*) FROM read_parquet({p})")),
            e.record_count as i64
        );
        assert!(e.record_count > 0, "{} is not vacuous", e.name);
        for (column, table) in [("src", source), ("dst", destination)] {
            assert_eq!(
                scalar(
                    &conn,
                    &format!(
                        "SELECT count(*) FROM read_parquet({p}) e ANTI JOIN read_parquet({}) v ON e.{column} = v.dense_id",
                        at(&table.path)
                    )
                ),
                0,
                "every {column} of {} is a dense_id of {}",
                e.name,
                table.name
            );
        }
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT src, dst, lag(src) OVER w AS ps, lag(dst) OVER w AS pd \
                     FROM read_parquet({p}, file_row_number = true) WINDOW w AS (ORDER BY file_row_number)) \
                     WHERE ps IS NOT NULL AND (ps > src OR (ps = src AND pd >= dst))"
                )
            ),
            0,
            "{} is sorted by (src, dst)",
            e.name
        );
    }

    // 6. The sorted keys are delta-encoded.
    for (path, column) in m
        .vertex_tables
        .iter()
        .map(|v| (&v.path, "dense_id"))
        .chain(m.edge_tables.iter().map(|e| (&e.path, "src")))
    {
        let encodings = text(
            &conn,
            &format!(
                "SELECT string_agg(DISTINCT encodings, ',') FROM parquet_metadata({}) WHERE path_in_schema = '{column}'",
                at(path)
            ),
        );
        assert!(
            encodings.contains("DELTA_BINARY_PACKED"),
            "{path}.{column} is {encodings}"
        );
    }

    // 7. The order is `ST_Hilbert`'s, over the union's box.
    let sql = format!(
        "LOAD spatial; \
         WITH v AS (SELECT dense_id, x, y FROM read_parquet([{all}])), \
         b AS (SELECT {{'min_x': min(x), 'min_y': min(y), 'max_x': max(x), 'max_y': max(y)}}::BOX_2D AS bx FROM v), \
         h AS (SELECT dense_id, ST_Hilbert(x, y, bx) AS code FROM v, b) \
         SELECT count(*) FROM (SELECT code, lag(code) OVER (ORDER BY dense_id) AS prev FROM h) WHERE prev > code;"
    );
    match std::process::Command::new("duckdb")
        .args(["-noheader", "-csv", "-c", &sql])
        .output()
    {
        Ok(out) if out.status.success() => assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "0",
            "dense_id order is ST_Hilbert order"
        ),
        Ok(out) => eprintln!(
            "skipped the ST_Hilbert check: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(_) => eprintln!("skipped the ST_Hilbert check: no `duckdb` on the path"),
    }

    // 8. The payload half: a value travels with its row, and an edge connects
    //    the identities it was written from. Every check above is about counts,
    //    ids and order, so a writer that renumbered the rows and attached each
    //    name to the wrong vertex, or pointed each edge at the wrong id, keeps
    //    them all. Joined through `subject`, never `dense_id`: the id is an
    //    address the layout assigns, and the IRI is what a stranger holds.
    let ordinal = |column: &str| format!("regexp_extract({column}, '([0-9]+)$', 1)");
    let person = m
        .vertex_tables
        .iter()
        .find(|v| v.name == "Person")
        .expect("Person");
    let order = m
        .vertex_tables
        .iter()
        .find(|v| v.name == "Order")
        .expect("Order");
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM read_parquet({}) WHERE name <> 'person ' || {}",
                at(&person.path),
                ordinal("subject")
            )
        ),
        0,
        "a vertex carries a name that was not built from its own identity"
    );
    let orders_csv = root.join("orders.csv");
    std::fs::write(&orders_csv, &sources(400, 1_200)[1].1).expect("write orders.csv");
    let placed_by = m
        .edge_tables
        .iter()
        .find(|e| e.name == "Order_placedBy_Person")
        .expect("the relation");
    let written = format!(
        "SELECT {} AS o, {} AS u FROM read_parquet({}) e \
         JOIN read_parquet({}) s ON s.dense_id = e.src \
         JOIN read_parquet({}) d ON d.dense_id = e.dst",
        ordinal("s.subject"),
        ordinal("d.subject"),
        at(&placed_by.path),
        at(&order.path),
        at(&person.path),
    );
    let asked = format!(
        "SELECT order_id::VARCHAR AS o, user_id::VARCHAR AS u FROM read_csv('{}')",
        orders_csv.display()
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM (({written} EXCEPT ALL {asked}) UNION ALL ({asked} EXCEPT ALL {written}))"
            )
        ),
        0,
        "the edges are not the (order, person) pairs the input named"
    );

    // 9. fossil.json went last.
    let puts = out.puts.lock().expect("lock").clone();
    assert_eq!(
        puts.last().map(String::as_str),
        Some("run-1/fossil.json"),
        "{puts:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **A cluster is one run of `dense_id`** — over the union of the vertex
/// tables, because the layout is joint: one partition and one placement over
/// the whole graph, and one rank across every type.
///
/// `fossil_layout::layout::cluster_layout` gives each group one aligned block
/// off a frontier that never goes back, and `order_by_hierarchy` numbers the
/// groups so a run of consecutive ids is a subtree — so a group is a run of
/// adjacent blocks, hence of Hilbert codes, hence of ids. This goes red for a
/// buddy allocation that reuses its holes, a placement that ignores the
/// hierarchy's order, or a type laid out apart from the others: each writes a
/// corpus that opens and draws, with a reader colouring scattered packets.
///
/// A budget and not a zero: `dense_id` is the Hilbert rank over the union's
/// bounding box, quantised per axis over the extent the positions turned out
/// to have, and that box is square only to within the margins of the groups at
/// its corners. What is asserted is the mass — ids inside a group's range that
/// are not the group's — against one per cent of the corpus.
#[tokio::test]
async fn a_cluster_is_one_run_of_dense_id() {
    let (result, out) = run(PROGRAM, sources(2_000, 6_000), usize::MAX).await;
    result.expect("the run");
    let root = materialise(&out, "cluster_runs").await;
    let m = manifest(&root);
    let conn = Connection::open_in_memory().expect("duckdb");
    let all = m
        .vertex_tables
        .iter()
        .map(|v| format!("'{}'", root.join(&v.path).display()))
        .collect::<Vec<_>>()
        .join(", ");

    // More than one group, and a group that spans types, or the property is
    // vacuous: a partition of one is an interval whatever the placement did,
    // and a partition that never mixes types could be laid out per type.
    let groups = scalar(
        &conn,
        &format!("SELECT count(DISTINCT cluster_id) FROM read_parquet([{all}])"),
    );
    assert!(groups > 1, "one group is not a partition to check");
    let mixed = scalar(
        &conn,
        &format!(
            "SELECT count(*) FROM (SELECT cluster_id FROM read_parquet([{all}], filename = true) \
             GROUP BY cluster_id HAVING count(DISTINCT filename) > 1)"
        ),
    );
    assert!(
        mixed > 0,
        "no group spans two types, so the layout is not shown to be joint"
    );

    let total: i64 = m.vertex_tables.iter().map(|v| v.record_count as i64).sum();
    let foreign = scalar(
        &conn,
        &format!(
            "SELECT coalesce(sum(hi - lo + 1 - n), 0)::BIGINT FROM ( \
               SELECT count(*) AS n, min(dense_id) AS lo, max(dense_id) AS hi \
               FROM read_parquet([{all}]) GROUP BY cluster_id)"
        ),
    );
    let budget = total / 100;
    assert!(
        foreign <= budget,
        "{foreign} ids fall inside a group's range without belonging to it, over {groups} \
         groups — the budget is {budget}, and a partition scattered across the axis reaches {}",
        total * groups,
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A table past one row group is cut at [`ROW_GROUP_ROWS`], which is what lets
/// a reader prune a bounding box by the footer's statistics.
#[tokio::test]
async fn a_table_past_one_row_group_is_cut_at_122_880_rows() {
    let rows = ROW_GROUP_ROWS as u32 + 7_000;
    let (result, out) = run(PEOPLE_ONLY, sources(rows, 0), usize::MAX).await;
    result.expect("the run");
    let root = materialise(&out, "row_groups").await;
    let conn = Connection::open_in_memory().expect("duckdb");
    let p = format!("'{}'", root.join("vertex/Person.parquet").display());
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(DISTINCT row_group_id) FROM parquet_metadata({p})")
        ),
        2
    );
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT max(row_group_num_rows) FROM parquet_metadata({p})")
        ),
        ROW_GROUP_ROWS as i64
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A write that fails part of the way leaves Parquet with no manifest over it,
/// which no reader opens — never a manifest over missing tables.
#[tokio::test]
async fn a_write_that_fails_leaves_no_manifest() {
    for fail_at in [0, 1, 2] {
        let (result, out) = run(PROGRAM, sources(20, 30), fail_at).await;
        let refused = result.expect_err("the store refused a put");
        assert_eq!(refused.problem.code(), "write/failed", "{refused}");
        let chain =
            std::iter::successors(Some(&refused as &(dyn std::error::Error + 'static)), |e| {
                e.source()
            })
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert!(
            chain.iter().any(|e| e.contains("refused on purpose")),
            "the store's own error is kept as a cause: {chain:?}"
        );
        let listed: Vec<_> = out.inner.list(None).try_collect().await.expect("list");
        assert!(
            listed
                .iter()
                .all(|m| !m.location.as_ref().ends_with("fossil.json")),
            "a failed write at put {fail_at} left a manifest: {listed:?}"
        );
    }
}
