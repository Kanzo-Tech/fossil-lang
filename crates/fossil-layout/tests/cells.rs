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
//!
//! **And one property that is not an obligation against anything below.** All
//! five rows above are a summary against the data it summarises, and all five
//! are evaluated in SQL, which is set semantics: a rung holding exactly the right
//! rows in a different *order* satisfies every one of them and is a different
//! file. Determinism is that difference, so it is compared as bytes and needs a
//! second process rather than a second query —
//! `the_same_graph_writes_byte_identical_rungs_in_a_second_process_worth_of_work`,
//! at the foot of this file.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use common::tree;
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

/// **The partition the manifest publishes is an interval of `dense_id`**, which
/// is what makes it locatable by the same arithmetic everything else here is.
///
/// It is not a property of the pyramid and it is the pyramid's precondition. A
/// cell is `dense_id >> shift`, so a rung means something about the plane only
/// if the id axis follows the plane — and `cluster_id`, which is the one
/// partition a reader colours by, is the coarsest thing that has to survive the
/// trip. It does because of how the vertices were placed: `order_by_hierarchy`
/// makes a run of consecutive placement groups a subtree, and
/// `fossil_layout::layout::cluster_layout` gives each group one aligned block of
/// the quaternary off a frontier that never goes back, so a subtree is a run of
/// adjacent blocks and a run of adjacent blocks is a run of ids.
///
/// **What this goes red for** is every way of breaking that chain: a buddy
/// allocation that reuses the holes its alignment leaves, a placement that
/// ignores the hierarchy's order, and a grid whose cells are not powers of four
/// of each other. Each produces a corpus that opens, draws and answers, with a
/// reader colouring scattered packets and nothing saying so — which is the
/// defect `/docs/design/position` measured before the placement was changed:
/// 1,229 published groups against 55,712 that drew, and no way to tell.
///
/// # Why this is a budget and not a zero
///
/// The chain has one link the placement does not own. `morton_codes` quantises
/// each axis over the extent the positions turned out to have, independently —
/// so the blocks map onto the addressing's own grid exactly when the placement
/// filled a square, and the placement fills its square to within the margins of
/// the two groups at its corners. Measured: this fixture and com-DBLP both come
/// out at zero foreign ids, and a variant whose extent was 0.13% off square put
/// thirteen of 1,603 groups into two runs apiece. So what is asserted is the
/// mass — how much of the corpus falls inside a group's id range without
/// belonging to that group — against a budget a rounding does not reach and the
/// defect does: taking `order_by_hierarchy` out of the pass puts 853 ids on the
/// wrong side of it against a budget of forty, which is the same corpus and the
/// same placement with only the group numbering changed.
///
/// What would make it a zero is a published quantisation box, and that is a
/// change to the format rather than to the placement — `/docs/design/discarded`
/// carries it with what would bring it back.
#[test]
fn a_cluster_is_one_run_of_dense_id() {
    let root = dir("runs");
    let edges = planted(ROWS);
    let c = corpus(ROWS, &edges);
    let (v, a) = targets(&c, &root);
    enrich_layout(&v, &a).expect("the layout pass");

    let db = Connection::open_in_memory().expect("duckdb");
    let payload = lit(&root.join("payload").join("tiles.parquet"));

    // More than one group, or the property is vacuous: a partition of one is an
    // interval whatever the placement did.
    let groups = scalar(
        &db,
        &format!("SELECT count(DISTINCT cluster_id) FROM read_parquet('{payload}')"),
    );
    assert!(groups > 1, "one group is not a partition to check");

    // Every id inside a group's range that is not the group's own. Zero when
    // every group is one interval, and `rows × groups` in the limit where the
    // partition is scattered evenly over the whole axis.
    let foreign = scalar(
        &db,
        &format!(
            "SELECT coalesce(sum(hi - lo + 1 - n), 0) FROM (                SELECT count(*) AS n, min(dense_id) AS lo, max(dense_id) AS hi                FROM read_parquet('{payload}') GROUP BY cluster_id              )"
        ),
    );
    let budget = i64::from(ROWS) / 100;
    assert!(
        foreign <= budget,
        "{foreign} ids fall inside a group's range without belonging to it, over {groups} groups —          the budget is {budget}, and a partition scattered across the axis reaches {}",
        i64::from(ROWS) * groups,
    );

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

/// The directory a child process is to write its corpus into.
///
/// An environment variable and not an argument: the child is a libtest binary
/// and its argv belongs to the harness.
const DEST: &str = "FOSSIL_CELLS_DEST";

/// The writing half below, by the name libtest filters on.
const CHILD: &str = "one_corpus_written_where_the_parent_asked";

/// **The same graph writes the same pyramid in a second process, byte for byte.**
///
/// `/docs/design/holons` puts determinism first of the three properties it puts
/// on a holon, and names this gap in the same breath: everything above catches a
/// *wrong* summary, and what nothing caught is the same corpus summarising
/// *differently* twice. A holon whose referent moves between writes is not
/// navigable — a reader who descends, pans and ascends has to arrive back where
/// they were, and a bookmark has to still resolve tomorrow.
///
/// **Bytes, because the obligations above are sets.** Every one of them is a
/// `count(*)` over a join, and a rung holding exactly the right rows in a
/// different order satisfies all five: the cells are the same cells, and the
/// *file* is not the same file. That is not a cosmetic difference in a pyramid,
/// where the tiling is the addressing — a reader takes tile `j` by arithmetic on
/// a cell id, so rows that moved between tiles are a bookmark resolving to
/// somebody else's cells. So this reads the bytes, and it reads the quotients
/// with them, whose row order is a second run-length walk and a second chance to
/// differ.
///
/// **And a second process, which is the load-bearing half.**
/// `tests/holons.rs, the_same_graph_gives_the_same_holon_rows_twice` evaluates
/// the aggregation twice inside one process, which cannot rule out anything drawn
/// **once per process and then reused** — the pid, a seed behind a `OnceLock`, an
/// address the allocator settled on, a clock read at startup. Two calls in one
/// process see the same value of each and agree; two processes are free to
/// disagree. Those are the inputs a summariser reaches for by accident, so the
/// second run is `current_exe`: this binary, filtered to the one `#[ignore]`d
/// test below.
#[test]
fn the_same_graph_writes_byte_identical_rungs_in_a_second_process_worth_of_work() {
    let root = dir("determinism");
    let first = corpus_from_its_own_process(&root.join("first"));
    let second = corpus_from_its_own_process(&root.join("second"));

    // **The pyramid is in what is compared**, stated as the two files that carry
    // it. Without this the test is green on a walk that reached neither: two
    // identical payloads and no rung between them would report the summary
    // deterministic without having read one.
    let holon = Path::new("payload").join("holon").join("r1");
    let wanted = [
        holon.join("tiles.parquet"),
        holon.join("quotient").join("tiles.parquet"),
    ];
    for path in &wanted {
        let path = path.to_string_lossy();
        assert!(
            first
                .iter()
                .any(|(seen, bytes)| *seen == path && !bytes.is_empty()),
            "`{path}` is not among the files compared: {:?}",
            first.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        );
    }

    assert_eq!(
        first.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        second.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        "the two processes wrote different files",
    );
    for ((path, left), (_, right)) in first.iter().zip(&second) {
        if let Some(at) = first_difference(left, right) {
            let show = |bytes: &[u8]| {
                bytes
                    .get(at)
                    .map_or_else(|| "end of file".to_string(), |b| format!("0x{b:02x}"))
            };
            panic!(
                "`{path}` differs between the two processes at byte {at}: {} against {}, \
                 over {} and {} bytes",
                show(left),
                show(right),
                left.len(),
                right.len(),
            );
        }
    }

    fs::remove_dir_all(&root).ok();
}

/// **One corpus, written by a process of its own**, as `(relative path, bytes)`.
///
/// `current_exe` is this test binary, so the second process needs no second
/// fixture and no `main` of its own — it runs [`CHILD`] and nothing else.
///
/// The `1 passed` check is not belt-and-braces. **A libtest filter that matches
/// nothing exits zero**, so renaming the function without renaming [`CHILD`]
/// would leave this comparing two empty directories, and every assertion in the
/// caller would hold.
fn corpus_from_its_own_process(dest: &Path) -> Vec<(String, Vec<u8>)> {
    let binary = std::env::current_exe().expect("this test binary's own path");
    let out = Command::new(&binary)
        .args([CHILD, "--exact", "--ignored", "--nocapture"])
        .env(DEST, dest)
        .output()
        .unwrap_or_else(|e| panic!("spawning {} failed: {e}", binary.display()));
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(out.status.success(), "the child process failed:\n{log}");
    assert!(
        log.contains("1 passed"),
        "the child harness ran no test — `{CHILD}` is the name it filters on:\n{log}",
    );
    tree(dest)
}

/// The offset of the first byte two files disagree at, or `None` where they
/// agree whole.
///
/// The offset and not a `bool`, because "the trees differ" is not a finding. A
/// rung is one Parquet, and the offset is what separates a column's values from a
/// row-group boundary from the footer — which is the difference between a
/// summary that was computed differently and a tiling that landed differently.
fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    if let Some(at) = left.iter().zip(right).position(|(a, b)| a != b) {
        return Some(at);
    }
    (left.len() != right.len()).then_some(left.len().min(right.len()))
}

/// **The writing half of the test above: one process, one corpus, and no claim
/// about it.**
///
/// `#[ignore]` rather than a binary target or a `cfg`: the parent needs a second
/// process running *this* fixture, and the fixture is in this file. It is ignored
/// because it is not a test — it writes what the parent reads, and the parent is
/// what chooses where.
///
/// The one thing it does assert is that a pyramid was written at all. Two corpora
/// with no rung in them compare equal, and this is the cheaper place to notice
/// than the caller's file list.
#[test]
#[ignore = "the writing half of the determinism test, which spawns it with --ignored"]
fn one_corpus_written_where_the_parent_asked() {
    // Unset means somebody ran `-- --ignored` by hand, which on this crate is
    // how `tests/levels.rs`'s instruments are run. There is nothing to write and
    // nothing to claim, and a panic would fail an invocation that was not asking
    // for this. It cannot go unnoticed where it matters: the parent sets the
    // variable and then asserts that one test ran and that the rungs are in what
    // it compared.
    let Some(dest) = std::env::var_os(DEST) else {
        eprintln!("{DEST} is unset, so there is nowhere to write: this is the writing half of");
        eprintln!("`the_same_graph_writes_byte_identical_rungs_in_a_second_process_worth_of_work`");
        return;
    };
    let dest = PathBuf::from(dest);
    fs::create_dir_all(&dest).expect("create the corpus directory");

    let edges = planted(ROWS);
    let c = corpus(ROWS, &edges);
    let (v, a) = targets(&c, &dest);
    let report = enrich_layout(&v, &a).expect("the layout pass");

    assert!(
        !report.pyramids.is_empty(),
        "this wrote no pyramid, and two corpora without one compare equal for the wrong reason",
    );
}
