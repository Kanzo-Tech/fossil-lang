//! Integration: **the level files and the level predicate select the same rows.**
//!
//! Level `k` is defined as `dense_id % 2^k == 0` over the payload, and a written
//! `vertex/<Type>/l{k}/` is an optimisation of exactly that — a coarse camera
//! reading a small file instead of striding a large one. The whole reason the
//! pyramid can be added without becoming a second contract is that the two paths
//! agree, so a corpus with no levels draws the identical picture and only reads
//! more.
//!
//! **Nothing else in the tree can catch a divergence.** A level set is
//! well-formed Parquet with the same schema as the payload; a writer that wrote
//! every 63rd row, or the right rows in the wrong order, or the multiples of a
//! stride over the write order where the numbering has a hole in it, produces a
//! corpus that opens, answers, draws, and is a different graph at every zoom
//! level than the one the payload holds. There is no exception to throw. The
//! only way to catch it is to evaluate the predicate against the payload and
//! diff.
//!
//! **The assertions are `DuckDB`; the pass under test is not** — the same
//! division `layout_renumber.rs` keeps. `enrich_layout` writes Parquet through
//! arrow-rs and a second engine reads those bytes back, so this also says the
//! levels are Parquet in the sense the rest of the world means.

mod common;

use std::fs;
use std::path::Path;

use common::{dir, fixture};
use duckdb::Connection;
use fossil_layout::layout::enrich_layout;
use fossil_sinks::manifest::VertexLevels;

/// Rows in the fixture, and a tile size that puts it well over
/// `LEVEL_FLOOR_TILES` without making the test a benchmark. The floor is
/// expressed in TILES precisely so that it can be crossed by either term, and
/// crossing it with the small one is what keeps this a two-second test: 4,000
/// rows at 8 rows a tile is 500 tiles, where 4,096 rows a tile would need a
/// quarter of a million vertices to say the same thing.
const ROWS: u32 = 4_000;
/// A power of two, because a tile's address is a shift and not a division.
const CHUNK: u64 = 8;

fn lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn scalar(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).expect(sql)
}

/// **The property, both directions.**
///
/// `EXCEPT ALL` in one direction catches a level that dropped a row and in the
/// other a level that invented one; running only one of them passes over a file
/// that is a strict subset of the truth, which is exactly the failure a
/// too-clever stride produces.
#[test]
fn a_level_file_holds_exactly_what_the_level_predicate_selects() {
    let mut f = fixture(dir("levels"), ROWS, 14);
    f.targets[0].chunk_size = CHUNK;
    enrich_layout(&f.targets, &f.adjacencies).expect("the layout pass");

    let plan =
        VertexLevels::planned(u64::from(ROWS), CHUNK).expect("4,000 rows is above the floor");
    assert_eq!(
        plan.levels,
        vec![7, 8, 9],
        "the plan this corpus is checked against"
    );

    let chunks = f.root.join("chunks");
    let payload = lit(&chunks.join("tiles.parquet"));
    let db = Connection::open_in_memory().expect("duckdb");

    // Every column the pass writes, not just `dense_id`: a level that selected
    // the right ids and carried the wrong position for them draws a picture
    // that is wrong about where everything is, and an id-only diff is green.
    let cols = "dense_id, subject, x, y, cluster_id";

    for &level in &plan.levels {
        let step = 1u64 << level;
        let file = lit(&chunks.join(plan.level_prefix(level)).join("tiles.parquet"));
        let predicate =
            format!("SELECT {cols} FROM read_parquet('{payload}') WHERE dense_id % {step} = 0");
        let written = format!("SELECT {cols} FROM read_parquet('{file}')");

        let missing = scalar(
            &db,
            &format!("SELECT count(*) FROM ({predicate} EXCEPT ALL {written})"),
        );
        let invented = scalar(
            &db,
            &format!("SELECT count(*) FROM ({written} EXCEPT ALL {predicate})"),
        );
        assert_eq!(
            missing, 0,
            "level {level}: rows the predicate has and the file does not"
        );
        assert_eq!(
            invented, 0,
            "level {level}: rows the file has and the predicate does not"
        );

        // The count is the third statement rather than a consequence of the two
        // above, because `EXCEPT ALL` over a file with a duplicated row and a
        // missing one nets to zero in both directions.
        let expected = i64::try_from(VertexLevels::rows_at(u64::from(ROWS), level))
            .expect("a level of a 4,000-row type fits an i64");
        assert_eq!(
            scalar(&db, &format!("SELECT count(*) FROM read_parquet('{file}')")),
            expected,
            "level {level}: row count"
        );

        // And in the payload's own order, which is what makes a level tile a
        // contiguous `dense_id` range and therefore addressable by the same
        // arithmetic as the payload. A file holding the right rows shuffled is
        // a file whose tile `j` is not a range of anything.
        let out_of_place = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM (SELECT dense_id, row_number() OVER () - 1 AS pos \
                 FROM read_parquet('{file}')) WHERE dense_id <> pos * {step}"
            ),
        );
        assert_eq!(
            out_of_place, 0,
            "level {level}: rows not in ascending id order"
        );
    }
}

/// `k+1` is a strict subset of `k` **in the files**, which is the same claim the
/// predicate makes and the reason zooming in only ever adds. Checked against the
/// bytes rather than re-derived from the definition, because a writer that got
/// one level's predicate wrong breaks the nesting and nothing else.
#[test]
fn a_coarser_level_file_is_a_subset_of_the_finer_one() {
    let mut f = fixture(dir("levels_nest"), ROWS, 14);
    f.targets[0].chunk_size = CHUNK;
    enrich_layout(&f.targets, &f.adjacencies).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(ROWS), CHUNK).expect("above the floor");
    let chunks = f.root.join("chunks");
    let db = Connection::open_in_memory().expect("duckdb");

    for pair in plan.levels.windows(2) {
        let (fine, coarse) = (pair[0], pair[1]);
        let f_url = lit(&chunks.join(plan.level_prefix(fine)).join("tiles.parquet"));
        let c_url = lit(&chunks.join(plan.level_prefix(coarse)).join("tiles.parquet"));
        let escaped = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM (SELECT dense_id, x, y FROM read_parquet('{c_url}') \
                 EXCEPT ALL SELECT dense_id, x, y FROM read_parquet('{f_url}'))"
            ),
        );
        assert_eq!(
            escaped, 0,
            "level {coarse} has a vertex level {fine} does not"
        );
    }
}

/// Only the declared levels exist on disk, in both directions.
///
/// A file nobody declared is unreachable — the manifest is how a reader learns a
/// level is there, and there is no listing over HTTP to discover it with. A
/// declaration with no file behind it is a 404 in a camera. `planned` is the one
/// function both sides call, so this is what says the writer and that function
/// have not drifted apart.
#[test]
fn the_levels_on_disk_are_the_levels_the_plan_names() {
    let mut f = fixture(dir("levels_declared"), ROWS, 14);
    f.targets[0].chunk_size = CHUNK;
    enrich_layout(&f.targets, &f.adjacencies).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(ROWS), CHUNK).expect("above the floor");
    let chunks = f.root.join("chunks");

    let mut on_disk: Vec<String> = fs::read_dir(&chunks)
        .expect("the tile directory")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            (entry.path().is_dir() && name.starts_with(&plan.prefix) && name != "index")
                .then_some(name)
        })
        .collect();
    on_disk.sort();

    let mut declared: Vec<String> = plan
        .levels
        .iter()
        .map(|&k| plan.level_prefix(k).trim_end_matches('/').to_string())
        .collect();
    declared.sort();
    assert_eq!(on_disk, declared);

    for &k in &plan.levels {
        assert!(
            chunks
                .join(plan.level_prefix(k))
                .join("tiles.parquet")
                .is_file(),
            "level {k} is declared and its tiles are not there"
        );
    }
}

/// **Under the floor a type gets no pyramid, and that is not a regression.**
///
/// `fossil run examples/hello.fossil` writes five `Person` vertices and the
/// walking skeleton asserts them by content; a corpus that grew a directory here
/// would be a corpus paying for a view it serves in one range request. The
/// predicate still answers over the payload, which is the whole point of the
/// floor being safe to set high.
#[test]
fn a_small_type_writes_no_levels_at_all() {
    let f = fixture(dir("levels_floor"), 200, 14);
    assert!(VertexLevels::planned(200, f.targets[0].chunk_size).is_none());
    enrich_layout(&f.targets, &f.adjacencies).expect("the layout pass");

    let strays: Vec<String> = fs::read_dir(f.root.join("chunks"))
        .expect("the tile directory")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            (entry.path().is_dir() && name != "index").then_some(name)
        })
        .collect();
    assert!(strays.is_empty(), "a type under the floor grew {strays:?}");
}
