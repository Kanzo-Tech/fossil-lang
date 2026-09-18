//! Integration: **the level files and the level predicate select the same rows.**
//!
//! Level `k` is defined as `dense_id % 4^k == 0` over the payload, and a written
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
use fossil_sinks::manifest::{HOLON_PREFIX, LEVEL_PREFIX_STEM, VertexLevels};

/// Rows in the fixture, and a tile size that buys a five-level pyramid without
/// making the test a benchmark. A type earns levels the moment it is over one
/// tile, and the coarsest is the one that fits back into one — so it is the
/// RATIO that sets the depth, and taking it with the small term is what keeps
/// this a two-second test: 4,000 rows at 8 a tile spans five levels, where
/// 4,096 rows a tile would need four million vertices to say the same thing.
const ROWS: u32 = 4_000;
/// A power of two, because a tile's address is a shift and not a division.
const CHUNK: u64 = 8;

fn lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// **The relation the pass WROTE, not the one it was given.**
///
/// `enrich_layout` renumbers every vertex, so the batches the fixture hands it
/// carry the OLD `dense_id`s and the tiles it emits carry the new ones. Measured
/// on this fixture: 27,976 rows each and **55,698 of them differ** — which is all
/// of them. A test that reads the input and joins it to the written payload on
/// `dense_id` is joining two different numberings and gets an answer about
/// nothing; the edges it counts are not edges of this corpus.
///
/// It is harder to get wrong than it was, and worth saying why: the input used to
/// be a `by_source.parquet` sitting in the same directory tree as the output, so
/// reading the wrong one was a plausible path. There is no file to read.
///
/// That is not hypothetical. It is what
/// `what_the_pixel_floor_leaves_of_a_coarse_view` did on the day it was written,
/// and the table it printed was wrong in the direction that mattered: random
/// pairs are LONGER than real edges, so a length floor over a mis-joined
/// relation keeps far more of them than the corpus has.
fn written_relation(root: &Path, orientation: &str) -> String {
    lit(&root.join(orientation).join("tiles.parquet"))
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
    f.chunk_size = CHUNK;
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(ROWS), CHUNK).expect("4,000 rows is over one tile");
    assert_eq!(
        plan.levels,
        vec![1, 2, 3, 4, 5],
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
        let stride = VertexLevels::stride(level);
        let file = lit(&chunks.join(plan.level_prefix(level)).join("tiles.parquet"));
        let predicate =
            format!("SELECT {cols} FROM read_parquet('{payload}') WHERE dense_id % {stride} = 0");
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
                 FROM read_parquet('{file}')) WHERE dense_id <> pos * {stride}"
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
    f.chunk_size = CHUNK;
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(ROWS), CHUNK).expect("over one tile");
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
    f.chunk_size = CHUNK;
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(ROWS), CHUNK).expect("over one tile");
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

/// **A type that fits one tile gets no levels, and that is not a regression.**
///
/// `fossil run examples/hello.fossil` writes five `Person` vertices and the
/// walking skeleton asserts them by content; a corpus that grew a level
/// directory here would be a corpus paying for a view one range request already
/// serves. The predicate still answers over the payload, which is what makes the
/// one exclusion safe.
///
/// **The cell pyramid is not held to the same floor, and the difference is
/// argued rather than accidental.** This test used to assert that no directory
/// but `index` appeared at all, and `fossil_sinks::manifest::HolonTree` made it
/// red by writing a `holon/` beside it: a level is a transport optimisation, so
/// one tile is already one range request and coarser buys nothing, while a rung
/// is arithmetic — `ceil(V / 4^k)` names an artefact for every `k` a reader can
/// compute, so the tree has no data-dependent tail and no *this rung was not
/// written* branch for a reader to carry. That is the floor `HolonTree::planned`
/// argues for in its own doc. It is NOT that *a reader zoomed all the way out
/// wants four marks rather than two hundred*, which that doc used to say and
/// this one cited: no reader in this tree can ask for four marks, the smallest
/// mark budget anything spends being `SCREEN_MARKS`' 15,000 in `level_vs_rung.rs`
/// beside this file. The two floors differ on purpose, so this asserts the level
/// rule and lets the rung rule state itself.
#[test]
fn a_small_type_writes_no_levels_at_all() {
    let f = fixture(dir("levels_one_tile"), 200, 14);
    assert!(VertexLevels::planned(200, f.chunk_size).is_none());
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let dirs: Vec<String> = fs::read_dir(f.root.join("chunks"))
        .expect("the tile directory")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry
                .path()
                .is_dir()
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();

    let levels: Vec<&String> = dirs
        .iter()
        .filter(|name| {
            name.strip_prefix(LEVEL_PREFIX_STEM)
                .is_some_and(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    assert!(levels.is_empty(), "a type inside one tile grew {levels:?}");

    // And the rung pyramid IS there, at the floor that is its own: 200 rows over
    // a base of sixteen is a tree, where 200 rows in one tile is not a pyramid.
    let holon = HOLON_PREFIX.trim_end_matches('/');
    assert!(
        dirs.iter().any(|name| name == holon),
        "the cell pyramid a type of 200 earns is not there: {dirs:?}",
    );
}

/// **What the default view costs, measured** — an instrument rather than a
/// test, which is why it is `#[ignore]`d and CI never runs it.
///
/// It shares `common`'s fixture rather than copying a generator into
/// `examples/`, and it asserts nothing about a byte count: a compressor is
/// entitled to change its mind. What it reports is the ratio the pyramid exists
/// for — the bytes a camera reads to draw a level, against the bytes it reads to
/// stride the whole type for the same picture.
///
/// `cargo test -p fossil-layout --test levels -- --ignored --nocapture`
#[test]
#[ignore = "an instrument: it writes a 300,000-vertex corpus and measures it"]
#[allow(clippy::cast_precision_loss)] // a human-readable percentage in a println
fn level_cost_against_the_whole_type() {
    const BIG: u32 = 300_000;
    let f = fixture(dir("levels_cost"), BIG, 14);
    let chunk = f.chunk_size;
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(BIG), chunk).expect("over one tile");
    let chunks = f.root.join("chunks");
    let payload = fs::metadata(chunks.join("tiles.parquet"))
        .expect("payload")
        .len();
    let tiles = u64::from(BIG).div_ceil(chunk);
    println!("payload: {tiles} tiles, {} kB", payload / 1024);
    for &k in &plan.levels {
        let bytes = fs::metadata(chunks.join(plan.level_prefix(k)).join("tiles.parquet"))
            .expect("a level")
            .len();
        let rows = VertexLevels::rows_at(u64::from(BIG), k);
        println!(
            "l{k}: {rows} rows, {} tile(s), {} kB — {:.1}% of the payload",
            rows.div_ceil(chunk),
            bytes / 1024,
            (bytes as f64 / payload as f64) * 100.0
        );
    }
}

/// **What a coarse view actually draws, measured** — the number the decision to
/// read a level file rests on, and which nothing in this tree had measured.
///
/// The **induced** set — edges with BOTH ends in the level — is the right
/// question for *writing* an edge pyramid and the wrong one for *reading* a
/// vertex level, because the camera's rule is not the induced set. `view` keeps
/// an edge with **at least one end drawn** and both ends *positioned* — both in
/// the tiles it opened — and then the renderer's floor cuts the short ones. So
/// the edges a coarse view draws scale with the marks (`V/4^k · degree`), not
/// with their square, and the induced count says nothing about them.
///
/// That distinction is what decided whether a level file could answer at all.
/// The question is not *how many edges are induced* but **how many survive the
/// three-pixel floor at the zoom where the pyramid is used**, and this still
/// measures that.
///
/// # The premise underneath it was closed, and the number is answering an old question
///
/// This paragraph read: *a read of `l{k}/` holds the level's rows and nothing
/// else, so the far end of a mark-incident edge is not in it — reading the level
/// means those edges and their anchors go.* That was true when it was written
/// and stopped being true at commit `0003d13`, which gave a relation its own
/// level and put **both endpoints' coordinates on every row**, making the set
/// self-drawing. That commit did not come back and update this header.
///
/// So the percentages below — 14.31, 3.20, 0.74, 0.14 on the planted fixture —
/// are what a level read would lose **if a level of a relation did not exist**.
/// Against today's camera the answer is 100% at every level: an edge level draws
/// every mark-incident edge past the floor, because it carries the far end
/// itself. `crates/fossil-layout/tests/level_vs_rung.rs` reports both columns
/// side by side, and the vertex-only one reproduces these numbers exactly, which
/// is also how the two instruments are held to the same graph.
///
/// It is kept rather than corrected because the figure it prints is the cost of
/// NOT writing edge levels, and that is the number anyone weighing whether to
/// keep writing them needs.
///
/// The floor is the app's: `@kanzo-tech/graph`'s `BOUNDED_DEFAULTS.minLinkPixels`
/// is 3, `apps/playground/src/tiles.ts` multiplies it by the corpus units a
/// pixel covers, and at the whole extent on a 1,200-pixel canvas that is
/// `3 · width / 1200`.
///
/// Same fixture as `level_cost_against_the_whole_type`, deliberately: the byte
/// ratio and this ratio have to be read against each other, and two corpora
/// would make that a comparison between two graphs.
///
/// `cargo test -p fossil-layout --test levels -- --ignored --nocapture`
#[test]
#[ignore = "an instrument: it writes a 300,000-vertex corpus and measures it"]
#[allow(clippy::cast_precision_loss)] // human-readable ratios in a println
fn what_the_pixel_floor_leaves_of_a_coarse_view() {
    const BIG: u32 = 300_000;
    const CANVAS_PX: f64 = 1_200.0;
    const MIN_LINK_PX: f64 = 3.0;

    let f = fixture(dir("levels_camera"), BIG, 14);
    let chunk = f.chunk_size;
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(BIG), chunk).expect("over one tile");
    let payload = lit(&f.root.join("chunks").join("tiles.parquet"));
    let by_source = written_relation(&f.root, "by_source");
    let db = Connection::open_in_memory().expect("duckdb");

    // The extent the whole-extent view is drawn against, from the written
    // positions rather than from the generator's: the layout pass moves every
    // vertex, and a floor derived from the wrong extent is a floor in the wrong
    // units.
    let (min_x, max_x): (f64, f64) = db
        .query_row(
            &format!("SELECT min(x), max(x) FROM read_parquet('{payload}')"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the extent");
    let floor = MIN_LINK_PX * (max_x - min_x) / CANVAS_PX;
    let total = scalar(
        &db,
        &format!("SELECT count(*) FROM read_parquet('{by_source}')"),
    );
    println!(
        "corpus: {BIG} vertices, {total} edges, extent {min_x:.1}..{max_x:.1}, \
         floor {floor:.3} corpus units ({MIN_LINK_PX} px at {CANVAS_PX} px)"
    );

    for &level in &plan.levels {
        let stride = VertexLevels::stride(level);
        let marks = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM read_parquet('{payload}') WHERE dense_id % {stride} = 0"
            ),
        );
        // The camera's rule, spelled once and filtered three ways: at least one
        // end a mark, both ends positioned (the whole extent opens every tile,
        // so that is every vertex), and the length floor on top.
        let counts = format!(
            "WITH v AS (SELECT dense_id, x, y FROM read_parquet('{payload}')), \
                  e AS (SELECT a.dense_id AS s, b.dense_id AS d, \
                               (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) AS d2 \
                        FROM read_parquet('{by_source}') r \
                        JOIN v a ON a.dense_id = r.src_dense \
                        JOIN v b ON b.dense_id = r.dst_dense \
                        WHERE r.src_dense % {stride} = 0 OR r.dst_dense % {stride} = 0)"
        );
        let incident = scalar(&db, &format!("{counts} SELECT count(*) FROM e"));
        let induced = scalar(
            &db,
            &format!("{counts} SELECT count(*) FROM e WHERE s % {stride} = 0 AND d % {stride} = 0"),
        );
        let drawn = scalar(
            &db,
            &format!(
                "{counts} SELECT count(*) FROM e WHERE d2 >= {}",
                floor * floor
            ),
        );
        let drawn_induced = scalar(
            &db,
            &format!(
                "{counts} SELECT count(*) FROM e \
                 WHERE d2 >= {} AND s % {stride} = 0 AND d % {stride} = 0",
                floor * floor
            ),
        );
        // An anchor is a far end that is NOT a mark, over the edges that survive
        // the floor — the points a level read cannot position and therefore
        // cannot draw.
        let anchors = scalar(
            &db,
            &format!(
                "{counts} SELECT count(DISTINCT x) FROM ( \
                   SELECT s AS x FROM e WHERE d2 >= {f2} AND s % {stride} != 0 \
                   UNION SELECT d FROM e WHERE d2 >= {f2} AND d % {stride} != 0)",
                f2 = floor * floor
            ),
        );
        println!(
            "l{level}: {marks} marks · mark-incident {incident} ({induced} induced) · \
             past the floor {drawn} ({drawn_induced} induced) · anchors {anchors} · \
             a level read draws {:.2}% of the edges this view draws today",
            if drawn == 0 {
                100.0
            } else {
                (drawn_induced as f64 / drawn as f64) * 100.0
            }
        );
    }
}

/// **The edge level and the edge predicate select the same edges, at the same
/// coordinates.**
///
/// A level of a relation is the edges incident to a level-`k` vertex —
/// `src % 4^k == 0 OR dst % 4^k == 0` — and every row carries both endpoints'
/// positions so that a camera can draw the line without opening the vertex
/// payload. Two things can go wrong independently and neither throws:
///
/// - the WRONG EDGES, which an `EXCEPT ALL` in both directions against the
///   predicate over the adjacency catches;
/// - the right edges at the WRONG PLACES, which only a join back to the payload
///   catches — and which is the whole reason the file carries coordinates at
///   all, so an edge-only diff would be green for the failure this exists to
///   prevent.
#[test]
fn an_edge_level_holds_the_incident_edges_at_the_payload_s_own_coordinates() {
    let mut f = fixture(dir("levels_edge_sets"), ROWS, 14);
    f.chunk_size = CHUNK;
    enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");

    let plan = VertexLevels::planned(u64::from(ROWS), CHUNK).expect("over one tile");
    let by_source = written_relation(&f.root, "by_source");
    let payload = lit(&f.root.join("chunks").join("tiles.parquet"));
    let db = Connection::open_in_memory().expect("duckdb");

    for &level in &plan.levels {
        let stride = VertexLevels::stride(level);
        let set = lit(&f.root.join(format!("l{level}")).join("tiles.parquet"));

        // Non-vacuity first: a level file that is empty passes every diff below
        // for the wrong reason, and an empty file is exactly what a wrong
        // predicate produces.
        let held = scalar(&db, &format!("SELECT count(*) FROM read_parquet('{set}')"));
        assert!(
            held > 0,
            "level {level} holds no edges, so every assertion below is over an empty file"
        );

        let predicate = format!(
            "SELECT src_dense, dst_dense FROM read_parquet('{by_source}') \
             WHERE src_dense % {stride} = 0 OR dst_dense % {stride} = 0"
        );
        let file = format!("SELECT src_dense, dst_dense FROM read_parquet('{set}')");
        let differ = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM (({predicate} EXCEPT ALL {file}) \
                 UNION ALL ({file} EXCEPT ALL {predicate}))"
            ),
        );
        assert_eq!(
            differ, 0,
            "level {level}: the file and `src % {stride} = 0 OR dst % {stride} = 0` disagree by \
             {differ} edge(s)"
        );

        // The coordinates are the payload's own, for BOTH ends. A level naming
        // the right edges at the wrong places draws a picture that is wrong
        // about where everything is, and the diff above is green for it.
        let misplaced = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM read_parquet('{set}') e \
                   JOIN read_parquet('{payload}') s ON s.dense_id = e.src_dense \
                   JOIN read_parquet('{payload}') d ON d.dense_id = e.dst_dense \
                  WHERE e.src_x != s.x OR e.src_y != s.y OR e.dst_x != d.x OR e.dst_y != d.y"
            ),
        );
        assert_eq!(
            misplaced, 0,
            "level {level}: {misplaced} edge(s) carry coordinates the payload disagrees with"
        );

        println!("l{level}: {held} edges, both ends placed as the payload places them");
    }

    // It nests, which is what makes zooming in only ever ADD a line.
    for pair in plan.levels.windows(2) {
        let (fine, coarse) = (pair[0], pair[1]);
        let a = lit(&f.root.join(format!("l{fine}")).join("tiles.parquet"));
        let b = lit(&f.root.join(format!("l{coarse}")).join("tiles.parquet"));
        let escaped = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM ((SELECT src_dense, dst_dense FROM read_parquet('{b}')) \
                 EXCEPT ALL (SELECT src_dense, dst_dense FROM read_parquet('{a}')))"
            ),
        );
        assert_eq!(
            escaped, 0,
            "level {coarse} holds {escaped} edge(s) level {fine} does not, so the levels do not nest"
        );
    }
}
