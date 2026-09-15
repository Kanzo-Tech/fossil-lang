//! Integration: **a cell and the level below it say the same thing.**
//!
//! `/docs/design/holons` states a summary row's whole contract as obligations
//! against the level underneath, and the reason they have to be *evaluated* is
//! that every way of getting one wrong produces a corpus that opens, addresses
//! and draws. An aggregate that dropped a child, or counted one twice, or kept
//! the edges between siblings instead of absorbing them, is a well-formed
//! pyramid summarising a graph nobody has. There is no exception to throw, and
//! the reader that would notice is the one asking the tree what is down there.
//!
//! So this is `levels.rs`'s standard one artefact along — compare the written
//! bytes against the definition rather than trusting the writer — and
//! `tests/holons.rs` is the same three obligations over an in-memory `Cut`,
//! where what stands in for a writer is a struct. This is the writer.
//!
//! **The assertions are `DuckDB` and the pass under test is not.** A second
//! engine reads the Parquet back, which is also the only thing in the tree that
//! says a rung is Parquet in the sense the rest of the world means.
//!
//! # What is checked, and what each one catches on its own
//!
//! | obligation | what it catches that nothing else does |
//! | --- | --- |
//! | a cell holds exactly the rows whose id shifts to it | a partition that is not the addressing |
//! | a cell's count is the sum of its four children's | a rung folded from the wrong level |
//! | cross weight + internal == the edges below | edges kept as self-loops, or dropped |
//! | a cell's position is its members' centroid | a tree whose referent moved |
//! | the mode is the majority and the purity is its share | a categorical averaged |
//!
//! The third is the one the page argues hardest for, and it is the only one that
//! sees a writer relabelling the children's edge set and keeping it — which on a
//! graph with any structure is not an edge case but the overwhelming majority.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use duckdb::Connection;
use fossil_layout::layout::{AdjacencyTarget, Endpoint, VertexLayoutTarget, enrich_layout};
use fossil_sinks::manifest::HolonTree;

/// Vertices, and a base small enough that four thousand of them span a pyramid
/// rather than one rung.
///
/// Four per cell, not the default sixteen: what this file evaluates is the
/// aggregation, and the aggregation is exercised by having several rungs of
/// several cells each. At sixteen the same corpus would be 250 → 63 → 16 → 4 →
/// 1, which is the same property over fewer steps and a slower test.
const ROWS: u32 = 4_000;
const PER_CELL: u64 = 4;
/// Rows per tile. Two, so a rung of five cells is three tiles and the tiling is
/// exercised at a boundary rather than at one file wearing a tile's name.
const CHUNK: u64 = 2;

/// A fresh directory, keyed on the pid for the reason `layout_renumber.rs`
/// spells out: the path is wiped before it is seeded, so a fixed one is shared
/// by every process running this binary and a second `cargo test` deletes this
/// one's fixture mid-assertion.
fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("fossil_cells_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create test dir");
    path
}

fn lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn prefix(path: &Path) -> String {
    format!("{}{}", path.to_string_lossy(), std::path::MAIN_SEPARATOR)
}

/// A graph with communities in it, so the placement is a real permutation and
/// the cells are not the identity.
///
/// Blocks of a hundred, most edges inside one — the same planted shape
/// `examples/enrich_memory` measures on, and for the same reason: Louvain on a
/// uniform random graph merges nothing, so the renumbering would be near enough
/// to the identity that a wrong partition would still look right.
fn planted(n: u32) -> Vec<(u32, u32)> {
    let block = 100u32;
    let mut edges = Vec::new();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for v in 0..n {
        for _ in 0..3 {
            let u = if next() % 10 == 0 {
                u32::try_from(next() % u64::from(n)).unwrap_or(0)
            } else {
                let base = (v / block) * block;
                base + u32::try_from(next() % u64::from(block.min(n - base))).unwrap_or(0)
            };
            if u != v {
                edges.push((v.min(u), v.max(u)));
            }
        }
    }
    // Distinct pairs, so "the edge count of the level below" is a number the
    // corpus and this file agree on without either of them deduplicating.
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// The input the pass takes, held so the targets can borrow it.
struct Corpus {
    vertices: Vec<RecordBatch>,
    by_source: Vec<RecordBatch>,
    by_target: Vec<RecordBatch>,
}

fn corpus(rows: u32, edges: &[(u32, u32)]) -> Corpus {
    let schema = Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]));
    let subjects: Vec<String> = (0..rows).map(|i| format!("s{i:06}")).collect();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from((0..rows).collect::<Vec<_>>())),
        Arc::new(StringArray::from(subjects)),
        Arc::new(Float32Array::from(vec![0.0f32; rows as usize])),
        Arc::new(Float32Array::from(vec![0.0f32; rows as usize])),
        Arc::new(UInt32Array::from(vec![0u32; rows as usize])),
    ];
    Corpus {
        vertices: vec![RecordBatch::try_new(schema, columns).expect("the vertex batch")],
        by_source: orientation(edges, Endpoint::Src),
        by_target: orientation(edges, Endpoint::Dst),
    }
}

fn orientation(edges: &[(u32, u32)], by: Endpoint) -> Vec<RecordBatch> {
    let mut rows = edges.to_vec();
    match by {
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

fn targets<'a>(
    c: &'a Corpus,
    root: &Path,
) -> (Vec<VertexLayoutTarget<'a>>, Vec<AdjacencyTarget<'a>>) {
    let adjacency = |dir: &str, ordered_by, batches: &'a Vec<RecordBatch>| AdjacencyTarget {
        src_type: "Node".to_string(),
        label: "near".to_string(),
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
            chunk_prefix: prefix(&root.join("payload")),
            chunk_size: CHUNK,
            vertices_per_cell: Some(PER_CELL),
        }],
        vec![
            adjacency("by_source", Endpoint::Src, &c.by_source),
            adjacency("by_target", Endpoint::Dst, &c.by_target),
        ],
    )
}

fn scalar(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).expect(sql)
}

/// One rung's tile set, as a `read_parquet` argument.
fn rung(root: &Path, at: u32) -> String {
    lit(&root
        .join("payload")
        .join("holon")
        .join(format!("r{at}"))
        .join("tiles.parquet"))
}

/// One rung's quotient, as a `read_parquet` argument.
fn quotient(root: &Path, at: u32) -> String {
    lit(&root
        .join("payload")
        .join("holon")
        .join(format!("r{at}"))
        .join("quotient")
        .join("tiles.parquet"))
}

/// **Everything the pyramid claims, against the payload it claims it about.**
///
/// One fixture and one pass, then every obligation over every rung — rather than
/// a test per obligation, because writing the corpus is the expensive part and
/// each assertion names which property it is.
#[test]
fn every_rung_says_what_the_level_below_it_says() {
    let root = dir("rungs");
    let edges = planted(ROWS);
    let c = corpus(ROWS, &edges);
    let (v, a) = targets(&c, &root);
    let report = enrich_layout(&v, &a).expect("the layout pass");

    let tree = &report
        .pyramids
        .iter()
        .find(|(label, _)| label == "Node")
        .expect("Node earned a pyramid")
        .1;
    let db = Connection::open_in_memory().expect("duckdb");
    let payload = lit(&root.join("payload").join("tiles.parquet"));
    let relation = lit(&root.join("by_source").join("tiles.parquet"));

    // The plan the manifest declares is the plan the arithmetic names. Asserted
    // against `holons_at` rather than against a list, because what a reader
    // reproduces is the division.
    for (index, declared) in tree.rungs.iter().enumerate() {
        let at = u32::try_from(index + 1).expect("a rung index");
        assert_eq!(
            declared.holon_count,
            HolonTree::holons_at(u64::from(ROWS), PER_CELL, at).expect("a valid base"),
            "rung {at} declares a count the base does not name",
        );
    }
    assert_eq!(
        tree.rungs.last().map(|r| r.holon_count),
        Some(1),
        "the tree runs to a single cell",
    );

    for (index, declared) in tree.rungs.iter().enumerate() {
        let at = u32::try_from(index + 1).expect("a rung index");
        let shift = HolonTree::shift_at(PER_CELL, at).expect("a valid base");
        let rows = rung(&root, at);

        // **Obligation 1 — a cell holds exactly the rows whose id shifts to it.**
        //
        // The membership and the addressing are the same statement under a
        // quaternary partition, which is the whole of what makes the tree
        // navigable: a reader descends by shifting. A rung whose counts came
        // from any other partition fails here, and nothing else would notice —
        // the file is well-formed and every count sums.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT count(*) FROM read_parquet('{rows}') c \
                     FULL OUTER JOIN ( \
                       SELECT dense_id >> {shift} AS cell_id, count(*) AS n \
                       FROM read_parquet('{payload}') GROUP BY 1 \
                     ) p USING (cell_id) \
                     WHERE coalesce(c.count, 0) <> coalesce(p.n, 0)"
                )
            ),
            0,
            "rung {at}: a cell's count is not the rows whose id shifts to it",
        );
        assert_eq!(
            scalar(&db, &format!("SELECT count(*) FROM read_parquet('{rows}')")),
            i64::try_from(declared.holon_count).expect("a row count"),
            "rung {at}: the file and the manifest disagree about how many cells there are",
        );
        // The ids are a gapless `0..n` in file order, which is what makes a cell
        // id addressable and a tile a range. An empty cell is a real row here.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT count(*) FROM (SELECT cell_id, row_number() OVER () - 1 AS pos \
                     FROM read_parquet('{rows}')) WHERE cell_id <> pos"
                )
            ),
            0,
            "rung {at}: the cell ids are not the range [0, n) in order",
        );

        // **Obligation 4 — the position is the members' centroid.**
        //
        // Declared rather than derived, which is why it is checked: a holon's
        // `derived_by` says `cell-member-centroid`, and re-deriving is how a
        // reader finds out the referent has not moved. Compared with a tolerance
        // because the corpus stores `f32` and the mean is taken in `f64`.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT count(*) FROM read_parquet('{rows}') c \
                     JOIN ( \
                       SELECT dense_id >> {shift} AS cell_id, avg(x) AS mx, avg(y) AS my \
                       FROM read_parquet('{payload}') GROUP BY 1 \
                     ) p USING (cell_id) \
                     WHERE abs(c.x - p.mx) > 1e-3 OR abs(c.y - p.my) > 1e-3"
                )
            ),
            0,
            "rung {at}: a cell is not at its members' centroid",
        );

        // **Obligation 5 — the mode is the majority and the purity is its share.**
        //
        // A category does not average, so a cell carries the majority value and
        // what fraction of it holds that value. Both, because a mode without a
        // purity is a lie at the coarse end — and the purity is what a reader
        // desaturates by, so a wrong one draws a confident blur.
        // Ranked rather than nested: a window over an aggregate inside a join
        // makes DuckDB's own subplan optimizer abort (`ConvertTableIndex`), and
        // the tie-break is explicit either way — highest count, then lowest
        // value, which is what the writer's ascending walk takes.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "WITH tally AS ( \
                       SELECT dense_id >> {shift} AS cell_id, cluster_id, count(*) AS n \
                       FROM read_parquet('{payload}') GROUP BY 1, 2 \
                     ), ranked AS ( \
                       SELECT cell_id, cluster_id, n, \
                              row_number() OVER ( \
                                PARTITION BY cell_id ORDER BY n DESC, cluster_id ASC \
                              ) AS rk \
                       FROM tally \
                     ) \
                     SELECT count(*) FROM read_parquet('{rows}') c \
                     JOIN (SELECT cell_id, cluster_id AS mode, n AS best FROM ranked WHERE rk = 1) p \
                       USING (cell_id) \
                     WHERE c.mode <> p.mode \
                        OR abs(c.purity - p.best::DOUBLE / c.count) > 1e-6"
                )
            ),
            0,
            "rung {at}: the mode is not the majority, or the purity is not its share",
        );

        // **Obligation 3 — mass conservation.**
        //
        // Cross weight plus internal weight equals the edge count of the level
        // below, at every rung. This is the one that catches both mistakes the
        // page names: a writer that drops the edges between siblings, and one
        // that keeps them as self-loops on the quotient. Neither changes a count
        // of cells, and neither is visible in a picture.
        let below = if at == 1 {
            format!("SELECT count(*) FROM read_parquet('{relation}')")
        } else {
            // The level below a rung is the rung below it, whose edge population
            // is its own quotient plus what IT absorbed.
            let under = rung(&root, at - 1);
            let under_q = quotient(&root, at - 1);
            format!(
                "SELECT (SELECT coalesce(sum(internal), 0) FROM read_parquet('{under}')) \
                      + (SELECT coalesce(sum(weight), 0) FROM read_parquet('{under_q}'))"
            )
        };
        let internal = scalar(
            &db,
            &format!("SELECT coalesce(sum(internal), 0) FROM read_parquet('{rows}')"),
        );
        let cross = declared.quotient.as_ref().map_or(0, |_| {
            scalar(
                &db,
                &format!(
                    "SELECT coalesce(sum(weight), 0) FROM read_parquet('{}')",
                    quotient(&root, at)
                ),
            )
        });
        assert_eq!(
            internal + cross,
            scalar(&db, &below),
            "rung {at}: {internal} absorbed + {cross} crossing is not the mass below it",
        );

        // And the quotient's own declaration: how many ROWS it has, which is
        // what tells a 404 from an empty one.
        if let Some(q) = &declared.quotient {
            assert_eq!(
                scalar(
                    &db,
                    &format!(
                        "SELECT count(*) FROM read_parquet('{}')",
                        quotient(&root, at)
                    )
                ),
                i64::try_from(q.edge_count).expect("an edge count"),
                "rung {at}: the quotient file and the manifest disagree",
            );
            // A quotient edge is between two DIFFERENT cells. A self-loop here
            // is mass that should have been absorbed, and it is the failure the
            // page measured at 1,088 of 1,095 on its own fixture.
            assert_eq!(
                scalar(
                    &db,
                    &format!(
                        "SELECT count(*) FROM read_parquet('{}') WHERE src_cell = dst_cell",
                        quotient(&root, at)
                    )
                ),
                0,
                "rung {at}: the quotient carries a self-loop",
            );
        }
    }

    // **Obligation 2 — a count is the sum of its children's.**
    //
    // An obligation on the ROW and not on the data, which is what makes it an
    // obligation at all: the count is recorded beside the members, so deriving
    // it from the members would make this a tautology no corruption can fail.
    for at in 2..=u32::try_from(tree.rungs.len()).expect("a rung count") {
        let here = rung(&root, at);
        let under = rung(&root, at - 1);
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT count(*) FROM read_parquet('{here}') h \
                     JOIN (SELECT cell_id >> 2 AS cell_id, sum(count) AS n \
                           FROM read_parquet('{under}') GROUP BY 1) u USING (cell_id) \
                     WHERE h.count <> u.n"
                )
            ),
            0,
            "rung {at}: a cell's count is not the sum of the four cells under it",
        );
    }

    fs::remove_dir_all(&root).ok();
}

/// **A type no bigger than one cell gets no pyramid**, and the report says so by
/// omission rather than by an empty tree.
///
/// The difference matters in the document: a manifest with no `holons:` block is
/// what a corpus written before the block existed reads as, and one with an
/// empty tree is a writer claiming a summary of nothing.
#[test]
fn a_type_that_is_one_cell_earns_no_pyramid() {
    let root = dir("one_cell");
    let edges = [(0u32, 1u32), (1, 2), (2, 3)];
    let c = corpus(4, &edges);
    let (v, a) = targets(&c, &root);
    let report = enrich_layout(&v, &a).expect("the layout pass");

    assert!(
        report.pyramids.is_empty(),
        "four vertices at four per cell is one cell, and one cell is not a summary",
    );
    assert!(
        !root.join("payload").join("holon").exists(),
        "no tree was declared, so no tree may be on disk",
    );

    fs::remove_dir_all(&root).ok();
}
