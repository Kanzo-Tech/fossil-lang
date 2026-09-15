//! Integration: renumbering `dense_id` into Morton order must not change the
//! graph, and must leave every file the manifest describes actually describing it.
//!
//! These four assertions exist because each corresponding failure is **silent**.
//! Renumbering rewrites `UINTEGER` columns with other perfectly valid
//! `UINTEGER`s, so a mistake anywhere here produces a well-formed corpus that
//! answers queries and means something else. Nothing throws. The only way to
//! catch it is to state the invariants and check them.
//!
//! **The assertions are `DuckDB`; the pass under test is not.** `enrich_layout`
//! writes Parquet through `arrow-rs`, and what reads those bytes back here is a
//! second engine. That is worth keeping rather than porting: a corpus only one
//! writer can read is not a corpus, and the assertions below are the only place
//! anything checks that what this pass emits is Parquet in the sense the rest of
//! the world means.
//!
//! The **fixture** was `DuckDB` too — three `COPY … TO … (FORMAT PARQUET)`
//! statements, because the pass opened files. It takes `RecordBatch`es now, so
//! the input is built in Arrow and `DuckDB` is on one side of the pass instead of
//! both.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use duckdb::Connection;
use fossil_layout::layout::{AdjacencyTarget, Endpoint, LayoutError, VertexLayoutTarget};

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

/// A fresh, empty directory under `std::env::temp_dir()`, named
/// `fossil_layout_<name>_<pid>`.
///
/// **The `<pid>` is load-bearing**, for the same reason it is in
/// `fossil-cli/tests/common/mod.rs`. The directory is wiped before it is
/// seeded, so a path keyed only on the test name is shared by every process on
/// the machine running this binary, and a second `cargo test` deletes this
/// one's fixtures between the `COPY` that writes them and the read that checks
/// them. The failures are not honest about their cause: `write vertices: IO
/// Error: Cannot open file …`, or — worse, because it looks like a real defect
/// in the pass — `renumbering changed which subjects are connected` with three
/// surviving edges out of seven. Measured 2026-08-23: six concurrent copies of
/// this binary, five rounds, thirty processes, thirty failures; the same binary
/// alone is green. Keep the path unique per process.
///
/// The wipe stays even so: pids are reused, and a reused one must not inherit
/// the previous run's chunk files, which assertion 3 counts.
fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("fossil_layout_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create test dir");
    path
}

fn lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// The input a single-type corpus consists of, as the executor would hand it
/// over: `dense_id` in IRI order, subjects `s0`…`s5`, and the placeholder layout
/// columns the writer emits.
///
/// Held by the caller and borrowed by [`targets`], because a struct carrying a
/// target beside the batches it points at is self-referential.
struct Corpus {
    vertices: Vec<RecordBatch>,
    by_source: Vec<RecordBatch>,
    by_target: Vec<RecordBatch>,
}

fn corpus(edges: &[(u32, u32)]) -> Corpus {
    let schema = Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]));
    let subjects: Vec<String> = (0..6).map(|i| format!("s{i}")).collect();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from((0..6u32).collect::<Vec<_>>())),
        Arc::new(StringArray::from(subjects)),
        Arc::new(Float32Array::from(vec![0.0f32; 6])),
        Arc::new(Float32Array::from(vec![0.0f32; 6])),
        Arc::new(UInt32Array::from(vec![0u32; 6])),
    ];
    Corpus {
        vertices: vec![RecordBatch::try_new(schema, columns).expect("the vertex batch")],
        by_source: orientation(edges, Endpoint::Src),
        by_target: orientation(edges, Endpoint::Dst),
    }
}

/// One orientation, ordered by the endpoint it declares — which is what the
/// manifest's `ordered: true` claims and what `LayoutError::Disordered` refuses
/// it for otherwise.
fn orientation(edges: &[(u32, u32)], ordered_by: Endpoint) -> Vec<RecordBatch> {
    let mut rows = edges.to_vec();
    match ordered_by {
        Endpoint::Src => rows.sort_unstable_by_key(|&(s, d)| (s, d)),
        Endpoint::Dst => rows.sort_unstable_by_key(|&(s, d)| (d, s)),
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from(
            rows.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|&(_, d)| d).collect::<Vec<_>>(),
        )),
    ];
    vec![RecordBatch::try_new(schema, columns).expect("the adjacency batch")]
}

/// Where the pass is to write: the payload under `chunks/`, each orientation's
/// tiles under a directory named after it, and the relation's level sets at the
/// root.
///
/// The two prefixes are passed in where they used to be derived by stripping
/// `.parquet` off the input's URL — the input has no URL.
fn targets<'a>(
    c: &'a Corpus,
    root: &Path,
    chunks: &Path,
) -> (Vec<VertexLayoutTarget<'a>>, Vec<AdjacencyTarget<'a>>) {
    let prefix = |p: &Path| format!("{}{}", p.to_string_lossy(), std::path::MAIN_SEPARATOR);
    let adjacency = |dir: &str, ordered_by, batches: &'a Vec<RecordBatch>| AdjacencyTarget {
        src_type: "Node".to_string(),
        label: "edge".to_string(),
        dst_type: "Node".to_string(),
        ordered_by,
        batches,
        tile_prefix: prefix(&root.join(dir)),
        levels_prefix: prefix(root),
    };
    (
        vec![VertexLayoutTarget {
            type_name: "Node".to_string(),
            batches: &c.vertices,
            chunk_prefix: prefix(chunks),
            // Two rows per chunk over six vertices, so the emission is exercised
            // as three chunks and a boundary rather than as one file wearing a
            // chunk's name.
            chunk_size: 2,
            // Four vertices per cell, and not the default sixteen, so that six
            // vertices earn a pyramid at all: at the default the whole type is
            // one cell and there is nothing to summarise. Assertion 3 below is
            // what notices that the tree appeared.
            vertices_per_cell: Some(4),
        }],
        vec![
            adjacency("by_source", Endpoint::Src, &c.by_source),
            adjacency("by_target", Endpoint::Dst, &c.by_target),
        ],
    )
}

/// The graph the fixture states, as pairs of subjects — the one description of
/// it that renumbering is not allowed to change.
///
/// Stated rather than derived. This used to be a `DuckDB` join over the staged
/// vertex and adjacency Parquet, described in place as «the only point at which
/// that file is the source of truth»; with no file, the source of truth is the
/// constant at the top of this file.
fn expected_edges(edges: &[(u32, u32)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = edges
        .iter()
        .map(|&(a, b)| (format!("s{a}"), format!("s{b}")))
        .collect();
    pairs.sort();
    pairs
}

/// One payload set as one relation — what a `GraphAr` reader sees.
///
/// It was `format!("{}/*.parquet", …)` and a star is a directory listing, which
/// is the one thing the corpus is designed so that nobody performs. A set is one
/// file now; the tiles inside it are its row groups.
fn payload(prefix: &Path) -> String {
    lit(&prefix.join("tiles.parquet"))
}

/// Edges as pairs of **subjects**, which is the one description of the graph
/// that renumbering is not allowed to change.
fn edges_by_subject(conn: &Connection, all: &str, adjacency: &str) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT s.subject, d.subject FROM read_parquet('{adjacency}') e \
             JOIN read_parquet('{all}') s ON s.dense_id = e.src_dense \
             JOIN read_parquet('{all}') d ON d.dense_id = e.dst_dense \
             ORDER BY 1, 2",
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
    let c = corpus(&EDGES);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    // The graph as it stands before the layout touches it.
    let before = expected_edges(&EDGES);
    let (v, a) = targets(&c, &root, &chunks);
    fossil_layout::layout::enrich_layout(&v, &a).expect("enrich_layout");

    // Where each orientation's tiles went — the `tile_prefix` the targets above
    // declared. Nothing else is in this tree: the pass is the only thing that
    // wrote into it.
    let src_tiles = root.join("by_source");
    let dst_tiles = root.join("by_target");

    // 1. The graph is the same graph. Ids changed; who is connected to whom did
    //    not. This is the assertion a missed adjacency file fails.
    assert_eq!(
        before,
        edges_by_subject(&conn, &payload(&chunks), &payload(&src_tiles)),
        "renumbering changed which subjects are connected",
    );
    assert_eq!(
        before,
        edges_by_subject(&conn, &payload(&chunks), &payload(&dst_tiles)),
        "by_target disagrees with by_source about the graph",
    );

    // 2. Ids stay dense and gap-free — a chunk range means nothing otherwise.
    let all = payload(&chunks);
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

    // 3. Each ROW GROUP holds exactly the dense_id range GraphAr says it does,
    //    and holds it in order. This is what the whole renumbering was for: tile
    //    k is [k*size, (k+1)*size), so unless the ids land that way an ordinal
    //    is a number and not an address. Six vertices at two per tile is three
    //    row groups, in one file.
    //
    //    It read three `chunk{k}.parquet` files. The container changed and the
    //    property did not: what makes a footer an index is one box per tile, and
    //    a file boundary between the tiles only costs requests.
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM (SELECT dense_id, row_number() OVER () - 1 AS pos \
                 FROM read_parquet('{all}')) WHERE dense_id <> pos"
            )
        ),
        0,
        "the payload is not the dense_id range [0, 6) in order",
    );
    let groups: Vec<(i64, i64)> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT DISTINCT row_group_id, row_group_num_rows \
                 FROM parquet_metadata('{all}') ORDER BY 1"
            ))
            .expect("prepare footer read");
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("read the footer");
        rows.map(|r| r.expect("row group")).collect()
    };
    assert_eq!(
        groups,
        vec![(0, 2), (1, 2), (2, 2)],
        "six vertices at a chunk_size of 2 is three row groups of two",
    );

    // ONE Parquet in the prefix, and it is the payload. Two containers at once
    // is one too many — a reader that globs finds both — which is the convention
    // `apps/corpus`'s `declared-tiling` fires on. `index/`, `l1/` and `holon/`
    // are not a second one: they are directories. The list is exhaustive so that
    // a fifth entry cannot appear unremarked — and `holon/` is the entry that
    // proved it works, having appeared here the moment the pass grew a pyramid.
    let mut emitted: Vec<String> = fs::read_dir(&chunks)
        .expect("read the tile prefix")
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    emitted.sort();
    assert_eq!(
        emitted,
        vec![
            "holon".to_string(),
            "index".to_string(),
            "l1".to_string(),
            "tiles.parquet".to_string(),
        ],
        "the tile prefix holds the payload, the index, the one level six vertices at two a \
         tile earn, the cell pyramid, and nothing else",
    );

    // **The pyramid, as the arithmetic names it.** Six vertices at four per cell
    // is two cells and then one, so two rungs — and each is a directory with a
    // tile set in it. Asserted by listing rather than by opening, because what
    // the rungs HOLD is `tests/cells.rs`, evaluated against the predicate; this
    // is the assertion that they are where the manifest will say they are.
    let mut rungs: Vec<String> = fs::read_dir(chunks.join("holon"))
        .expect("read the tree prefix")
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    rungs.sort();
    assert_eq!(rungs, vec!["r1".to_string(), "r2".to_string()]);
    for rung in &rungs {
        assert!(
            chunks
                .join("holon")
                .join(rung)
                .join("tiles.parquet")
                .exists(),
            "{rung} has no tile set",
        );
    }

    // And the index the pass writes beside it, addressed the same way: tiled at
    // the same size over the SORTED order, so three row groups again.
    let index_groups = scalar(
        &conn,
        &format!(
            "SELECT count(DISTINCT row_group_id) FROM parquet_metadata('{}')",
            payload(&chunks.join("index"))
        ),
    );
    assert_eq!(
        index_groups, 3,
        "six vertices at a chunk_size of 2 is three payload tiles and, since the index is tiled at \
         the same size over the SORTED order, three index tiles — unless the fixture's chunk_size \
         differs, in which case this number is the one that has to move",
    );

    // 4. Both adjacency lists are sorted on the endpoint they declare. The
    //    manifest says `ordered: true`; the remap invalidates that order and
    //    re-sorting is what restores it.
    for (prefix, first, second) in [
        (&src_tiles, "src_dense", "dst_dense"),
        (&dst_tiles, "dst_dense", "src_dense"),
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
                    payload(prefix)
                )
            ),
            0,
            "{} is not ordered by {first}",
            prefix.display(),
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
    let mut edges = EDGES.to_vec();
    edges.push((0, 99)); // no such vertex
    let c = corpus(&edges);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    let (v, a) = targets(&c, &root, &chunks);
    let err = fossil_layout::layout::enrich_layout(&v, &a)
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
    let c = corpus(&EDGES);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    let (v, mut a) = targets(&c, &root, &chunks);
    // Retyped on BOTH orientations, so the relation is still one relation and
    // what is wrong with it is the type it names. Changing one would make the
    // two halves different relations and the failure would be the missing
    // counterpart instead.
    for target in &mut a {
        target.dst_type = "Nowhere".to_string();
    }
    let err = fossil_layout::layout::enrich_layout(&v, &a).expect_err("unknown type");
    assert!(
        matches!(err, LayoutError::UnknownVertexType { ref vertex_type, .. } if vertex_type == "Nowhere"),
        "expected the unknown type to be named, got {err:?}",
    );

    fs::remove_dir_all(&root).ok();
}

/// **The two orientations of a relation are matched on its key.**
///
/// A self-relation handed over source-ordered with no target-ordered half cannot
/// be laid out: the placement is undirected and reads each vertex's in-edges out
/// of the half that groups them by destination. Refusing is the only honest
/// answer — deriving the missing half would be a sort of the whole relation to
/// recover rows the writer already has.
///
/// The pairing used to compare the DIRECTORY of two Parquet URLs, which is a
/// property of a naming convention rather than of the relation. This is the test
/// that says it is the `(src_type, label, dst_type)` key: the surviving half here
/// keeps its directory and the error still fires, because what is missing is a
/// counterpart to a key.
#[test]
fn a_relation_with_one_orientation_is_refused() {
    let root = dir("one_orientation");
    let c = corpus(&EDGES);
    let chunks = root.join("chunks");
    fs::create_dir_all(&chunks).expect("chunk dir");

    let (v, mut a) = targets(&c, &root, &chunks);
    a.retain(|target| target.ordered_by == Endpoint::Src);
    let err = fossil_layout::layout::enrich_layout(&v, &a)
        .expect_err("a relation with no target-ordered half must not pass silently");
    assert!(
        matches!(err, LayoutError::MissingOrientation { ref target } if target == "Node_edge_Node"),
        "expected the RELATION to be named, got {err:?}",
    );

    fs::remove_dir_all(&root).ok();
}
