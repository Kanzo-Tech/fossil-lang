//! **What a level costs and what a rung costs, on ONE corpus** — an instrument,
//! not a test, which is why it is `#[ignore]`d and CI never runs it.
//!
//! `levels.rs` already measures half of this: `level_cost_against_the_whole_type`
//! reports a level's bytes against the payload's, and
//! `what_the_pixel_floor_leaves_of_a_coarse_view` reports what a level read can
//! actually draw. Neither one has anything to say about `holon/r{k}/`, and the
//! two artefacts have to be weighed against each other on the SAME corpus or the
//! comparison is between two graphs.
//!
//! So this writes one corpus and measures THREE arms on it:
//!
//! - the level pyramid — `chunks/l{k}/` and the edge levels at `l{k}/`;
//! - the cell pyramid — `chunks/holon/r{k}/` and `chunks/holon/r{k}/quotient/`;
//! - **a stride**, which stores nothing: `dense_id % 2^k = 0` evaluated per
//!   query over the Morton-ordered payload. It is what `frame` falls back to
//!   where no `l{k}/` is written, and the only thing the production consumer
//!   does at all.
//!
//! — and the head-to-head at the scales where they can answer together.
//!
//! **A stride returns few rows and READS many bytes, and that is the whole
//! reason it needs measuring rather than reasoning about.** `pool` strides
//! `inrect` rather than the file, and `vis` carries `count(*)` over `inrect`, so
//! the predicate is evaluated over everything the rectangle matched. The only
//! thing that cuts it is Parquet footer pruning, and at the far view there is
//! nothing to prune. So the unit is **bytes read per mark delivered**, not rows
//! returned, and the tables are run at two rectangles and swept over eight.
//!
//! **Bytes are measured two ways and both are reported.** A camera does not read
//! a whole Parquet: it projects the columns it draws with, and Parquet's
//! per-column-chunk compressed size is what that read costs. So every file is
//! reported as its whole size AND as the size of the columns a camera opens —
//! `dense_id, x, y, cluster_id` for a payload or a vertex level, all but
//! `internal` for a rung, the two endpoints for an adjacency, the four
//! coordinates for an edge level. Whole-file bytes flatter the level pyramid,
//! because a level file is a copy of the payload and carries the subject IRI that
//! is most of it.
//!
//! **The screen is the unit `/docs/design/holons` uses**: about fifteen thousand
//! marks on a megapixel canvas. The pixel floor is `levels.rs`'s own — three
//! device pixels at 1,200, in the corpus units the written extent puts them in.
//!
//! `cargo test -p fossil-layout --test level_vs_rung -- --ignored --nocapture`

#![cfg(not(target_arch = "wasm32"))]
// Deliberate numeric code, and the same declension `layout.rs` makes for the same
// reason: this prints byte counts as kilobytes and row counts as ratios, so every
// count becomes an `f64` on its way to a table. `cast_sign_loss` joins them for
// the one trip in the other direction — a canvas divided by a mark width is a
// ratio of two positive constants and lands back as a count of marks.
// `tuple_array_conversions` is declined beside them because the pair being
// flattened IS an edge, and spelling that as an array conversion says less than
// the pattern does.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::tuple_array_conversions
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{dir, fixture, over};
use duckdb::Connection;
use fossil_layout::layout::enrich_layout;
use fossil_sinks::manifest::{HOLON_PREFIX, HolonTree, QUOTIENT_PREFIX, VertexLevels};
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The same size and the same mean degree the two `levels.rs` instruments use,
/// so the three corpora are the same graph and the numbers compose.
const BIG: u32 = 300_000;
const DEGREE: u32 = 14;
const CANVAS_PX: f64 = 1_200.0;
const MIN_LINK_PX: f64 = 3.0;
/// The marks a megapixel canvas draws comfortably — the unit `/docs/design/holons`
/// picks `DEFAULT_VERTICES_PER_CELL` with.
const SCREEN_MARKS: u64 = 15_000;

fn lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn bytes(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The compressed bytes of exactly these columns, summed over every row group —
/// what a camera's range requests actually move. `None` where the file is not
/// there.
fn projected(path: &Path, columns: &[&str]) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    let reader = SerializedFileReader::new(file).ok()?;
    let meta = reader.metadata();
    let mut total = 0i64;
    for rg in meta.row_groups() {
        for col in rg.columns() {
            if columns.contains(&col.column_path().string().as_str()) {
                total += col.compressed_size();
            }
        }
    }
    u64::try_from(total).ok()
}

fn rows_of(path: &Path) -> u64 {
    let Ok(file) = fs::File::open(path) else {
        return 0;
    };
    let Ok(reader) = SerializedFileReader::new(file) else {
        return 0;
    };
    u64::try_from(reader.metadata().file_metadata().num_rows()).unwrap_or(0)
}

fn scalar(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).expect(sql)
}

fn kb(b: u64) -> f64 {
    b as f64 / 1024.0
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    (part as f64 / whole as f64) * 100.0
}

/// The columns a camera opens, per artefact.
const VERTEX_DRAW: &[&str] = &["dense_id", "x", "y", "cluster_id"];
const EDGE_DRAW: &[&str] = &["src_x", "src_y", "dst_x", "dst_y"];
const ADJACENCY_DRAW: &[&str] = &["src_dense", "dst_dense"];
const CELL_DRAW: &[&str] = &["cell_id", "x", "y", "count", "mode", "purity"];
const QUOTIENT_DRAW: &[&str] = &["src_cell", "dst_cell", "weight"];

/// The grid `/docs/design/camera` measures fidelity on: 48 cells per axis over
/// the extent, 2,304 cells. Restated here rather than imported because it lives
/// in a `.mjs` script on the other side of the repo, and the number is the
/// measurement's — changing it changes what every figure below means.
const GRID: usize = 48;

/// The mark budgets the three arms are compared at. `SCREEN_MARKS` is one of
/// them; the others bracket it by roughly half-decades, because the crossover is
/// a number on this axis and one sample cannot find it.
const BUDGETS: &[u64] = &[1_000, 5_000, SCREEN_MARKS, 50_000];

/// A camera's rectangle, in corpus units.
#[derive(Clone, Copy)]
struct Rect {
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
}

impl Rect {
    /// The concentric box covering `f` of each axis of the written extent —
    /// `f == 1.0` being the far view, where the rectangle covers the graph and
    /// footer pruning has nothing to prune.
    fn centred(x0: f64, x1: f64, y0: f64, y1: f64, f: f64) -> Self {
        let (cx, cy) = (f64::midpoint(x0, x1), f64::midpoint(y0, y1));
        let (hw, hh) = ((x1 - x0) * f / 2.0, (y1 - y0) * f / 2.0);
        Self {
            x0: cx - hw,
            x1: cx + hw,
            y0: cy - hh,
            y1: cy + hh,
        }
    }

    /// The box predicate, over a table that has `x` and `y`.
    fn sql(self) -> String {
        format!(
            "x >= {} AND x <= {} AND y >= {} AND y <= {}",
            self.x0, self.x1, self.y0, self.y1
        )
    }
}

/// **The stride the production consumer actually builds** — `apps/playground/src/stride.ts`,
/// `strideSql`.
///
/// It is NOT `ceil(matched / limit)`. That integer is quantised UP to the next
/// power of two, and the reason is in that file: arbitrary strides do not nest,
/// so two adjacent zoom steps share almost no vertices and the camera move
/// REPLACES the picture instead of refining it. So the stride family is `2^k`,
/// one octave finer than the level family's `4^k` — not "any budget", which is
/// the thing this measurement had to check before it could compare the two.
///
/// Spelled in integers where that file spells it in doubles, and it is the same
/// function: `1 << (floor(log2(n - 0.5)) + 1)` IS `n.next_power_of_two()`, and
/// the `- 0.5` is the hack that makes the float version agree with the integer
/// one at the exact powers of two, where a double lands on either side of an
/// integer and buys a doubling nobody asked for. Checked at 1, 4, 6, 20 and 300.
fn stride_for(matched: u64, limit: u64) -> u64 {
    matched.div_ceil(limit.max(1)).max(1).next_power_of_two()
}

/// The rows of `parquet_metadata`, folded to one row per row group: the `x`/`y`
/// box the footer carries, the `dense_id`-like key's range, and the compressed
/// size of just the columns a camera opens.
///
/// **This is the index, and it is the only one.** `frame` selects tiles out of
/// the per-tile boxes in the Parquet footer and out of nothing else; the tiles
/// of this corpus are row groups of one file, so the footer's row-group
/// statistics ARE those boxes.
fn footer(path: &Path, columns: &[&str], key: &str) -> String {
    // `IN ()` is a parse error, and the empty projection is a real call: `key_ranges`
    // wants the footer's boxes and no byte count at all.
    let cols = if columns.is_empty() {
        "NULL".to_string()
    } else {
        columns
            .iter()
            .map(|c| format!("'{c}'"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "SELECT row_group_id AS rg, \
                max(CASE WHEN path_in_schema = 'x' THEN CAST(stats_min_value AS DOUBLE) END) AS xlo, \
                max(CASE WHEN path_in_schema = 'x' THEN CAST(stats_max_value AS DOUBLE) END) AS xhi, \
                max(CASE WHEN path_in_schema = 'y' THEN CAST(stats_min_value AS DOUBLE) END) AS ylo, \
                max(CASE WHEN path_in_schema = 'y' THEN CAST(stats_max_value AS DOUBLE) END) AS yhi, \
                max(CASE WHEN path_in_schema = '{key}' THEN CAST(stats_min_value AS BIGINT) END) AS klo, \
                max(CASE WHEN path_in_schema = '{key}' THEN CAST(stats_max_value AS BIGINT) END) AS khi, \
                sum(CASE WHEN path_in_schema IN ({cols}) THEN total_compressed_size ELSE 0 END) AS drawn \
           FROM parquet_metadata('{p}') GROUP BY 1",
        p = lit(path)
    )
}

/// The drawn-column bytes of the row groups whose footer box meets `r`, and how
/// many of how many that was. `(0, 0, 0)` where the file is not there.
fn drawn_in_rect(db: &Connection, path: &Path, columns: &[&str], r: Rect) -> (u64, u64, u64) {
    if !path.is_file() {
        return (0, 0, 0);
    }
    let g = footer(path, columns, "dense_id");
    let sql = format!(
        "WITH g AS ({g}) SELECT coalesce(sum(CASE WHEN {keep} THEN drawn ELSE 0 END), 0), \
                                count(*) FILTER (WHERE {keep}), count(*) FROM g",
        keep = format!(
            "xhi >= {} AND xlo <= {} AND yhi >= {} AND ylo <= {}",
            r.x0, r.x1, r.y0, r.y1
        )
    );
    let (b, kept, all): (i64, i64, i64) = db
        .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("the footer");
    (
        u64::try_from(b).unwrap_or(0),
        u64::try_from(kept).unwrap_or(0),
        u64::try_from(all).unwrap_or(0),
    )
}

/// The `[min, max]` of `key` over the row groups whose footer box meets `r` —
/// the id runs a rectangle selects, which is what addresses the adjacency and
/// the quotient, neither of which carries an `x`.
fn key_ranges(db: &Connection, path: &Path, key: &str, r: Rect) -> Vec<(i64, i64)> {
    if !path.is_file() {
        return Vec::new();
    }
    let g = footer(path, &[], key);
    let sql = format!(
        "WITH g AS ({g}) SELECT klo, khi FROM g \
          WHERE xhi >= {} AND xlo <= {} AND yhi >= {} AND ylo <= {} ORDER BY klo",
        r.x0, r.x1, r.y0, r.y1
    );
    let mut stmt = db.prepare(&sql).expect("prepare the ranges");
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
        .expect("the ranges");
    rows.filter_map(Result::ok).collect()
}

/// The drawn-column bytes of the row groups whose `key` range meets any of
/// `ranges` — a keyed file pruned by the ids a rectangle already chose.
fn drawn_on_key(
    db: &Connection,
    path: &Path,
    columns: &[&str],
    key: &str,
    ranges: &[(i64, i64)],
) -> u64 {
    if !path.is_file() || ranges.is_empty() {
        return 0;
    }
    let g = footer(path, columns, key);
    let keep = ranges
        .iter()
        .map(|&(lo, hi)| format!("(khi >= {lo} AND klo <= {hi})"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!("WITH g AS ({g}) SELECT coalesce(sum(drawn), 0) FROM g WHERE {keep}");
    u64::try_from(scalar(db, &sql)).unwrap_or(0)
}

/// A normalised density grid over the extent — `GRID`×`GRID` cells, each holding
/// its share of the ink.
///
/// `weight` is what a row contributes: `1` for real vertices, `"count"` for a
/// rung, whose rows are synthetic and each stand for that many members. That
/// second spelling is the whole of how a rung is compared honestly — its
/// centroids rasterised with their populations, against the real vertices.
fn density(db: &Connection, path: &Path, predicate: &str, weight: &str, extent: Rect) -> Vec<f64> {
    density_on(db, path, predicate, weight, extent, GRID)
}

/// The same grid at a **stated** resolution, which is the one knob
/// [`what_grid_resolution_does_to_the_fidelity_verdict_on_com_dblp`] turns.
/// [`density`] is this at [`GRID`], and that spelling is kept because every
/// caller but the sweep means the camera's 48.
fn density_on(
    db: &Connection,
    path: &Path,
    predicate: &str,
    weight: &str,
    extent: Rect,
    grid: usize,
) -> Vec<f64> {
    let mut cells = vec![0.0f64; grid * grid];
    if !path.is_file() {
        return cells;
    }
    let cw = (extent.x1 - extent.x0) / grid as f64;
    let ch = (extent.y1 - extent.y0) / grid as f64;
    let sql = format!(
        "SELECT least({last}, greatest(0, floor((x - {x0}) / {cw})))::INTEGER AS gx, \
                least({last}, greatest(0, floor((y - {y0}) / {ch})))::INTEGER AS gy, \
                CAST(sum({weight}) AS DOUBLE) AS n \
           FROM read_parquet('{p}') WHERE {predicate} GROUP BY 1, 2",
        last = grid - 1,
        x0 = extent.x0,
        y0 = extent.y0,
        p = lit(path),
    );
    let mut stmt = db.prepare(&sql).expect("prepare the grid");
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })
        .expect("the grid");
    let mut total = 0.0;
    for (gx, gy, n) in rows.filter_map(Result::ok) {
        let i = usize::try_from(gy.max(0)).unwrap_or(0) * grid
            + usize::try_from(gx.max(0)).unwrap_or(0);
        if let Some(cell) = cells.get_mut(i) {
            *cell += n;
            total += n;
        }
    }
    if total > 0.0 {
        for cell in &mut cells {
            *cell /= total;
        }
    }
    cells
}

/// Total variation distance — the largest fraction of the ink that can land in
/// the wrong cell. `/docs/design/camera`'s statistic, and TV rather than a χ²
/// for its reason: the question is about a picture, TV is bounded in `[0, 1]`
/// and it does not grow with the grid.
fn tv(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(p, q)| (p - q).abs()).sum::<f64>() / 2.0
}

/// **The mark budget at which a stride's bytes-per-mark comes down to an
/// artefact's.**
///
/// A stride's bytes do not move with the budget, so its bytes-per-mark is
/// `stride / budget` — a hyperbola. An artefact's is a constant, `bytes /
/// marks`, because it can only be read at the scales it was written at. They
/// meet at `budget = stride · marks / bytes`, and BELOW that budget the artefact
/// is cheaper per mark. That is the crossover, and it is one division rather
/// than an interpolation because one of the two curves is flat.
///
/// `None` where the budget is past `matched`: a stride cannot deliver more marks
/// than the rectangle holds, so the artefact is cheaper at every budget a camera
/// can ask for.
fn crossover(stride_bytes: u64, arm: &Arm, matched: u64) -> Option<f64> {
    if arm.bytes == 0 || arm.marks == 0 {
        return None;
    }
    let budget = stride_bytes as f64 * arm.marks as f64 / arm.bytes as f64;
    (budget <= matched as f64).then_some(budget)
}

/// Where com-DBLP lives, relative to the repository root. Both directions of
/// each of its 1,049,866 undirected edges, which is how the playground's bench
/// checked it in.
const DBLP_CSV: &str = "../../apps/playground/bench/dblp/data/links.csv";

struct Corpus {
    root: PathBuf,
    chunk: u64,
    vertex_count: u64,
    tree: HolonTree,
}

fn write_corpus(name: &str) -> Corpus {
    let f = fixture(dir(name), BIG, DEGREE);
    let chunk = f.chunk_size;
    let report = enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");
    let tree = report
        .pyramids
        .into_iter()
        .find(|(ty, _)| ty == "Node")
        .expect("the one vertex type earned a pyramid")
        .1;
    Corpus {
        root: f.root,
        chunk,
        vertex_count: u64::from(BIG),
        tree,
    }
}

/// com-DBLP as the pass takes it: the CSV's ids **densified** — they are the
/// original sparse com-DBLP ids and run to 425,956 over 317,080 vertices, so a
/// pass fed them raw would lay out a corpus of empty rows — one wide vertex
/// batch with an IRI the same width the planted fixture uses, and the two
/// orientations of the one self-relation.
fn dblp_corpus(name: &str) -> Option<Corpus> {
    let csv = Path::new(env!("CARGO_MANIFEST_DIR")).join(DBLP_CSV);
    let text = fs::read_to_string(&csv).ok()?;

    let mut raw: Vec<(u32, u32)> = Vec::with_capacity(2_100_000);
    for line in text.lines().skip(1) {
        let Some((a, b)) = line.split_once(',') else {
            continue;
        };
        let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) else {
            continue;
        };
        raw.push((a, b));
    }
    // Densify: the distinct ids, sorted, become 0..V.
    let mut ids: Vec<u32> = raw.iter().flat_map(|&(a, b)| [a, b]).collect();
    ids.sort_unstable();
    ids.dedup();
    let rows = u32::try_from(ids.len()).ok()?;
    let dense = |id: u32| u32::try_from(ids.binary_search(&id).ok().unwrap_or(0)).unwrap_or(0);
    let edges: Vec<(u32, u32)> = raw.into_iter().map(|(a, b)| (dense(a), dense(b))).collect();

    let f = over(dir(name), rows, &edges);
    let chunk = f.chunk_size;
    let report = enrich_layout(&f.targets(), &f.adjacencies()).expect("the layout pass");
    let tree = report
        .pyramids
        .into_iter()
        .find(|(ty, _)| ty == "Node")
        .expect("com-DBLP earned a pyramid")
        .1;
    Some(Corpus {
        root: f.root,
        chunk,
        vertex_count: u64::from(rows),
        tree,
    })
}

#[test]
#[ignore = "an instrument: it writes a 300,000-vertex corpus and measures both pyramids on it"]
fn what_a_level_costs_and_what_a_rung_costs_on_a_planted_corpus() {
    let c = write_corpus("level_vs_rung");
    report(&c, u64::from(BIG), "planted partition, mean degree 14");
}

/// The same measurement over **com-DBLP** — 317,080 real vertices and the
/// 2,099,732 directed rows of `apps/playground/bench/dblp/data/links.csv`, which
/// is the graph `/docs/design/holons` already quotes.
///
/// A second graph of a different shape is exactly what
/// `DEFAULT_VERTICES_PER_CELL`'s doc says would settle the base, and it is what
/// keeps the planted fixture's numbers from being a property of the generator.
/// Skipped with a printed line, not a failure, where the CSV is not checked out.
///
/// `cargo test -p fossil-layout --test level_vs_rung -- --ignored --nocapture`
#[test]
#[ignore = "an instrument: it reads com-DBLP off disk and measures both pyramids on it"]
fn what_a_level_costs_and_what_a_rung_costs_on_com_dblp() {
    let Some(c) = dblp_corpus("level_vs_rung_dblp") else {
        println!("com-DBLP is not checked out at {DBLP_CSV}; skipping");
        return;
    };
    let v = c.vertex_count;
    report(&c, v, "com-DBLP, both directions");
}

#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
fn report(c: &Corpus, vertex_count: u64, shape: &str) {
    let big = vertex_count;
    let chunks = c.root.join("chunks");
    let payload = chunks.join("tiles.parquet");
    let by_source = c.root.join("by_source").join("tiles.parquet");

    let payload_bytes = bytes(&payload);
    let payload_draw = projected(&payload, VERTEX_DRAW).expect("the payload");
    let adjacency_bytes = bytes(&by_source);
    let adjacency_draw = projected(&by_source, ADJACENCY_DRAW).expect("the adjacency");
    let edges = rows_of(&by_source);

    println!("\n=== CORPUS ===");
    println!(
        "{big} vertices, {edges} edges ({shape}), \
         chunk_size {}, vertices_per_cell {}",
        c.chunk, c.tree.vertices_per_cell
    );
    println!(
        "payload          {:>9.0} kB whole · {:>9.0} kB drawn-columns · {} tiles",
        kb(payload_bytes),
        kb(payload_draw),
        big.div_ceil(c.chunk)
    );
    println!(
        "by_source        {:>9.0} kB whole · {:>9.0} kB drawn-columns · {edges} rows",
        kb(adjacency_bytes),
        kb(adjacency_draw),
    );

    // ---- the whole-extent floor, from the WRITTEN positions ----
    let db = Connection::open_in_memory().expect("duckdb");
    let pay = lit(&payload);
    let (min_x, max_x, min_y, max_y): (f64, f64, f64, f64) = db
        .query_row(
            &format!("SELECT min(x), max(x), min(y), max(y) FROM read_parquet('{pay}')"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the extent");
    let extent = Rect {
        x0: min_x,
        x1: max_x,
        y0: min_y,
        y1: max_y,
    };
    let floor = MIN_LINK_PX * (max_x - min_x) / CANVAS_PX;
    let f2 = floor * floor;
    println!(
        "extent {min_x:.1}..{max_x:.1}, floor {floor:.3} corpus units \
         ({MIN_LINK_PX} px at {CANVAS_PX} px)"
    );

    // ============================ 1. LEVELS ============================
    let plan = VertexLevels::planned(big, c.chunk).expect("over one tile");
    println!("\n=== 1. WHAT A LEVEL COSTS ===");
    println!(
        "{:<4} {:>9} {:>7} {:>10} {:>7} {:>10} {:>11} {:>10} {:>10} {:>9}",
        "k",
        "marks",
        "tiles",
        "vtx kB",
        "% pay",
        "marks kB",
        "edge rows",
        "edge kB",
        "view kB",
        "vs stride"
    );
    let stride_view = payload_draw + adjacency_draw;
    let mut level_view = Vec::new();
    for &k in &plan.levels {
        let vfile = chunks.join(plan.level_prefix(k)).join("tiles.parquet");
        let efile = c.root.join(format!("l{k}")).join("tiles.parquet");
        let vb = bytes(&vfile);
        let vd = projected(&vfile, VERTEX_DRAW).unwrap_or(0);
        let eb = bytes(&efile);
        let ed = projected(&efile, EDGE_DRAW).unwrap_or(0);
        let erows = rows_of(&efile);
        let marks = VertexLevels::rows_at(big, k);
        let view = vd + ed;
        level_view.push((k, marks, view, vd, eb));
        println!(
            "l{k:<3} {marks:>9} {:>7} {:>10.0} {:>6.1}% {:>10.0} {erows:>11} {:>10.0} {:>10.0} {:>8.2}x",
            marks.div_ceil(c.chunk),
            kb(vb),
            pct(vb, payload_bytes),
            kb(vd),
            kb(eb),
            kb(view),
            stride_view as f64 / view as f64,
        );
    }
    println!(
        "  (view kB = drawn columns of a level's vertex file plus its edge file; \
         striding the payload for the same picture reads {:.0} kB)",
        kb(stride_view)
    );
    let level_total: u64 = plan
        .levels
        .iter()
        .map(|&k| {
            bytes(&chunks.join(plan.level_prefix(k)).join("tiles.parquet"))
                + bytes(&c.root.join(format!("l{k}")).join("tiles.parquet"))
        })
        .sum();
    println!(
        "  whole level pyramid (vertex + edge): {:.0} kB = {:.1}% of payload+adjacency",
        kb(level_total),
        pct(level_total, payload_bytes + adjacency_bytes)
    );

    // ============================ 2. RUNGS ============================
    let holon = chunks.join(HOLON_PREFIX.trim_end_matches('/'));
    println!("\n=== 2. WHAT A RUNG COSTS ===");
    println!(
        "{:<4} {:>9} {:>7} {:>10} {:>7} {:>10} {:>11} {:>10} {:>8} {:>10} {:>9}",
        "k",
        "marks",
        "tiles",
        "cell kB",
        "% pay",
        "marks kB",
        "quot rows",
        "quot kB",
        "density",
        "view kB",
        "vs stride"
    );
    let mut rung_view = Vec::new();
    for (index, rung) in c.tree.rungs.iter().enumerate() {
        let k = index + 1;
        let rdir = holon.join(rung.path.trim_end_matches('/'));
        let cfile = rdir.join("tiles.parquet");
        let qfile = rdir
            .join(QUOTIENT_PREFIX.trim_end_matches('/'))
            .join("tiles.parquet");
        let cb = bytes(&cfile);
        let cd = projected(&cfile, CELL_DRAW).unwrap_or(0);
        let qb = bytes(&qfile);
        let qd = projected(&qfile, QUOTIENT_DRAW).unwrap_or(0);
        let qrows = rung.quotient.as_ref().map_or(0, |q| q.edge_count);
        let view = cd + qd;
        rung_view.push((k as u32, rung.holon_count, view, cd, qb, qrows));
        println!(
            "r{k:<3} {:>9} {:>7} {:>10.1} {:>6.2}% {:>10.1} {qrows:>11} {:>10.1} {:>7.2}% {:>10.1} {:>8.1}x",
            rung.holon_count,
            rung.holon_count.div_ceil(c.chunk),
            kb(cb),
            pct(cb, payload_bytes),
            kb(cd),
            kb(qb),
            // How full the quotient is against the complete graph on this
            // rung's holons. A rung whose quotient IS the complete graph
            // carries no structure left to draw, whatever its byte count says.
            {
                let n = rung.holon_count;
                let possible = n.saturating_mul(n.saturating_sub(1)) / 2;
                pct(qrows, possible.max(1))
            },
            kb(view),
            stride_view as f64 / view.max(1) as f64,
        );
    }
    let rung_total: u64 = c
        .tree
        .rungs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let rdir = holon.join(r.path.trim_end_matches('/'));
            let _ = i;
            bytes(&rdir.join("tiles.parquet"))
                + bytes(
                    &rdir
                        .join(QUOTIENT_PREFIX.trim_end_matches('/'))
                        .join("tiles.parquet"),
                )
        })
        .sum();
    println!(
        "  whole cell pyramid (cells + quotients): {:.0} kB = {:.1}% of payload+adjacency",
        kb(rung_total),
        pct(rung_total, payload_bytes + adjacency_bytes)
    );

    // ========================= 3. HEAD TO HEAD =========================
    // A rung's holon count and a level's mark count are the same arithmetic one
    // octave apart: a base of 16 makes rung k = V/(16·4^(k-1)) = V/4^(k+1),
    // which is level k+1. So they are paired by MARKS and not by index.
    println!("\n=== 3. HEAD TO HEAD, at the scales where both answer ===");
    println!("  MARKS ONLY — a camera that draws points and no lines");
    println!(
        "{:>9} {:>6} {:>10} {:>6} {:>10} {:>10} {:>9}",
        "marks", "level", "level kB", "rung", "rung kB", "cheaper", "factor"
    );
    for &(lk, marks, _, lmarks, _) in &level_view {
        let Some(&(rk, _, _, rmarks, _, _)) =
            rung_view.iter().find(|(_, hc, _, _, _, _)| *hc == marks)
        else {
            continue;
        };
        let (winner, factor) = if rmarks < lmarks {
            ("rung", lmarks as f64 / rmarks.max(1) as f64)
        } else {
            ("level", rmarks as f64 / lmarks.max(1) as f64)
        };
        println!(
            "{marks:>9} {:>6} {:>10.0} {:>6} {:>10.1} {winner:>10} {factor:>8.2}x",
            format!("l{lk}"),
            kb(lmarks),
            format!("r{rk}"),
            kb(rmarks),
        );
    }
    println!("\n  MARKS AND LINES — the whole coarse view");
    println!(
        "{:>9} {:>6} {:>10} {:>6} {:>10} {:>10} {:>9}",
        "marks", "level", "level kB", "rung", "rung kB", "cheaper", "factor"
    );
    for &(lk, marks, lview, _, _) in &level_view {
        let Some(&(rk, _, rview, _, _, _)) =
            rung_view.iter().find(|(_, hc, _, _, _, _)| *hc == marks)
        else {
            println!(
                "{marks:>9} {:>6} {:>10.0} {:>6} {:>10} {:>10} {:>9}",
                format!("l{lk}"),
                kb(lview),
                "-",
                "-",
                "level only",
                "-"
            );
            continue;
        };
        let (winner, factor) = if rview < lview {
            ("rung", lview as f64 / rview.max(1) as f64)
        } else {
            ("level", rview as f64 / lview.max(1) as f64)
        };
        println!(
            "{marks:>9} {:>6} {:>10.0} {:>6} {:>10.1} {winner:>10} {factor:>8.2}x",
            format!("l{lk}"),
            kb(lview),
            format!("r{rk}"),
            kb(rview),
        );
    }
    // And the scales only one of them reaches.
    for &(rk, hc, rview, _, _, _) in &rung_view {
        if !level_view.iter().any(|&(_, marks, _, _, _)| marks == hc) {
            println!(
                "{hc:>9} {:>6} {:>10} {:>6} {:>10.1} {:>10} {:>9}",
                "-",
                "-",
                format!("r{rk}"),
                kb(rview),
                "rung only",
                "-"
            );
        }
    }
    println!(
        "\n  the {SCREEN_MARKS}-mark screen: nearest level is l{}, nearest rung is r{}",
        level_view
            .iter()
            .min_by_key(|&&(_, m, _, _, _)| m.abs_diff(SCREEN_MARKS))
            .map_or(0, |&(k, _, _, _, _)| k),
        rung_view
            .iter()
            .min_by_key(|&&(_, h, _, _, _, _)| h.abs_diff(SCREEN_MARKS))
            .map_or(0, |&(k, _, _, _, _, _)| k),
    );

    // ================== 4. WHERE THEY STOP BEING COMPARABLE ==================
    println!("\n=== 4. WHERE THEY STOP ANSWERING THE SAME QUESTION ===");
    println!(
        "{:<6} {:>9} {:>9} {:>12} {:>9} {:>12} {:>9} {:>11} {:>10}",
        "artef",
        "marks",
        "covers V",
        "lines: edge",
        "of all",
        "vtx-only",
        "of all",
        "B per line",
        "identity"
    );
    let bysrc = lit(&by_source);
    // What a whole-extent view draws TODAY, off the payload: every edge past the
    // floor. The denominator both sides are measured against.
    let today = scalar(
        &db,
        &format!(
            "WITH v AS (SELECT dense_id, x, y FROM read_parquet('{pay}')) \
             SELECT count(*) FROM read_parquet('{bysrc}') r \
               JOIN v a ON a.dense_id = r.src_dense \
               JOIN v b ON b.dense_id = r.dst_dense \
              WHERE (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) >= {f2}"
        ),
    );
    println!("  a whole-extent view off the payload draws {today} lines past the floor");
    for &(k, marks, lview, _, _) in &level_view {
        let stride = VertexLevels::stride(k);
        // A level read holds the level's rows and nothing else, so the lines it
        // can draw are the INDUCED ones past the floor.
        let drawn = scalar(
            &db,
            &format!(
                "WITH v AS (SELECT dense_id, x, y FROM read_parquet('{pay}')) \
                 SELECT count(*) FROM read_parquet('{bysrc}') r \
                   JOIN v a ON a.dense_id = r.src_dense \
                   JOIN v b ON b.dense_id = r.dst_dense \
                  WHERE r.src_dense % {stride} = 0 AND r.dst_dense % {stride} = 0 \
                    AND (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) >= {f2}"
            ),
        );
        // `what_the_pixel_floor_leaves_of_a_coarse_view`'s own denominator: the
        // MARK-INCIDENT edges past the floor, which is what a coarse view at
        // this stride draws when it is drawn off the payload.
        let incident = scalar(
            &db,
            &format!(
                "WITH v AS (SELECT dense_id, x, y FROM read_parquet('{pay}')) \
                 SELECT count(*) FROM read_parquet('{bysrc}') r \
                   JOIN v a ON a.dense_id = r.src_dense \
                   JOIN v b ON b.dense_id = r.dst_dense \
                  WHERE (r.src_dense % {stride} = 0 OR r.dst_dense % {stride} = 0) \
                    AND (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) >= {f2}"
            ),
        );
        // **What the EDGE level lets a camera draw, which is the corpus as it
        // now stands.** `l{k}/tiles.parquet` carries both endpoints'
        // coordinates — commit 0003d13, "a level of a relation, so a coarse
        // view draws lines without the payload" — so a level read is
        // SELF-DRAWING: every mark-incident edge past the floor comes out of
        // that one file, anchors and all, with no vertex tile opened.
        //
        // `what_the_pixel_floor_leaves_of_a_coarse_view`'s header predates that
        // file and says the opposite ("the far end of a mark-incident edge is
        // not in it ... those edges and their anchors go"), so `drawn` below is
        // the number for a corpus with vertex levels and no edge levels. Both
        // are reported; the first is the one a camera gets today.
        let selfdrawn = scalar(
            &db,
            &format!(
                "SELECT count(*) FROM read_parquet('{elevel}') \
                  WHERE (src_x - dst_x) * (src_x - dst_x) \
                      + (src_y - dst_y) * (src_y - dst_y) >= {f2}",
                elevel = lit(&c.root.join(format!("l{k}")).join("tiles.parquet"))
            ),
        );
        let _ = incident;
        println!(
            "l{k:<5} {marks:>9} {:>8.2}% {selfdrawn:>12} {:>8.2}% {drawn:>12} {:>8.2}% {:>11.1} {:>10}",
            pct(marks, big),
            (selfdrawn as f64 / today as f64) * 100.0,
            (drawn as f64 / today as f64) * 100.0,
            lview as f64 / selfdrawn.max(1) as f64,
            "real IRI"
        );
    }
    for (index, rung) in c.tree.rungs.iter().enumerate() {
        let k = index + 1;
        let rdir = holon.join(rung.path.trim_end_matches('/'));
        let cfile = lit(&rdir.join("tiles.parquet"));
        let qfile = rdir
            .join(QUOTIENT_PREFIX.trim_end_matches('/'))
            .join("tiles.parquet");
        // Coverage: the member counts sum to the whole type, at every rung.
        let covered = scalar(
            &db,
            &format!("SELECT coalesce(sum(count), 0) FROM read_parquet('{cfile}')"),
        );
        // The lines a rung read draws: its quotient edges past the same floor,
        // at the holon positions the rung itself carries. Both ends are in the
        // rung file by construction, so there is no anchor to miss.
        let drawn = if qfile.is_file() {
            let q = lit(&qfile);
            scalar(
                &db,
                &format!(
                    "WITH h AS (SELECT cell_id, x, y FROM read_parquet('{cfile}')) \
                     SELECT count(*) FROM read_parquet('{q}') e \
                       JOIN h a ON a.cell_id = e.src_cell \
                       JOIN h b ON b.cell_id = e.dst_cell \
                      WHERE (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) >= {f2}"
                ),
            )
        } else {
            0
        };
        let rview = rung_view
            .iter()
            .find(|(rk, _, _, _, _, _)| *rk == k as u32)
            .map_or(0, |&(_, _, v, _, _, _)| v);
        println!(
            "r{k:<5} {:>9} {:>8.2}% {drawn:>12} {:>8.2}% {:>12} {:>9} {:>11.1} {:>10}",
            rung.holon_count,
            pct(u64::try_from(covered).unwrap_or(0), big),
            (drawn as f64 / today as f64) * 100.0,
            "same",
            "-",
            rview as f64 / drawn.max(1) as f64,
            "synthetic"
        );
    }
    println!(
        "\n  deepest level: l{} at {} marks (the one-tile floor, chunk_size {});\n  \
         deepest rung:  r{} at {} holons",
        plan.levels.last().copied().unwrap_or(0),
        level_view.last().map_or(0, |&(_, m, _, _, _)| m),
        c.chunk,
        c.tree.rungs.len(),
        c.tree.rungs.last().map_or(0, |r| r.holon_count),
    );

    // ================== 5. THE THIRD ARM, AT TWO RECTANGLES ==================
    for f in [1.0, 0.1] {
        stride_arm(&db, c, big, &payload, &by_source, &plan, extent, f);
    }

    // ========================== 6. FIDELITY ==========================
    fidelity(&db, c, big, &payload, &plan, extent);

    // ========================== 7. THE CROSSOVER ==========================
    crossover_report(&db, c, &payload, &by_source, &plan, extent);
}

/// One arm at one rectangle: what it is called, what it DELIVERS, what it
/// TOUCHES. Marks and bytes are the two halves of the headline and nothing else
/// in this file needs to be carried beside them.
struct Arm {
    name: String,
    marks: u64,
    bytes: u64,
}

impl Arm {
    fn per_mark(&self) -> f64 {
        if self.marks == 0 {
            f64::INFINITY
        } else {
            self.bytes as f64 / self.marks as f64
        }
    }
}

/// **Every scale of the level pyramid at one rectangle, `l0` included.**
///
/// `l0` is the payload read — and it is BYTE-FOR-BYTE the stride's read, because
/// a stride does not change which files are opened. Listing it is how the table
/// says that the level arm degenerates INTO the stride arm as the rectangle
/// tightens, rather than losing to something else.
///
/// The vertex file is pruned on its own `x`/`y` footers. The edge file has no
/// `x`, so it is pruned on the `src_dense` runs the vertex file's kept row
/// groups hold — which is `frame`'s own rule, which derives the edge tiles from
/// the vertex tiles arithmetically out of one plan.
fn level_arms(
    db: &Connection,
    c: &Corpus,
    payload: &Path,
    by_source: &Path,
    plan: &VertexLevels,
    r: Rect,
) -> Vec<Arm> {
    let chunks = c.root.join("chunks");
    let mut out = Vec::new();
    for k in std::iter::once(0).chain(plan.levels.iter().copied()) {
        let (vfile, efile) = if k == 0 {
            (payload.to_path_buf(), by_source.to_path_buf())
        } else {
            (
                chunks.join(plan.level_prefix(k)).join("tiles.parquet"),
                c.root.join(format!("l{k}")).join("tiles.parquet"),
            )
        };
        let (vb, _, _) = drawn_in_rect(db, &vfile, VERTEX_DRAW, r);
        let ids = key_ranges(db, &vfile, "dense_id", r);
        let eb = if k == 0 {
            drawn_on_key(db, &efile, ADJACENCY_DRAW, "src_dense", &ids)
        } else {
            drawn_on_key(db, &efile, EDGE_DRAW, "src_dense", &ids)
        };
        let marks = u64::try_from(scalar(
            db,
            &format!(
                "SELECT count(*) FROM read_parquet('{}') WHERE {}",
                lit(&vfile),
                r.sql()
            ),
        ))
        .unwrap_or(0);
        out.push(Arm {
            name: format!("l{k}"),
            marks,
            bytes: vb + eb,
        });
    }
    out
}

/// Every rung at one rectangle: its cells by their own `x`/`y` footers, its
/// quotient by the `cell_id` runs those footers chose.
fn rung_arms(db: &Connection, c: &Corpus, r: Rect) -> Vec<Arm> {
    let holon = c
        .root
        .join("chunks")
        .join(HOLON_PREFIX.trim_end_matches('/'));
    c.tree
        .rungs
        .iter()
        .enumerate()
        .map(|(index, rung)| {
            let rdir = holon.join(rung.path.trim_end_matches('/'));
            let cfile = rdir.join("tiles.parquet");
            let qfile = rdir
                .join(QUOTIENT_PREFIX.trim_end_matches('/'))
                .join("tiles.parquet");
            let (cb, _, _) = drawn_in_rect(db, &cfile, CELL_DRAW, r);
            let ids = key_ranges(db, &cfile, "cell_id", r);
            let qb = drawn_on_key(db, &qfile, QUOTIENT_DRAW, "src_cell", &ids);
            let marks = u64::try_from(scalar(
                db,
                &format!(
                    "SELECT count(*) FROM read_parquet('{}') WHERE {}",
                    lit(&cfile),
                    r.sql()
                ),
            ))
            .unwrap_or(0);
            Arm {
                name: format!("r{}", index + 1),
                marks,
                bytes: cb + qb,
            }
        })
        .collect()
}

/// One row of the three tables [`stride_arm`] prints: a budget, the stride it
/// quantises to, and the index of the level and the rung a camera would read at
/// it. Three tables out of ONE gathering, so they cannot disagree.
struct Row {
    budget: u64,
    stride: u64,
    /// Marks the stride delivers, and the bytes it touched to deliver them.
    sm: u64,
    sb: u64,
    /// Into `levels` and into `rungs`.
    l: usize,
    g: usize,
}

/// **The third arm: a stride, at one rectangle.**
///
/// A stride is what `frame` falls back to when no `l{k}/` is written, and — as of
/// the measurement this was added for — the only thing the production consumer
/// (`@kanzo-tech/graph`'s `duck-source.ts`) ever does. It stores nothing. The
/// question it has to answer is therefore not what it stores but what it READS,
/// and the trap is that a stride returns few rows and touches many:
///
/// - `pool` applies `dense_id % stride = 0` to `inrect`, not to the file, so the
///   predicate is evaluated over every row the rectangle holds;
/// - `vis` carries `(SELECT count(*) FROM inrect) AS matched`, which is the
///   number the NEXT stride is computed from, so `inrect` has to be counted
///   whether or not the predicate could have pruned it.
///
/// **So the stride's byte count does not move with the budget, and that is the
/// whole of its shape.** The only thing that cuts it is footer pruning — the row
/// groups whose `x`/`y` box misses the rectangle — which is why this is run at
/// two rectangles and then swept over a range of them in [`where_the_stride_wins`].
///
/// A level and a rung are read at the scale whose IN-RECTANGLE mark count is
/// nearest the budget, not at the scale whose global row count is: a rung's
/// cells are laid out over the whole graph, so a rectangle over a hundredth of
/// the extent meets a hundredth of them, and picking by the global count asks a
/// coarse artefact a question it cannot answer and then reports the loss as a
/// cost.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn stride_arm(
    db: &Connection,
    c: &Corpus,
    big: u64,
    payload: &Path,
    by_source: &Path,
    plan: &VertexLevels,
    extent: Rect,
    fraction: f64,
) {
    let rect = Rect::centred(extent.x0, extent.x1, extent.y0, extent.y1, fraction);
    let pay = lit(payload);
    let matched = u64::try_from(scalar(
        db,
        &format!(
            "SELECT count(*) FROM read_parquet('{pay}') WHERE {}",
            rect.sql()
        ),
    ))
    .unwrap_or(0);

    let levels = level_arms(db, c, payload, by_source, plan, rect);
    let rungs = rung_arms(db, c, rect);
    // The stride reads what `l0` reads — the same files, pruned by the same
    // footers — because striding changes which rows survive and not which bytes
    // are opened.
    let stride_bytes = levels.first().map_or(0, |a| a.bytes);
    let (_, kept, of) = drawn_in_rect(db, payload, VERTEX_DRAW, rect);

    println!(
        "\n=== 5. A STRIDE AS A THIRD ARM — {} ===",
        if fraction >= 1.0 {
            "THE FAR VIEW, the rectangle covers the extent".to_string()
        } else {
            format!("a rectangle over {:.0}% of each axis", fraction * 100.0)
        }
    );
    println!(
        "  the rectangle holds {matched} of {big} vertices ({:.1}%) and selects {kept} of {of} \
         payload row groups; a stride over it touches {:.0} kB — the payload's drawn columns plus \
         the adjacency the ids it kept address — whatever the budget",
        pct(matched, big),
        kb(stride_bytes),
    );

    let nearest = |arms: &[Arm], budget: u64| {
        arms.iter()
            .enumerate()
            .min_by_key(|(_, a)| a.marks.abs_diff(budget))
            .map_or(0, |(i, _)| i)
    };
    let rows: Vec<Row> = BUDGETS
        .iter()
        .map(|&budget| {
            let stride = stride_for(matched, budget);
            let sm = u64::try_from(scalar(
                db,
                &format!(
                    "SELECT count(*) FROM read_parquet('{pay}') \
                      WHERE {} AND dense_id % {stride} = 0",
                    rect.sql()
                ),
            ))
            .unwrap_or(0);
            Row {
                budget,
                stride,
                sm,
                sb: stride_bytes,
                l: nearest(&levels, budget),
                g: nearest(&rungs, budget),
            }
        })
        .collect();

    println!("\n  1. BYTES READ, in the camera's projected columns (kB)");
    println!(
        "{:>9} {:>10} {:>12} {:>7} {:>11} {:>7} {:>11}",
        "budget", "stride", "stride kB", "level", "level kB", "rung", "rung kB"
    );
    for row in &rows {
        println!(
            "{:>9} {:>10} {:>12.1} {:>7} {:>11.1} {:>7} {:>11.1}",
            row.budget,
            format!("1 in {}", row.stride),
            kb(row.sb),
            levels[row.l].name,
            kb(levels[row.l].bytes),
            rungs[row.g].name,
            kb(rungs[row.g].bytes),
        );
    }

    println!("\n  2. MARKS DELIVERED");
    println!(
        "{:>9} {:>12} {:>7} {:>12} {:>7} {:>12}",
        "budget", "stride", "level", "level", "rung", "rung"
    );
    for row in &rows {
        println!(
            "{:>9} {:>12} {:>7} {:>12} {:>7} {:>12}",
            row.budget,
            row.sm,
            levels[row.l].name,
            levels[row.l].marks,
            rungs[row.g].name,
            rungs[row.g].marks,
        );
    }

    println!("\n  3. BYTES PER MARK — the headline");
    println!(
        "{:>9} {:>12} {:>12} {:>12} {:>16}",
        "budget", "stride B/mk", "level B/mk", "rung B/mk", "cheapest"
    );
    for row in &rows {
        let strided = if row.sm == 0 {
            f64::INFINITY
        } else {
            row.sb as f64 / row.sm as f64
        };
        let (level, rung) = (levels[row.l].per_mark(), rungs[row.g].per_mark());
        let best = if rung <= strided && rung <= level {
            format!("{} {:.1}x", rungs[row.g].name, strided / rung)
        } else if level <= strided {
            // `l0` IS the stride, so a tie is not a level winning: it is the two
            // arms being the same read, and the table says so rather than
            // awarding it.
            if levels[row.l].name == "l0" {
                "stride (= l0)".to_string()
            } else {
                format!("{} {:.1}x", levels[row.l].name, strided / level)
            }
        } else {
            "stride".to_string()
        };
        println!(
            "{:>9} {strided:>12.1} {level:>12.1} {rung:>12.1} {best:>16}",
            row.budget
        );
    }

    println!("\n  every scale of both artefacts at this rectangle, for the crossover below");
    println!(
        "{:<7} {:>10} {:>11} {:>12}",
        "arm", "marks", "kB", "B per mark"
    );
    for arm in levels.iter().chain(&rungs) {
        println!(
            "{:<7} {:>10} {:>11.1} {:>12.1}",
            arm.name,
            arm.marks,
            kb(arm.bytes),
            arm.per_mark()
        );
    }
}

/// **Fidelity, on the measure this repo already has.**
///
/// `/docs/design/camera` and `apps/playground/scripts/verify-properties.mjs`
/// state it: total variation distance between two normalised density grids,
/// 48×48 over the extent, and a tolerance that is MEASURED rather than picked —
/// the same population drawn with no spatial structure at all
/// (`hash(dense_id) % s = 0`), which is what sampling noise alone costs at that
/// size. An arm passes when it is no further from the whole graph than that null
/// is. It is the figure `design/discarded` quotes as 0.0644 against 0.1326.
///
/// The three arms are rasterised as the three things they are:
///
/// - a **level**: the rows of `l{k}/tiles.parquet`, one point each;
/// - a **stride**: `dense_id % 2^k = 0` over the payload, one point each — and
///   where `2^k` is `4^j` that is the SAME SET as level `j`, which is the first
///   thing the table says;
/// - a **rung**: the cell centroids of `holon/r{k}/tiles.parquet`, each weighted
///   by its `count`. That is the honest comparison: a rung row is synthetic and
///   stands for that many members, so rasterising it unweighted would measure a
///   different quantity from the other two.
#[allow(clippy::too_many_lines)]
fn fidelity(
    db: &Connection,
    c: &Corpus,
    big: u64,
    payload: &Path,
    plan: &VertexLevels,
    extent: Rect,
) {
    let chunks = c.root.join("chunks");
    let holon = chunks.join(HOLON_PREFIX.trim_end_matches('/'));
    let base = density(db, payload, "TRUE", "1", extent);

    println!("\n=== 6. FIDELITY — {GRID}x{GRID} total variation against the payload ===");
    println!(
        "  the measure is `/docs/design/camera`'s: TV between normalised density grids over the \
         written extent, and the tolerance is the null — the same count drawn by \
         `hash(dense_id) % s = 0`, which has no spatial structure"
    );
    println!(
        "{:<10} {:>6} {:>10} {:>9} {:>9} {:>8}",
        "arm", "stride", "marks", "TV", "null TV", "verdict"
    );

    // The stride family, `2^k`, which contains the level family as its even
    // members. Walked as far as the deepest level's stride so the two tables
    // cover the same range of marks.
    let deepest = plan.levels.last().copied().unwrap_or(0);
    for k in 1..=(deepest * 2) {
        let stride = 1u64 << k;
        let marks = u64::try_from(scalar(
            db,
            &format!(
                "SELECT count(*) FROM read_parquet('{}') WHERE dense_id % {stride} = 0",
                lit(payload)
            ),
        ))
        .unwrap_or(0);
        let arm = density(
            db,
            payload,
            &format!("dense_id % {stride} = 0"),
            "1",
            extent,
        );
        let null = density(
            db,
            payload,
            &format!("hash(dense_id) % {stride} = 0"),
            "1",
            extent,
        );
        let (dist, noise) = (tv(&base, &arm), tv(&base, &null));
        println!(
            "{:<10} {stride:>6} {marks:>10} {dist:>9.4} {noise:>9.4} {:>8}",
            if k % 2 == 0 {
                format!("stride=l{}", k / 2)
            } else {
                "stride".to_string()
            },
            if dist <= noise { "pass" } else { "FAIL" },
        );
    }

    // The levels, read off their own files rather than off the predicate — a
    // written level that disagreed with the predicate would show up here as a
    // different TV from the `stride=l{k}` row above it.
    for &k in &plan.levels {
        let lfile = chunks.join(plan.level_prefix(k)).join("tiles.parquet");
        let stride = VertexLevels::stride(k);
        let arm = density(db, &lfile, "TRUE", "1", extent);
        let null = density(
            db,
            payload,
            &format!("hash(dense_id) % {stride} = 0"),
            "1",
            extent,
        );
        let (dist, noise) = (tv(&base, &arm), tv(&base, &null));
        println!(
            "{:<10} {stride:>6} {:>10} {dist:>9.4} {noise:>9.4} {:>8}",
            format!("level l{k}"),
            rows_of(&lfile),
            if dist <= noise { "pass" } else { "FAIL" },
        );
    }

    // The rungs: centroids weighted by `count`, against the real vertices. The
    // null is the unstructured subsample of the same SIZE — the rung's holon
    // count expressed as the stride that draws that many.
    for (index, rung) in c.tree.rungs.iter().enumerate() {
        let k = index + 1;
        let cfile = holon
            .join(rung.path.trim_end_matches('/'))
            .join("tiles.parquet");
        let stride = (big / rung.holon_count.max(1)).next_power_of_two().max(1);
        let arm = density(db, &cfile, "TRUE", "\"count\"", extent);
        let null = density(
            db,
            payload,
            &format!("hash(dense_id) % {stride} = 0"),
            "1",
            extent,
        );
        let (dist, noise) = (tv(&base, &arm), tv(&base, &null));
        println!(
            "{:<10} {:>6} {:>10} {dist:>9.4} {noise:>9.4} {:>8}",
            format!("rung r{k}"),
            format!("~{stride}"),
            rung.holon_count,
            if dist <= noise { "pass" } else { "FAIL" },
        );
    }
    println!(
        "  (a rung is rasterised as its CENTROIDS WEIGHTED BY `count`; the other two are one \
         point per row. `~s` is the stride that draws about as many marks, which is what its \
         null is drawn at)"
    );
}

/// **The composite crossover, and why a per-arm column is not it.**
///
/// A coarse arm's own crossover is a budget nobody would ever read it at: `r3`
/// delivers 1,172 marks, so its 27,000-mark figure is the budget at which a
/// stride would match the bytes-per-mark of an artefact that cannot fill a
/// twentieth of that screen. The number that decides anything is the one over
/// the arm a camera would ACTUALLY read at each budget — the scale nearest it —
/// so this sweeps the budget in one-percent steps and reads that.
fn composite(stride_bytes: u64, arms: &[Arm], matched: u64) -> Option<(u64, String)> {
    let mut budget = 100u64;
    while budget <= matched {
        let arm = arms.iter().min_by_key(|a| a.marks.abs_diff(budget))?;
        if stride_bytes as f64 / budget as f64 <= arm.per_mark() {
            return Some((budget, arm.name.clone()));
        }
        budget = (budget * 101 / 100).max(budget + 1);
    }
    None
}

/// **The crossover, on both axes it has one.**
///
/// The question it was asked on is the mark budget, and the answer on that axis
/// is [`crossover`]: one division per scale, because a stride's bytes-per-mark
/// falls as `1 / budget` while an artefact's is fixed by what was written.
///
/// **But the budget is not the axis the decision turns on, and this reports the
/// other one beside it.** A stride's bytes come down only when the footer prunes
/// row groups, so what moves it is the RECTANGLE. The sweep below holds the
/// budget at the screen's `SCREEN_MARKS` and tightens the rectangle until the
/// stride wins — and what it finds is not a budget at all. It is the zoom at
/// which a coarse artefact stops being the thing a camera should read, because
/// the rectangle no longer holds enough vertices to need decimating.
fn crossover_report(
    db: &Connection,
    c: &Corpus,
    payload: &Path,
    by_source: &Path,
    plan: &VertexLevels,
    extent: Rect,
) {
    let pay = lit(payload);
    println!("\n=== 7. THE CROSSOVER ===");

    // ---- on the budget axis, at the far view ----
    let far = Rect::centred(extent.x0, extent.x1, extent.y0, extent.y1, 1.0);
    let matched = u64::try_from(scalar(
        db,
        &format!(
            "SELECT count(*) FROM read_parquet('{pay}') WHERE {}",
            far.sql()
        ),
    ))
    .unwrap_or(0);
    let levels = level_arms(db, c, payload, by_source, plan, far);
    let rungs = rung_arms(db, c, far);
    let stride_bytes = levels.first().map_or(0, |a| a.bytes);
    println!(
        "\n  A. ON THE MARK BUDGET, at the far view. A stride reads {:.0} kB whatever the \
         budget, so its bytes-per-mark is {:.0} kB / budget; an artefact's is flat. They meet \
         at `stride x marks / bytes`.",
        kb(stride_bytes),
        kb(stride_bytes)
    );
    println!(
        "{:<7} {:>10} {:>11} {:>12} {:>16}",
        "arm", "marks", "kB", "B per mark", "stride wins at"
    );
    for arm in levels.iter().skip(1).chain(&rungs) {
        println!(
            "{:<7} {:>10} {:>11.1} {:>12.1} {:>16}",
            arm.name,
            arm.marks,
            kb(arm.bytes),
            arm.per_mark(),
            crossover(stride_bytes, arm, matched).map_or_else(
                || "never (> matched)".to_string(),
                |b| format!("{b:.0} marks")
            ),
        );
    }
    // `l0` is left out of the level arm: it IS the stride's read, and an arm
    // that is the thing it is being compared against cannot be overtaken by it.
    for (what, arms) in [
        ("level pyramid", &levels[1..]),
        ("cell pyramid", &rungs[..]),
    ] {
        match composite(stride_bytes, arms, matched) {
            Some((budget, name)) => println!(
                "  -> the {what} reads fewer bytes per mark than a far-view stride at every \
                 budget below {budget} marks, where the stride catches {name}"
            ),
            None => println!(
                "  -> the {what} is never overtaken at any budget this rectangle can supply"
            ),
        }
    }

    // ---- on the rectangle axis, at the screen's budget ----
    println!(
        "\n  B. ON THE RECTANGLE, at the {SCREEN_MARKS}-mark screen. This is the axis the \
         decision actually turns on: a stride's bytes fall only when the footer prunes row \
         groups, and the budget never prunes one."
    );
    println!(
        "{:>7} {:>10} {:>12} {:>12} {:>7} {:>12} {:>7} {:>12} {:>10}",
        "extent",
        "matched",
        "stride kB",
        "stride B/mk",
        "level",
        "level B/mk",
        "rung",
        "rung B/mk",
        "cheapest"
    );
    for f in [1.0, 0.7, 0.5, 0.35, 0.25, 0.15, 0.1, 0.05] {
        let r = Rect::centred(extent.x0, extent.x1, extent.y0, extent.y1, f);
        let m = u64::try_from(scalar(
            db,
            &format!(
                "SELECT count(*) FROM read_parquet('{pay}') WHERE {}",
                r.sql()
            ),
        ))
        .unwrap_or(0);
        let ls = level_arms(db, c, payload, by_source, plan, r);
        let gs = rung_arms(db, c, r);
        let sb = ls.first().map_or(0, |a| a.bytes);
        let stride = stride_for(m, SCREEN_MARKS);
        let sm = u64::try_from(scalar(
            db,
            &format!(
                "SELECT count(*) FROM read_parquet('{pay}') WHERE {} AND dense_id % {stride} = 0",
                r.sql()
            ),
        ))
        .unwrap_or(0);
        let sper = if sm == 0 {
            f64::INFINITY
        } else {
            sb as f64 / sm as f64
        };
        // The scale of each artefact whose IN-RECTANGLE marks land nearest the
        // screen, which is the one a camera at this zoom would actually read.
        let pick = |arms: &[Arm]| -> usize {
            arms.iter()
                .enumerate()
                .min_by_key(|(_, a)| a.marks.abs_diff(SCREEN_MARKS))
                .map_or(0, |(i, _)| i)
        };
        let (li, gi) = (pick(&ls), pick(&gs));
        let (lper, gper) = (ls[li].per_mark(), gs[gi].per_mark());
        let best = if gper < sper && gper <= lper {
            format!("{} {:.1}x", gs[gi].name, sper / gper)
        } else if lper < sper && ls[li].name != "l0" {
            format!("{} {:.1}x", ls[li].name, sper / lper)
        } else {
            "stride".to_string()
        };
        println!(
            "{:>6.0}% {m:>10} {:>12.1} {sper:>12.1} {:>7} {lper:>12.1} {:>7} {gper:>12.1} {best:>10}",
            f * 100.0,
            kb(sb),
            ls[li].name,
            gs[gi].name,
        );
    }
}

// ===================================================================
//  THE GRID IS A CONSTANT, AND THE VERDICT DEPENDS ON IT
// ===================================================================

/// **The grids the verdict is re-measured on.** They bracket the mark count of
/// every rung from both sides: 64 bins is coarser than all but the last two
/// rungs, 9,216 bins is finer than all but the first.
const SWEEP_GRIDS: &[usize] = &[8, 16, GRID, 96];

/// **The mark budget the production consumer actually spends** —
/// `@kanzo-tech/graph`'s `BOUNDED_DEFAULTS.limit`, which
/// `apps/playground/src/tiles.ts` converts into the door's `pixels` as
/// `√limit × √limit` rather than passing its window, and which
/// `/docs/design/reference-viewer` measures a frame at. Restated here for the
/// same reason [`GRID`] is: it lives on the other side of the wasm boundary and
/// nothing imports it into Rust.
const RENDERER_LIMIT: u64 = 20_000;

/// One row of the sweep: an artefact drawn some way, the marks it delivers, and
/// the stride whose unstructured subsample is its null.
struct SweepArm {
    name: String,
    marks: u64,
    path: PathBuf,
    predicate: &'static str,
    weight: &'static str,
    null_stride: u64,
}

/// The stored pyramids as the fidelity table reads them — the levels off their
/// own files, the rungs as centroids weighted by `count`. Strides are left out:
/// a stride's row is identical to the level's at the same `4^k` and the table
/// has to stay narrow enough to read five times over.
fn sweep_arms(c: &Corpus, big: u64, chunks: &Path, plan: &VertexLevels) -> Vec<SweepArm> {
    let holon = chunks.join(HOLON_PREFIX.trim_end_matches('/'));
    let mut arms = Vec::new();
    for &k in &plan.levels {
        let path = chunks.join(plan.level_prefix(k)).join("tiles.parquet");
        arms.push(SweepArm {
            name: format!("level l{k}"),
            marks: rows_of(&path),
            path,
            predicate: "TRUE",
            weight: "1",
            null_stride: VertexLevels::stride(k),
        });
    }
    for (index, rung) in c.tree.rungs.iter().enumerate() {
        arms.push(SweepArm {
            name: format!("rung r{}", index + 1),
            marks: rung.holon_count,
            path: holon
                .join(rung.path.trim_end_matches('/'))
                .join("tiles.parquet"),
            predicate: "TRUE",
            weight: "\"count\"",
            null_stride: (big / rung.holon_count.max(1)).next_power_of_two().max(1),
        });
    }
    arms
}

/// One arm at one resolution: its distance, its null's, and the gap between
/// them. The gap is the whole verdict — `dist <= noise` is `gap >= 0`.
fn gap_at(
    db: &Connection,
    payload: &Path,
    base: &[f64],
    arm: &SweepArm,
    extent: Rect,
    grid: usize,
) -> (f64, f64) {
    let drawn = density_on(db, &arm.path, arm.predicate, arm.weight, extent, grid);
    let unstructured = format!("hash(dense_id) % {} = 0", arm.null_stride);
    let null = density_on(db, payload, &unstructured, "1", extent, grid);
    (tv(base, &drawn), tv(base, &null))
}

fn sweep_row(name: &str, marks: u64, bins: usize, dist: f64, noise: f64) {
    println!(
        "{name:<10} {marks:>9} {bins:>7} {:>10.2} {dist:>9.4} {noise:>9.4} {:>+8.4} {:>8}",
        bins as f64 / marks.max(1) as f64,
        noise - dist,
        if dist <= noise { "pass" } else { "FAIL" },
    );
}

fn sweep_header() {
    println!(
        "{:<10} {:>9} {:>7} {:>10} {:>9} {:>9} {:>8} {:>8}",
        "arm", "marks", "bins", "bins/mark", "TV", "null TV", "gap", "verdict"
    );
}

/// **Is the tail's failure a property of the pyramid or of the ruler?** — an
/// instrument, `#[ignore]`d like its neighbours, and the second question
/// `fidelity` opens rather than answers.
///
/// `fidelity` measures every arm on ONE grid, `GRID`×`GRID` = 2,304 bins,
/// because that is the number `/docs/design/camera` states and a figure measured
/// at two resolutions is two figures. That constant is safe while an arm has
/// more marks than the grid has bins and **stops being safe below it**: total
/// variation against a normalised base saturates at 1 once the drawn set cannot
/// reach most cells, and a null of the same size saturates with it. Two numbers
/// pinned against the same ceiling differ by their binning noise, which is what
/// a verdict of `FAIL` at 310 marks in 2,304 bins is reporting.
///
/// So this turns the constant into a variable and prints the same table at each
/// value, with `bins/mark` beside it — the ratio that says whether the ruler is
/// in its range — and then once more on a grid **fitted to each arm**, about
/// `sqrt(marks)` bins per axis, where no arm is asked to fill more bins than it
/// has points.
///
/// The fitted table is the discriminating one and it is not comparable ACROSS
/// arms: every row is measured on its own grid, so a distance on one line and a
/// distance on the next are answers to two questions. What is comparable is the
/// GAP, because an arm and its null are always weighed on the same grid.
///
/// `cargo test -p fossil-layout --test level_vs_rung -- --ignored --nocapture`
#[test]
#[ignore = "an instrument: it writes a 300,000-vertex corpus and re-measures fidelity at five resolutions"]
fn what_grid_resolution_does_to_the_fidelity_verdict_on_a_planted_corpus() {
    let c = write_corpus("level_vs_rung_grid");
    resolution_sweep(&c, u64::from(BIG), "planted partition, mean degree 14");
}

/// The same sweep over com-DBLP, which is the arm the failing verdict was
/// reported on and the one whose tail disagrees with the planted fixture's.
///
/// `cargo test -p fossil-layout --test level_vs_rung -- --ignored --nocapture`
#[test]
#[ignore = "an instrument: it reads com-DBLP off disk and re-measures fidelity at five resolutions"]
fn what_grid_resolution_does_to_the_fidelity_verdict_on_com_dblp() {
    let Some(c) = dblp_corpus("level_vs_rung_grid_dblp") else {
        println!("com-DBLP is not checked out at {DBLP_CSV}; skipping");
        return;
    };
    let v = c.vertex_count;
    resolution_sweep(&c, v, "com-DBLP, both directions");
}

#[allow(clippy::too_many_lines)]
fn resolution_sweep(c: &Corpus, big: u64, shape: &str) {
    let chunks = c.root.join("chunks");
    let payload = chunks.join("tiles.parquet");

    let db = Connection::open_in_memory().expect("duckdb");
    let pay = lit(&payload);
    let (min_x, max_x, min_y, max_y): (f64, f64, f64, f64) = db
        .query_row(
            &format!("SELECT min(x), max(x), min(y), max(y) FROM read_parquet('{pay}')"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the extent");
    let extent = Rect {
        x0: min_x,
        x1: max_x,
        y0: min_y,
        y1: max_y,
    };

    let plan = VertexLevels::planned(big, c.chunk).expect("over one tile");
    let arms = sweep_arms(c, big, &chunks, &plan);

    println!("\n=== IS THE VERDICT A PROPERTY OF THE PYRAMID OR OF THE GRID? ===");
    println!(
        "  {big} vertices ({shape}), vertices_per_cell {}",
        c.tree.vertices_per_cell
    );
    println!(
        "  every row is one arm against the payload, and the tolerance is always the null of the \
         same SIZE: `hash(dense_id) % s = 0`. `gap` is `null TV - TV`, so `pass` is `gap >= 0`. \
         `bins/mark` is the ratio that says whether the ruler is in its range — above 1 the arm \
         is being asked to fill more cells than it has points."
    );

    for &grid in SWEEP_GRIDS {
        println!(
            "\n-- grid {grid}x{grid} = {} bins{} --",
            grid * grid,
            if grid == GRID {
                "   (the constant `/docs/design/camera` states, and what `=== 6 ===` reports)"
            } else {
                ""
            }
        );
        sweep_header();
        let base = density_on(&db, &payload, "TRUE", "1", extent, grid);
        for arm in &arms {
            let (dist, noise) = gap_at(&db, &payload, &base, arm, extent, grid);
            sweep_row(&arm.name, arm.marks, grid * grid, dist, noise);
        }
    }

    println!(
        "\n-- fitted: each arm on about `sqrt(marks)` bins per axis, so no arm fills more bins than it has points --"
    );
    println!(
        "  the distances down this table are NOT comparable to each other — every row is a \
         different grid. The GAP is, because an arm and its null are always weighed on the same one."
    );
    sweep_header();
    for arm in &arms {
        let grid = fitted_grid(arm.marks);
        let base = density_on(&db, &payload, "TRUE", "1", extent, grid);
        let (dist, noise) = gap_at(&db, &payload, &base, arm, extent, grid);
        sweep_row(&arm.name, arm.marks, grid * grid, dist, noise);
    }
    println!(
        "  (a grid of one bin is the degenerate ruler — every distribution is the same one — so \
         the fit floors at 2 bins per axis, which is the coarsest question that still has an answer)"
    );

    asked_for(c, big);
}

/// **A grid fitted to a mark count**: about one bin per mark, which is
/// `sqrt(marks)` bins per axis.
///
/// Floored at 2 and not at 1 because a 1×1 grid holds all the ink of every
/// distribution in its one cell, so every TV over it is 0 and every verdict is
/// `pass` — a ruler with no marks on it. 2 is the coarsest grid that still
/// distinguishes two pictures.
fn fitted_grid(marks: u64) -> usize {
    usize::try_from(marks.isqrt().max(2)).unwrap_or(2)
}

/// **The mark count below which nothing asks**, derived from what the tree
/// already states rather than from a number invented here, and which rung of
/// this corpus it lands on.
///
/// **A camera asks for a budget in marks and takes the coarsest artefact that
/// still carries them**, so the floor on what any artefact is read for is the
/// smallest budget anything spends — not the deepest zoom. Zooming IN does not
/// lower it: `levelForCanvas` estimates what the rectangle holds and answers
/// level 0 the moment that estimate drops below the budget, which puts the read
/// on the payload rather than on a coarser rung.
///
/// Three budgets, all of them already in the tree:
///
/// - **[`RENDERER_LIMIT`] — what the production consumer actually spends.**
///   `apps/playground/src/tiles.ts` does not pass its canvas. It passes
///   `pixels = √limit × √limit`, so the door's one-mark-per-pixel rule spends
///   exactly `BOUNDED_DEFAULTS.limit` marks whatever the window is, and that file
///   argues the point at length: deriving from the real canvas asks for 368,000
///   marks on a 920×400 window, drops the frame two levels and draws 250,000
///   points where 15,625 were asked for. So this is a budget and not a ceiling
///   on one.
/// - **[`SCREEN_MARKS`]**, the number `HolonTree::vertices_per_cell` picks the
///   *base* of the pyramid with: *«a megapixel canvas draws about fifteen
///   thousand marks comfortably»*. The smallest of the three, so it is the one
///   that decides.
/// - **The canvas, if the door ever did take it.** `FrameParams.pixels` is
///   *«one mark per pixel»* and its own doc says what to do about a mark wider
///   than one: *«Marks four pixels wide: pass the canvas divided by four»*. At
///   the `BOUNDED_DEFAULTS.minLinkPixels` of 3 that is [`MIN_LINK_PX`], on the
///   [`CANVAS_PX`] canvas both instruments in this crate measure their pixel
///   floor on, that is `(1200 / 3)²` — an order of magnitude ABOVE the other
///   two, which is `tiles.ts`'s complaint restated.
fn asked_for(c: &Corpus, big: u64) {
    let pixel_floor = ((CANVAS_PX / MIN_LINK_PX) * (CANVAS_PX / MIN_LINK_PX)) as u64;
    println!("\n=== WHAT ANYTHING ACTUALLY ASKS FOR ===");
    println!(
        "  a camera spends a budget in MARKS and takes the coarsest artefact that still carries \
         them, so the floor is the smallest budget anything spends — not the deepest zoom. \
         zooming in shrinks what the rectangle holds, which answers level 0 and puts the read on \
         the payload rather than on a coarser rung."
    );
    println!(
        "  BOUNDED_DEFAULTS.limit, what `tiles.ts` actually spends  : {RENDERER_LIMIT:>9} marks"
    );
    println!(
        "  a megapixel canvas, comfortably (SCREEN_MARKS)           : {SCREEN_MARKS:>9} marks"
    );
    println!(
        "  the {CANVAS_PX} px canvas at {MIN_LINK_PX} px a mark, if the door took it   : {pixel_floor:>9} marks"
    );
    let floor = SCREEN_MARKS.min(RENDERER_LIMIT).min(pixel_floor);
    println!("  the floor is the smallest of the three                   : {floor:>9} marks");

    println!(
        "\n{:<10} {:>10} {:>12} {:>10}",
        "rung", "marks", "vs floor", "asked for"
    );
    let mut deepest: Option<String> = None;
    for (index, rung) in c.tree.rungs.iter().enumerate() {
        let name = format!("r{}", index + 1);
        let reached = rung.holon_count >= floor;
        if reached {
            deepest = Some(name.clone());
        }
        println!(
            "{name:<10} {:>10} {:>11.2}x {:>10}",
            rung.holon_count,
            rung.holon_count as f64 / floor as f64,
            if reached { "yes" } else { "no" },
        );
    }
    match deepest {
        Some(name) => println!(
            "  the deepest rung anything ever asks for is {name}, out of {} the tree publishes \
             for {big} vertices",
            c.tree.rungs.len()
        ),
        None => println!("  no rung of this tree carries as many marks as the floor asks for"),
    }
}
