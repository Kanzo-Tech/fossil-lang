//! How much of the plane a payload tile's box claims, and how many tiles a
//! small window therefore opens — the measurement behind the proposal on
//! `/docs/design/backend`.
//!
//! ```text
//! cargo run --release -p fossil-layout --example enrich_memory -- 1000000 14
//! cargo run --release -p fossil-layout --example tile_compactness -- <dir>/chunks/tiles.parquet
//! ```
//!
//! Three numbers per corpus:
//!
//! - **compactness** — a tile's box area over the area its share of the rows
//!   would cover on a uniformly filled extent, `area / (extent × rows / V)`. One
//!   is a square of exactly its own rows; a Z-curve interval that crosses a quad
//!   boundary is a hull over two quads and reads far above it.
//! - **tiles hit** — for square windows centred on randomly chosen vertices,
//!   how many tile boxes the window intersects, which is what a view culling
//!   against `tileMatrix` opens.
//! - **tiles holding** — how many of those tiles hold a vertex that is in the
//!   window. No box of any shape can do better with the same tiles, so the gap
//!   between the two is the part of the cost the geometry owns, and what is left
//!   is the tile granularity.
//! - **falls back to cells** — the share of windows whose intersected tiles hold
//!   more than a view's 20,000-row budget, so the finest zoom that fits is a
//!   rung, at window sizes from a few hundred vertices to five thousand.
//!
//! It reads the payload directly rather than the tile manifest, because the
//! manifest's box IS the tile's `x`/`y` bounds and this also needs the rows.

use std::fmt::Write as _;

use duckdb::Connection;

const TILE: usize = 4096;
const WINDOWS: usize = 400;
/// The row budget kanzo-ui's view reported a window falling back at.
const BUDGET: usize = 20_000;

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: tile_compactness <payload tiles.parquet>");
    let db = Connection::open_in_memory().expect("duckdb");
    let mut stmt = db
        .prepare(&format!(
            "SELECT x, y FROM read_parquet('{path}') ORDER BY dense_id"
        ))
        .expect("prepare");
    let points: Vec<(f32, f32)> = stmt
        .query_map([], |r| Ok((r.get::<_, f32>(0)?, r.get::<_, f32>(1)?)))
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    let n = points.len();
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for &(x, y) in &points {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let (ew, eh) = (f64::from(x1 - x0), f64::from(y1 - y0));

    let boxes: Vec<[f32; 4]> = points
        .chunks(TILE)
        .map(|tile| {
            tile.iter().fold(
                [f32::MAX, f32::MIN, f32::MAX, f32::MIN],
                |[a, b, c, d], &(x, y)| [a.min(x), b.max(x), c.min(y), d.max(y)],
            )
        })
        .collect();
    let mut ratio: Vec<f64> = boxes
        .iter()
        .zip(points.chunks(TILE))
        .map(|(b, tile)| {
            f64::from(b[1] - b[0]) * f64::from(b[3] - b[2])
                / (ew * eh * tile.len() as f64 / n as f64)
        })
        .collect();
    ratio.sort_by(f64::total_cmp);
    println!(
        "{path}\n  {n} rows in {} tiles; compactness p50 {:.2} p90 {:.2} max {:.1}",
        boxes.len(),
        pct(&ratio, 0.5),
        pct(&ratio, 0.9),
        ratio[ratio.len() - 1]
    );

    // A fixed LCG, so the same corpus gives the same windows on every run.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let centres: Vec<usize> = (0..WINDOWS)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize % n
        })
        .collect();

    for (label, target) in [
        ("~5,000-vertex window", Some(5_000)),
        ("1% of the extent", None),
    ] {
        let side = target.map_or(0.1, |t| side_holding(&points, &centres, ew, eh, t));
        let (mut hit, mut holding, mut inside) = (vec![], vec![], vec![]);
        for &c in &centres {
            let (cx, cy) = points[c];
            let (hw, hh) = ((side * ew / 2.0) as f32, (side * eh / 2.0) as f32);
            let (left, right, bottom, top) = (cx - hw, cx + hw, cy - hh, cy + hh);
            hit.push(
                boxes
                    .iter()
                    .filter(|b| b[1] >= left && b[0] <= right && b[3] >= bottom && b[2] <= top)
                    .count() as f64,
            );
            let mut tiles = 0;
            let mut count = 0;
            for tile in points.chunks(TILE) {
                let k = tile
                    .iter()
                    .filter(|&&(x, y)| x >= left && x <= right && y >= bottom && y <= top)
                    .count();
                tiles += usize::from(k > 0);
                count += k;
            }
            holding.push(tiles as f64);
            inside.push(count as f64);
        }
        for v in [&mut hit, &mut holding, &mut inside] {
            v.sort_by(f64::total_cmp);
        }
        println!(
            "  {label} (side {side:.4} of the extent, {:.0} vertices at p50): \
             tiles hit p50 {:.0} p90 {:.0} max {:.0} = {:.0} / {:.0} rows; \
             tiles holding a window vertex p50 {:.0} p90 {:.0}",
            pct(&inside, 0.5),
            pct(&hit, 0.5),
            pct(&hit, 0.9),
            hit[hit.len() - 1],
            pct(&hit, 0.5) * TILE as f64,
            pct(&hit, 0.9) * TILE as f64,
            pct(&holding, 0.5),
            pct(&holding, 0.9),
        );
    }

    // The share of windows a view with a 20,000-row budget draws as cells,
    // over the first 300 centres: a window falls back when the rows of the
    // tiles its box intersects exceed the budget.
    let mut line = String::from("  falls back to cells at 20,000 rows:");
    for target in [385, 855, 1_500, 2_350, 3_400, 5_000] {
        let side = side_holding(&points, &centres, ew, eh, target);
        let (hw, hh) = ((side * ew / 2.0) as f32, (side * eh / 2.0) as f32);
        let over = centres[..300]
            .iter()
            .filter(|&&c| {
                let (cx, cy) = points[c];
                let rows: usize = boxes
                    .iter()
                    .zip(points.chunks(TILE))
                    .filter(|(b, _)| {
                        b[1] >= cx - hw && b[0] <= cx + hw && b[3] >= cy - hh && b[2] <= cy + hh
                    })
                    .map(|(_, tile)| tile.len())
                    .sum();
                rows > BUDGET
            })
            .count();
        let _ = write!(line, " ~{target} {}%", over * 100 / 300);
    }
    println!("{line}");
}

fn pct(sorted: &[f64], q: f64) -> f64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i]
}

/// The window side, as a fraction of the extent, whose median over the first
/// sixty centres holds `target` vertices — bisected, since the density varies.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn side_holding(points: &[(f32, f32)], centres: &[usize], ew: f64, eh: f64, target: usize) -> f64 {
    let (mut lo, mut hi) = (1e-4, 0.5);
    for _ in 0..25 {
        let mid = f64::midpoint(lo, hi);
        let (hw, hh) = ((mid * ew / 2.0) as f32, (mid * eh / 2.0) as f32);
        let mut counts: Vec<usize> = centres[..60]
            .iter()
            .map(|&c| {
                let (cx, cy) = points[c];
                points
                    .iter()
                    .filter(|&&(x, y)| (x - cx).abs() <= hw && (y - cy).abs() <= hh)
                    .count()
            })
            .collect();
        counts.sort_unstable();
        if counts[30] < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    f64::midpoint(lo, hi)
}
