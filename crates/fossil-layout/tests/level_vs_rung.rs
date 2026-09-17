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
//! So this writes one corpus and measures both sides of it:
//!
//! - the level pyramid — `chunks/l{k}/` and the edge levels at `l{k}/`;
//! - the cell pyramid — `chunks/holon/r{k}/` and `chunks/holon/r{k}/quotient/`;
//! - and the head-to-head at the scales where both can answer.
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
// count becomes an `f64` on its way to a table. `tuple_array_conversions` is
// declined beside them because the pair being flattened IS an edge, and spelling
// that as an array conversion says less than the pattern does.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
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
    let (min_x, max_x): (f64, f64) = db
        .query_row(
            &format!("SELECT min(x), max(x) FROM read_parquet('{pay}')"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the extent");
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
}
