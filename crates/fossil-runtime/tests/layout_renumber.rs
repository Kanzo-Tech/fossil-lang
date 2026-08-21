//! Integration: renumbering `dense_id` into Morton order must not change the
//! graph, and must leave every file the manifest describes actually describing it.
//!
//! These four assertions exist because each corresponding failure is **silent**.
//! Renumbering rewrites `UINTEGER` columns with other perfectly valid
//! `UINTEGER`s, so a mistake anywhere here produces a well-formed corpus that
//! answers queries and means something else. Nothing throws. The only way to
//! catch it is to state the invariants and check them.
//!
//! **The fixtures and the assertions are `DuckDB`; the pass under test is not.**
//! `enrich_layout` reads and writes Parquet through `arrow-rs`, and what is on
//! either side of it here is a second engine reading those bytes back. That is
//! worth keeping rather than porting: a corpus only one writer can read is a
//! corpus, and the assertions below are the only place anything checks that what
//! this pass emits is Parquet in the sense the rest of the world means.

use std::fs;
use std::path::{Path, PathBuf};

use duckdb::Connection;
use fossil_runtime::layout::{AdjacencyTarget, Endpoint, LayoutError, VertexLayoutTarget};

/// Two triangles joined by one edge — the smallest graph with communities to
/// find, so the layout actually moves the vertices and the renumbering is a
/// permutation rather than the identity.
const EDGES: [(u32, u32); 7] = [
    (0, 1),
    (0, 2),
    (1, 2), // triangle A
    (3, 4),
    (3, 5),
    (4, 5), // triangle B
    (0, 3), // the bridge
];

fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("fossil_layout_{name}"));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create test dir");
    path
}

fn lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// Write the three Parquets a single-type corpus consists of, with `dense_id`
/// in IRI order and the placeholder layout columns the W0b writer emits.
fn write_corpus(
    conn: &Connection,
    root: &Path,
    edges: &[(u32, u32)],
) -> (PathBuf, PathBuf, PathBuf) {
    let vertices = root.join("Node.parquet");
    let by_source = root.join("by_source.parquet");
    let by_target = root.join("by_target.parquet");

    let rows: Vec<String> = (0..6).map(|i| format!("({i}, 's{i}')")).collect();
    conn.execute_batch(&format!(
        "COPY (SELECT c0::UINTEGER AS dense_id, c1::VARCHAR AS subject, \
         0.0::REAL AS x, 0.0::REAL AS y, 0::UINTEGER AS cluster_id \
         FROM (VALUES {}) t(c0, c1) ORDER BY c0) TO '{}' (FORMAT PARQUET)",
        rows.join(", "),
        lit(&vertices)
    ))
    .expect("write vertices");

    let pairs: Vec<String> = edges.iter().map(|(a, b)| format!("({a}, {b})")).collect();
    for (path, order) in [(&by_source, "c0, c1"), (&by_target, "c1, c0")] {
        conn.execute_batch(&format!(
            "COPY (SELECT c0::UINTEGER AS src_dense, c1::UINTEGER AS dst_dense \
             FROM (VALUES {}) t(c0, c1) ORDER BY {order}) TO '{}' (FORMAT PARQUET)",
            pairs.join(", "),
            lit(path)
        ))
        .expect("write adjacency");
    }
    (vertices, by_source, by_target)
}

fn targets(
    vertices: &Path,
    by_source: &Path,
    by_target: &Path,
    chunks: &Path,
) -> (Vec<VertexLayoutTarget>, Vec<AdjacencyTarget>) {
    let adjacency = |p: &Path, ordered_by| AdjacencyTarget {
        parquet: p.to_string_lossy().into_owned(),
        src_type: "Node".to_string(),
        dst_type: "Node".to_string(),
        ordered_by,
    };
    (
        vec![VertexLayoutTarget {
            type_name: "Node".to_string(),
            vertex_parquet: vertices.to_string_lossy().into_owned(),
            chunk_prefix: format!("{}{}", chunks.to_string_lossy(), std::path::MAIN_SEPARATOR),
            // Two rows per chunk over six vertices, so the emission is exercised
            // as three chunks and a boundary rather than as one file wearing a
            // chunk's name.
            chunk_size: 2,
            self_edge_csr: vec![by_source.to_string_lossy().into_owned()],
        }],
        vec![
            adjacency(by_source, Endpoint::Src),
            adjacency(by_target, Endpoint::Dst),
        ],
    )
}

/// All chunks as one relation — what a `GraphAr` reader sees.
fn glob(chunks: &Path) -> String {
    format!("{}/*.parquet", lit(chunks))
}

/// Edges as pairs of **subjects**, which is the one description of the graph
/// that renumbering is not allowed to change.
fn edges_by_subject(conn: &Connection, all: &str, adjacency: &Path) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT s.subject, d.subject FROM read_parquet('{}') e \
             JOIN read_parquet('{all}') s ON s.dense_id = e.src_dense \
             JOIN read_parquet('{all}') d ON d.dense_id = e.dst_dense \
             ORDER BY 1, 2",
            lit(adjacency),
        ))
        .expect("prepare subject join");
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query subject join");
    rows.map(|r| r.expect("row")).collect()
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).expect("scalar query")
}

#[test]
fn renumbering_preserves_the_graph_and_the_order_the_manifest_declares() {
    let root = dir("renumber");
    let conn = Connection::open_in_memory().expect("duckdb");
    let (vertices, by_source, by_target) = write_corpus(&conn, &root, &EDGES);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    // The graph as it stands before the layout touches it, read off the writer's
    // single file — the only point at which that file is the source of truth.
    let before = edges_by_subject(&conn, &lit(&vertices), &by_source);
    let (v, a) = targets(&vertices, &by_source, &by_target, &chunks);
    fossil_runtime::layout::enrich_layout(&v, &a).expect("enrich_layout");

    // 1. The graph is the same graph. Ids changed; who is connected to whom did
    //    not. This is the assertion a missed adjacency file fails.
    assert_eq!(
        before,
        edges_by_subject(&conn, &glob(&chunks), &by_source),
        "renumbering changed which subjects are connected",
    );
    assert_eq!(
        before,
        edges_by_subject(&conn, &glob(&chunks), &by_target),
        "by_target disagrees with by_source about the graph",
    );

    // 2. Ids stay dense and gap-free — a chunk range means nothing otherwise.
    let all = glob(&chunks);
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(DISTINCT dense_id) FROM read_parquet('{all}')")
        ),
        6,
        "dense_id is not a gap-free permutation of 0..n-1",
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM read_parquet('{all}') WHERE dense_id NOT BETWEEN 0 AND 5"
            )
        ),
        0,
        "dense_id left the range 0..n-1",
    );

    // 3. Each chunk holds exactly the dense_id range GraphAr says it does, and
    //    holds it in order. This is what the whole renumbering was for: chunk k
    //    is [k*size, (k+1)*size), so unless the ids land that way a chunk is a
    //    file name and not a tile. Six vertices at two per chunk is three files.
    let mut found = 0;
    for k in 0..3u64 {
        let chunk = chunks.join(format!("chunk{k}.parquet"));
        assert!(chunk.exists(), "missing {}", chunk.display());
        found += 1;
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT dense_id, row_number() OVER () - 1 + {} AS pos \
                     FROM read_parquet('{}')) WHERE dense_id <> pos",
                    k * 2,
                    lit(&chunk)
                )
            ),
            0,
            "chunk {k} is not the dense_id range [{}, {}) in order",
            k * 2,
            k * 2 + 2,
        );
    }
    assert_eq!(found, 3);
    assert_eq!(
        fs::read_dir(&chunks).expect("read chunk dir").count(),
        3,
        "extra chunk files were emitted",
    );

    // 4. Both adjacency lists are sorted on the endpoint they declare. The
    //    manifest says `ordered: true`; the remap invalidates that order and
    //    re-sorting is what restores it.
    for (path, first, second) in [
        (&by_source, "src_dense", "dst_dense"),
        (&by_target, "dst_dense", "src_dense"),
    ] {
        // Stated as "file order equals sorted order" rather than as a
        // pairwise-descent check: comparing each row to the one before it via
        // `lag` reads naturally and is wrong, because DuckDB sorts NULL last, so
        // the first row — whose predecessor is NULL — compares as out of order
        // and every sorted file fails. Measured, not reasoned about.
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT row_number() OVER () AS pos, \
                     row_number() OVER (ORDER BY {first}, {second}) AS want \
                     FROM read_parquet('{}')) WHERE pos <> want",
                    lit(path)
                )
            ),
            0,
            "{} is not ordered by {first}",
            path.display(),
        );
    }

    fs::remove_dir_all(&root).ok();
}

/// A dangling endpoint has to be an error, because the rewrite is an inner join:
/// left alone it does not fail, it deletes the edge, and a corpus that is quietly
/// missing edges reads downstream as a sparser graph rather than as a bug.
#[test]
fn a_dangling_endpoint_is_an_error_and_not_a_missing_row() {
    let root = dir("dangling");
    let conn = Connection::open_in_memory().expect("duckdb");
    let mut edges = EDGES.to_vec();
    edges.push((0, 99)); // no such vertex
    let (vertices, by_source, by_target) = write_corpus(&conn, &root, &edges);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    let (v, a) = targets(&vertices, &by_source, &by_target, &chunks);
    let err = fossil_runtime::layout::enrich_layout(&v, &a)
        .expect_err("a dangling endpoint must not pass silently");
    assert!(
        matches!(err, LayoutError::DanglingEndpoint { dropped: 1, .. }),
        "expected one dropped row, got {err:?}",
    );

    fs::remove_dir_all(&root).ok();
}

/// An adjacency naming a type nobody laid out cannot be renumbered, and guessing
/// would mean writing ids from the wrong space.
#[test]
fn an_unknown_vertex_type_is_refused() {
    let root = dir("unknown_type");
    let conn = Connection::open_in_memory().expect("duckdb");
    let (vertices, by_source, by_target) = write_corpus(&conn, &root, &EDGES);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    let (v, mut a) = targets(&vertices, &by_source, &by_target, &chunks);
    a[0].dst_type = "Nowhere".to_string();
    let err = fossil_runtime::layout::enrich_layout(&v, &a).expect_err("unknown type");
    assert!(
        matches!(err, LayoutError::UnknownVertexType { ref vertex_type, .. } if vertex_type == "Nowhere"),
        "expected the unknown type to be named, got {err:?}",
    );

    fs::remove_dir_all(&root).ok();
}
