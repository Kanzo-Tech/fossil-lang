//! **What a fossil graph promises, checked on the files** — by a second engine.
//!
//! Every assertion reads the written files with plain SQL over `DuckDB`, and
//! never through `fossil-exec`'s own types: a test that reads back through the
//! writer's types proves the types are self-consistent and nothing else. The
//! information files are parsed by `graphar-info`, because those ARE the format;
//! the Parquet is read by `read_parquet`.
//!
//! The promises, in the order `/docs/format` states them:
//!
//! 1. the files are the ones the information files name — the chunks, the count
//!    files, the offsets — and nothing else under the destination;
//! 2. a vertex's index is its row within its type: `0..V` once each, per type;
//! 3. the columns are the ones the property group lists, in its order, the
//!    index and `subject` first;
//! 4. `subject` is unique within a table;
//! 5. an adjacency list is `_graphArSrcIndex`, `_graphArDstIndex`, sorted by
//!    both, every endpoint an index of the type its information file names, and
//!    its offset table has the source type's count plus one rows;
//! 6. the sorted key delta-encoded;
//! 7. a chunk is one row group, and its footer's `Statistics` count the
//!    distinct values beside the nulls — Parquet's own, and no key of fossil's;
//! 8. a value travels with its row and an edge connects the identities it was
//!    written from — asked through `subject`, since an index is an address;
//! 9. `fossil.graph.yml` is the last object written, and a write that fails
//!    before it leaves no graph document at all.

// Counts cross into `DuckDB`'s `BIGINT` and back; every one here is small. The
// executor is single-threaded, so its futures are not `Send` and need not be.
#![allow(
    clippy::literal_string_with_formatting_args,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::future_not_send
)]

use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use duckdb::Connection;
use fossil_storage::Storage;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use graphar_info::GraphInfo;
use graphar_parquet::paths;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

mod support;

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

/// Execute `program` over `files` and [`fossil_sinks::graphar::write`] its graph into a
/// [`Recording`] store at [`DEST`] that fails at put number `fail_at`. Answers
/// the write's result and the store.
///
/// The writer is called directly: a run chooses its own store
/// (`fossil_exec::Output`), and these promises are the writer's, so the store
/// that records and refuses is handed to it here.
async fn run(
    program: &str,
    files: Vec<(&'static str, Vec<u8>)>,
    fail_at: usize,
) -> (
    Result<fossil_sinks::graphar::Written, fossil_graph_schema::Failure>,
    Arc<Recording>,
) {
    let mut storage = Storage::new(Arc::new(support::NoHost));
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
    let (db, file) = support::db_with_shapes(program, "write.fossil", &[("executor.shex", SHEX)]);
    let ctx = fossil_exec::session::session_within(
        Arc::new(fossil_exec::memory::Budget::new(
            fossil_exec::memory::BUDGET,
        )),
        None,
    );
    for (authority, store) in storage.stores() {
        ctx.register_object_store(authority, store);
    }
    let graph = fossil_exec::execute_graph(&ctx, &db, file)
        .await
        .expect("the graph");
    let result = fossil_sinks::graphar::write(&graph, &storage, DEST)
        .await
        .map_err(fossil_graph_schema::Failure::from);
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

fn graph(root: &std::path::Path) -> GraphInfo {
    let graph = GraphInfo::load_file(root.join("fossil.graph.yml")).expect("fossil.graph.yml");
    graph.validate().expect("a valid GraphAr graph");
    graph
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
async fn the_graph_keeps_the_promises_it_makes_to_a_stranger() {
    let (result, out) = run(PROGRAM, sources(400, 1_200), usize::MAX).await;
    let written = result.expect("the write");
    let root = materialise(&out, "promises").await;
    let g = graph(&root);
    assert_eq!(g, written.info, "what was written is what was answered");
    let conn = Connection::open_in_memory().expect("duckdb");
    let at = |path: &str| format!("'{}'", root.join(path).display());
    let count = |path: String| -> i64 {
        i64::from_le_bytes(
            std::fs::read(root.join(path))
                .expect("a count file")
                .try_into()
                .expect("eight bytes"),
        )
    };

    // 1. The files are exactly the information files' and the ones they name.
    let mut named = vec!["fossil.graph.yml".to_string(), "mapping.ttl".to_string()];
    for v in &g.vertices {
        named.push(v.file_name());
        named.push(paths::vertex_count(v));
        named.push(paths::vertex_chunk(v, &v.property_groups[0], 0));
    }
    for e in &g.edges {
        let adj = &e.adj_lists[0];
        named.push(e.file_name());
        named.push(paths::adj_list_chunk(e, adj, 0, 0));
        named.push(paths::offset_chunk(e, adj, 0));
        named.push(paths::edge_count(e, adj, 0));
        named.push(paths::edge_vertex_count(e, adj));
    }
    named.sort();
    assert_eq!(
        tree(&root),
        named,
        "the destination holds what the information files name and nothing else"
    );
    assert_eq!(written.dropped.len(), g.edges.len());

    // 2. A vertex's index is its row: `0..V` once each, per type.
    let total: i64 = g
        .vertices
        .iter()
        .map(|v| count(paths::vertex_count(v)))
        .sum();
    assert_eq!(total, 1_600, "400 people and 1,200 orders");
    for v in &g.vertices {
        let p = at(&paths::vertex_chunk(v, &v.property_groups[0], 0));
        let n = count(paths::vertex_count(v));
        assert_eq!(
            scalar(&conn, &format!("SELECT count(*) FROM read_parquet({p})")),
            n,
            "{} declares its count",
            v.r#type
        );
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM read_parquet({p}, file_row_number = true) \
                     WHERE _graphArVertexIndex <> file_row_number"
                )
            ),
            0,
            "{}'s index is its row",
            v.r#type
        );
        assert_eq!(v.chunk_size as i64, n, "{} is one chunk", v.r#type);

        // 3. The columns are the properties, in order, the index first.
        let described = text(
            &conn,
            &format!(
                "SELECT string_agg(column_name, ',' ORDER BY rowid) FROM (SELECT column_name, row_number() OVER () AS rowid FROM (DESCRIBE SELECT * FROM read_parquet({p})))"
            ),
        );
        let declared: Vec<&str> = v.property_groups[0]
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            described,
            format!("_graphArVertexIndex,{}", declared.join(",")),
            "{} declares its columns",
            v.r#type
        );
        assert_eq!(declared[0], "subject");
        assert!(v.property_groups[0].properties[0].is_primary);

        // 4. `subject` is unique within the table.
        assert_eq!(
            scalar(
                &conn,
                &format!("SELECT count(DISTINCT subject) FROM read_parquet({p})")
            ),
            n
        );
    }

    // 5. An adjacency list's endpoints are indices of the types it names, it is
    //    sorted by (src, dst), and its offsets are the source type's count + 1.
    for e in &g.edges {
        let adj = &e.adj_lists[0];
        let p = at(&paths::adj_list_chunk(e, adj, 0, 0));
        let edges = count(paths::edge_count(e, adj, 0));
        assert_eq!(
            text(
                &conn,
                &format!(
                    "SELECT string_agg(column_name, ',') FROM (DESCRIBE SELECT * FROM read_parquet({p}))"
                )
            ),
            "_graphArSrcIndex,_graphArDstIndex"
        );
        assert_eq!(
            scalar(&conn, &format!("SELECT count(*) FROM read_parquet({p})")),
            edges
        );
        assert!(edges > 0, "{} is not vacuous", e.triple());
        for (column, ty) in [
            ("_graphArSrcIndex", &e.src_type),
            ("_graphArDstIndex", &e.dst_type),
        ] {
            let v = g.vertices.iter().find(|v| &v.r#type == ty).expect("a type");
            assert_eq!(
                scalar(
                    &conn,
                    &format!(
                        "SELECT count(*) FROM read_parquet({p}) WHERE {column} NOT BETWEEN 0 AND {}",
                        count(paths::vertex_count(v)) - 1
                    )
                ),
                0,
                "every {column} of {} is an index of {ty}",
                e.triple()
            );
        }
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT _graphArSrcIndex AS src, _graphArDstIndex AS dst, \
                     lag(_graphArSrcIndex) OVER w AS ps, lag(_graphArDstIndex) OVER w AS pd \
                     FROM read_parquet({p}, file_row_number = true) WINDOW w AS (ORDER BY file_row_number)) \
                     WHERE ps IS NOT NULL AND (ps > src OR (ps = src AND pd >= dst))"
                )
            ),
            0,
            "{} is sorted by (src, dst)",
            e.triple()
        );
        let sources = count(paths::edge_vertex_count(e, adj));
        let offsets = at(&paths::offset_chunk(e, adj, 0));
        assert_eq!(
            scalar(
                &conn,
                &format!("SELECT count(*) FROM read_parquet({offsets})")
            ),
            sources + 1,
            "{}'s offsets are one more than its sources",
            e.triple()
        );
        // The offset of source v is the number of edges whose source is below v.
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT o.i AS v, o.off AS off, \
                       (SELECT count(*) FROM read_parquet({p}) WHERE _graphArSrcIndex < o.i) AS below \
                     FROM (SELECT file_row_number AS i, _graphArOffset AS off \
                           FROM read_parquet({offsets}, file_row_number = true)) o \
                     WHERE o.i < {sources}) WHERE off <> below"
                )
            ),
            0,
            "{}'s offset of source v is the number of edges before it",
            e.triple()
        );
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT _graphArOffset FROM read_parquet({offsets}) ORDER BY _graphArOffset DESC LIMIT 1"
                )
            ),
            edges,
            "the last offset is one past the last edge"
        );
    }

    // 6. The sorted keys are delta-encoded.
    for (path, column) in g
        .vertices
        .iter()
        .map(|v| {
            (
                paths::vertex_chunk(v, &v.property_groups[0], 0),
                "_graphArVertexIndex",
            )
        })
        .chain(g.edges.iter().map(|e| {
            (
                paths::adj_list_chunk(e, &e.adj_lists[0], 0, 0),
                "_graphArSrcIndex",
            )
        }))
    {
        let encodings = text(
            &conn,
            &format!(
                "SELECT string_agg(DISTINCT encodings, ',') FROM parquet_metadata({}) WHERE path_in_schema = '{column}'",
                at(&path)
            ),
        );
        assert!(
            encodings.contains("DELTA_BINARY_PACKED"),
            "{path}.{column} is {encodings}"
        );
    }

    // 7. The counts are the footer's own `Statistics`: a chunk is one row
    //    group, so a row group's counts are the chunk's, and `distinct_count`
    //    is beside the nulls. No chunk carries a key of fossil's.
    for v in &g.vertices {
        let p = at(&paths::vertex_chunk(v, &v.property_groups[0], 0));
        assert_eq!(
            scalar(
                &conn,
                &format!("SELECT count(DISTINCT row_group_id) FROM parquet_metadata({p})")
            ),
            1,
            "{} is one row group",
            v.r#type
        );
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT stats_distinct_count FROM parquet_metadata({p}) WHERE path_in_schema = 'subject'"
                )
            ),
            count(paths::vertex_count(v)),
            "{}'s subjects are distinct, and the footer says how many",
            v.r#type
        );
    }
    for p in g
        .vertices
        .iter()
        .map(|v| paths::vertex_chunk(v, &v.property_groups[0], 0))
        .chain(
            g.edges
                .iter()
                .map(|e| paths::adj_list_chunk(e, &e.adj_lists[0], 0, 0)),
        )
    {
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM parquet_kv_metadata({}) WHERE key::VARCHAR LIKE 'fossil:%'",
                    at(&p)
                )
            ),
            0,
            "{p} carries no key of fossil's"
        );
    }

    // 8. The payload half: a value travels with its row, and an edge connects
    //    the identities it was written from. Every check above is about counts,
    //    indices and order, so a writer that renumbered the rows and attached each
    //    name to the wrong vertex, or pointed each edge at the wrong index, keeps
    //    them all. Joined through `subject`, never the index: the index is an
    //    address, and the IRI is what a stranger holds.
    let ordinal = |column: &str| format!("regexp_extract({column}, '([0-9]+)$', 1)");
    let chunk = |ty: &str| {
        let v = g.vertices.iter().find(|v| v.r#type == ty).expect("a type");
        paths::vertex_chunk(v, &v.property_groups[0], 0)
    };
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM read_parquet({}) WHERE name <> 'person ' || {}",
                at(&chunk("Person")),
                ordinal("subject")
            )
        ),
        0,
        "a vertex carries a name that was not built from its own identity"
    );
    let orders_csv = root.join("orders.csv");
    std::fs::write(&orders_csv, &sources(400, 1_200)[1].1).expect("write orders.csv");
    let placed_by = g
        .edges
        .iter()
        .find(|e| e.triple() == "Order_placedBy_Person")
        .expect("the relation");
    let written = format!(
        "SELECT {} AS o, {} AS u FROM read_parquet({}) e \
         JOIN read_parquet({}) s ON s._graphArVertexIndex = e._graphArSrcIndex \
         JOIN read_parquet({}) d ON d._graphArVertexIndex = e._graphArDstIndex",
        ordinal("s.subject"),
        ordinal("d.subject"),
        at(&paths::adj_list_chunk(
            placed_by,
            &placed_by.adj_lists[0],
            0,
            0
        )),
        at(&chunk("Order")),
        at(&chunk("Person")),
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

    // 9. The graph document is the last object written.
    let puts = out.puts.lock().expect("lock").clone();
    assert_eq!(
        puts.last().map(String::as_str),
        Some("run-1/fossil.graph.yml"),
        "{puts:?}"
    );
    // The mapping follows every table it describes and precedes the information
    // files, which the graph document closes.
    let mapping = puts
        .iter()
        .position(|p| p == "run-1/mapping.ttl")
        .unwrap_or_else(|| panic!("mapping.ttl was never written: {puts:?}"));
    let last_table = puts
        .iter()
        .rposition(|p| p.contains("/vertex/") || p.contains("/edge/"))
        .expect("tables were written");
    let first_info = puts
        .iter()
        .position(|p| {
            std::path::Path::new(p)
                .extension()
                .is_some_and(|e| e == "yml")
        })
        .expect("information files were written");
    assert!(
        last_table < mapping && mapping < first_info,
        "tables, then mapping.ttl, then the information files: {puts:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A table past parquet-rs's default row group (1,048,576 rows is its cap, and
/// 122,880 was fossil's own before) is still one row group: `graphar-parquet`
/// writes a chunk as one, so the footer's `Statistics` are the chunk's, and a
/// reader prunes by chunk.
#[tokio::test]
async fn a_chunk_past_122_880_rows_is_still_one_row_group() {
    let rows = 122_880 + 7_000;
    let (result, out) = run(PEOPLE_ONLY, sources(rows, 0), usize::MAX).await;
    result.expect("the write");
    let root = materialise(&out, "row_groups").await;
    let conn = Connection::open_in_memory().expect("duckdb");
    let g = graph(&root);
    let person = &g.vertices[0];
    let p = format!(
        "'{}'",
        root.join(paths::vertex_chunk(person, &person.property_groups[0], 0))
            .display()
    );
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(DISTINCT row_group_id) FROM parquet_metadata({p})")
        ),
        1
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT stats_distinct_count FROM parquet_metadata({p}) WHERE path_in_schema = '_graphArVertexIndex'"
            )
        ),
        i64::from(rows)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A write that fails part of the way leaves chunks with no graph document over
/// them, which no reader opens — never a document over missing tables.
#[tokio::test]
async fn a_write_that_fails_leaves_no_graph_document() {
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
                .all(|m| !m.location.as_ref().ends_with("fossil.graph.yml")),
            "a failed write at put {fail_at} left a graph document: {listed:?}"
        );
    }
}

/// A type with no rows is a type: its information file and its count of zero,
/// no chunk. A graph an empty source produced is still a graph a reader opens.
#[tokio::test]
async fn a_type_with_no_rows_is_still_a_valid_graph() {
    let none = PEOPLE_ONLY.replace("from users", "from users.where(users.id < 0)");
    let (result, out) = run(&none, sources(3, 0), usize::MAX).await;
    let written = result.expect("the write");
    let root = materialise(&out, "empty").await;
    let g = graph(&root);
    assert_eq!(g, written.info);
    let person = &g.vertices[0];
    assert_eq!(person.chunk_size, 1, "a chunk is never zero rows");
    assert_eq!(
        tree(&root),
        [
            "Person.vertex.yml",
            "fossil.graph.yml",
            "mapping.ttl",
            "vertex/Person/vertex_count"
        ],
        "a count and no chunk"
    );
    assert_eq!(
        std::fs::read(root.join("vertex/Person/vertex_count")).expect("count"),
        0_i64.to_le_bytes()
    );
    let _ = std::fs::remove_dir_all(&root);
}
