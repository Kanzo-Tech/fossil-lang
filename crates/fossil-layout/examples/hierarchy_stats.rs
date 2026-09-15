//! **Is there a level of the community hierarchy worth drawing?**
//!
//! The write path finds a whole hierarchy and keeps two things out of it: the
//! FINEST level, which becomes the placement, and ONE level chosen by
//! `flatten_to_budget`, which becomes `cluster_id`. Every level between them,
//! and the quotient graph each was found on, is freed.
//!
//! That is what makes the question unanswerable from outside: on com-DBLP the
//! finest level is 55,712 communities of median size 3, 53.5% of them holding
//! three members or fewer -- not a structure, very nearly the graph itself --
//! and the coarsest is 595 whose quotient has a mean degree of 23.2, which over
//! 595 nodes is 3.9% density and dense enough that a force layout over it is a
//! hairball. Neither is a level a picture can be built on, and whether a middle
//! one exists is a property of the DATA that no amount of reasoning settles.
//!
//! (Those four numbers are what THIS program prints, re-measured 2026-09-11.
//! They read 98,505 / mean 3.2 / 1,229 / 80 until then, which is a partition
//! this example cannot produce from this input -- it reports 317,080 vertices
//! and 1,049,866 edges, which are com-DBLP's own figures. The most likely
//! reading is that they predate `read_edges` densifying, and they are replaced
//! rather than annotated because the program is the reference.)
//!
//! So this measures it, per level:
//!
//! - how many communities, and how their sizes are distributed;
//! - **modularity on the original graph**, which is the number that says whether
//!   a partition found structure or cut the graph arbitrarily;
//! - the quotient's own density, which is what decides whether that level can be
//!   laid out at all.
//!
//! It adds no public API: `community_hierarchy` was already exported, and the
//! quotient of a level is recomputed here from the edge list and the membership
//! rather than reaching for the one the pass built and dropped.
//!
//! ```text
//! cargo run --release --example hierarchy_stats -- \
//!     apps/playground/bench/dblp/data/links.csv [out-dir]
//! ```
//!
//! With `out-dir`, every level's membership and quotient edge list is written
//! there as CSV, so a layout can be tried on a chosen level without running
//! this again. A third argument sets the chunk the cut below stops at.
//!
//! **And then it cuts.** The table above is what Louvain emitted; the table at
//! the end is what `Dendrogram::cut` publishes out of it — the levels whose
//! group counts fall by at least the factor the tile pyramid declares, which is
//! the whole of `/docs/design/holons`'s second property. The two tables side by
//! side are the argument: the levels the cut refuses are exactly the ones whose
//! contraction had collapsed.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use fossil_layout::layout::cluster_layout;
use fossil_layout::layout::community::{Cut, Dendrogram, order_by_hierarchy};
use fossil_layout::layout::morton::{morton_codes, morton_ranks};
use fossil_sinks::manifest::{DEFAULT_CHUNK_SIZE, DEFAULT_VERTICES_PER_CELL, HolonTree};

/// One community's aggregate over the ORIGINAL graph, which is what modularity
/// is defined against — a level's quality is a statement about the graph it
/// partitions, not about the quotient it produced.
#[derive(Default, Clone, Copy)]
struct Community {
    /// Edge weight with both ends inside.
    internal: f64,
    /// Sum of member degrees.
    degree: f64,
    members: u64,
}

/// Read the edge list, **densifying the ids**.
///
/// `max(id) + 1` is not the vertex count of a file whose ids have gaps, and
/// getting that wrong does not fail loudly -- it invents isolated vertices. On
/// com-DBLP the SNAP ids run to 425,957 for 317,080 real authors, so taking the
/// maximum conjures 108,877 vertices with no edges, each of which is its own
/// community at every level and can never merge. Measured both ways: it moved
/// "communities of size <= 3" from 53.5% to 99.7% and stalled the coarsening
/// dead, which would have read as "this graph has no hierarchy" when what it
/// had was a counting error.
///
/// The 53.5% is what this program prints (re-measured 2026-09-11); it read
/// 83.1% and that figure is reproducible from no run of this file.
///
/// The corpus does not have this problem -- `dense_id` is gapless by
/// construction -- so this is a property of reading the raw file.
fn read_edges(path: &str) -> (u32, Vec<(u32, u32)>) {
    let text = fs::read_to_string(path).expect("read edge list");
    let mut dense: HashMap<u32, u32> = HashMap::new();
    let mut edges = Vec::new();
    for line in text.lines().skip(1) {
        let Some((a, b)) = line.split_once(',') else {
            continue;
        };
        let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) else {
            continue;
        };
        // The file carries both orientations of every undirected link; keep one,
        // because modularity counts an edge once.
        if a >= b {
            continue;
        }
        let next = u32::try_from(dense.len()).expect("more than u32::MAX vertices");
        let da = *dense.entry(a).or_insert(next);
        let next = u32::try_from(dense.len()).expect("more than u32::MAX vertices");
        let db = *dense.entry(b).or_insert(next);
        edges.push((da, db));
    }
    (
        u32::try_from(dense.len()).expect("more than u32::MAX vertices"),
        edges,
    )
}

/// Counts as `f64`, via `u32`.
///
/// These are community and edge counts, not addresses: a corpus with more than
/// `u32::MAX` of either is one `dense_id` cannot address anyway. Going through
/// `u32` makes that explicit instead of leaving a `usize as f64` the lint has to
/// warn about on the general case.
fn ratio(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Nearest-rank percentile, `permille` in thousandths.
///
/// Integer arithmetic on purpose: a percentile of a length is a fraction of a
/// count, and rounding an `f64` back into an index is the one step that can be
/// off by one at the ends.
fn percentile(sorted: &[u64], permille: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let last = sorted.len() - 1;
    sorted[((last * permille) / 1000).min(last)]
}

// ─────────────────────────────────────────────────────── and then it places ──
//
// The finest level is the placement partition, and the plane it is placed on is
// the second half of the same question: a level worth drawing still has to be
// drawn somewhere. What follows is the baseline this repository replaced and the
// rule that replaced it, side by side over the same partition.

/// Phyllotaxis packing radius, and the golden angle that spaces it — the two
/// constants both placements share.
const INTRA_CLUSTER_RADIUS: f32 = 12.0;
const GOLDEN_ANGLE: f32 = 2.399_963_2;
/// The constant gap the uniform pitch added to the largest disc's diameter.
const CLUSTER_SPACING: f32 = 100.0;

// Deliberate numeric code, and scoped to the two functions that need it rather
// than to the file: a placement is `f32` geometry over `u32` counts and this
// prints areas as integers. The `ratio` helper above keeps the level table free
// of it, which is why the allow is not at the top.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
/// **The placement this measurement is the baseline for**: one grid, one pitch,
/// and the pitch is the largest group's disc plus a constant.
///
/// A copy, deliberately, and it is the only honest way to print two rows of one
/// table — `fossil_layout::layout::cluster_layout` is the row underneath and
/// there is no version of the crate that holds both. What keeps the copy true is
/// that the numbers it produces here are the ones `/docs/design/position`
/// already published from the corpus itself: an extent of 560,176 and an
/// occupancy of 0.046% over com-DBLP.
fn uniform_pitch_layout(cluster_ids: &[u32]) -> Vec<(f32, f32)> {
    let num_clusters = cluster_ids.iter().copied().max().map_or(0, |m| m + 1);
    if num_clusters == 0 {
        return Vec::new();
    }
    let mut sizes = vec![0u32; num_clusters as usize];
    for &c in cluster_ids {
        sizes[c as usize] += 1;
    }
    let largest = sizes.iter().copied().max().unwrap_or(0);
    let pitch = 2.0f32.mul_add(
        INTRA_CLUSTER_RADIUS * f64::from(largest).sqrt() as f32,
        CLUSTER_SPACING,
    );
    let mut seen = vec![0u32; num_clusters as usize];
    let mut out = Vec::with_capacity(cluster_ids.len());
    for &c in cluster_ids {
        let (col, row) = morton_decode(c);
        let k = seen[c as usize];
        seen[c as usize] += 1;
        let angle = k as f32 * GOLDEN_ANGLE;
        let radius = INTRA_CLUSTER_RADIUS * ((k as f32) + 1.0).sqrt();
        out.push((
            radius.mul_add(angle.cos(), col as f32 * pitch),
            radius.mul_add(angle.sin(), row as f32 * pitch),
        ));
    }
    out
}

/// The grid cell a cluster id occupies, for [`uniform_pitch_layout`] only — the
/// placement under test addresses by a frontier and needs no inverse.
const fn morton_decode(code: u32) -> (u32, u32) {
    const fn compact(mut n: u32) -> u32 {
        n &= 0x5555_5555;
        n = (n | (n >> 1)) & 0x3333_3333;
        n = (n | (n >> 2)) & 0x0f0f_0f0f;
        n = (n | (n >> 4)) & 0x00ff_00ff;
        n = (n | (n >> 8)) & 0x0000_ffff;
        n
    }
    (compact(code), compact(code >> 1))
}

/// A box, as the four numbers a bounding box is.
#[derive(Clone, Copy)]
struct Bbox {
    xlo: f64,
    ylo: f64,
    xhi: f64,
    yhi: f64,
}

impl Bbox {
    const EMPTY: Self = Self {
        xlo: f64::MAX,
        ylo: f64::MAX,
        xhi: f64::MIN,
        yhi: f64::MIN,
    };
    fn see(&mut self, (x, y): (f32, f32)) {
        self.xlo = self.xlo.min(f64::from(x));
        self.ylo = self.ylo.min(f64::from(y));
        self.xhi = self.xhi.max(f64::from(x));
        self.yhi = self.yhi.max(f64::from(y));
    }
    fn area(self) -> f64 {
        if self.xhi < self.xlo {
            return 0.0;
        }
        (self.xhi - self.xlo) * (self.yhi - self.ylo)
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
/// **What one placement costs the plane and the id axis**, printed as two rows
/// of the same table.
///
/// `label` names the placement, `positions` is what it returned and `placement`
/// the per-vertex group it was given. Everything below is derived from those
/// three and from the addressing rule the corpus publishes, so the numbers are
/// the corpus's own rather than the placement's opinion of itself.
fn plane_report(label: &str, positions: &[(f32, f32)], placement: &[u32]) {
    let groups = placement.iter().copied().max().map_or(0, |m| m + 1) as usize;
    let mut sizes = vec![0u64; groups];
    for &c in placement {
        sizes[c as usize] += 1;
    }

    let mut whole = Bbox::EMPTY;
    for &p in positions {
        whole.see(p);
    }
    // What the members themselves cover: one phyllotaxis disc per group, at the
    // radius the last member reached. Discs do not overlap in either placement,
    // so the sum is the occupied area and not an upper bound on it.
    let discs: f64 = sizes
        .iter()
        .map(|&m| {
            std::f64::consts::PI * (f64::from(INTRA_CLUSTER_RADIUS) * (m as f64).sqrt()).powi(2)
        })
        .sum();

    // The addressing, exactly as a reader would recompute it.
    let codes = morton_codes(positions);
    let (rank, order) = morton_ranks(&codes);

    // Is a group one run of `dense_id`, or several? The placement claims one:
    // a group is one aligned block of the quaternary, and the frontier hands
    // blocks out in id order.
    let mut runs = vec![0u64; groups];
    let mut previous: Option<u32> = None;
    for &old in &order {
        let c = placement[old as usize];
        if previous != Some(c) {
            runs[c as usize] += 1;
        }
        previous = Some(c);
    }
    let mut run_counts: Vec<u64> = runs.clone();
    run_counts.sort_unstable();
    let one_run = runs.iter().filter(|&&r| r == 1).count();

    // And is the id axis the group order? Every union of consecutive groups —
    // which is every coarser level of the dendrogram — is an interval of
    // `dense_id` exactly when it is.
    let mut monotone = true;
    let mut highest = 0u32;
    for &old in &order {
        let c = placement[old as usize];
        if c < highest {
            monotone = false;
            break;
        }
        highest = c;
    }

    println!("\n── {label}");
    println!(
        "  extensión {:>10.0} x {:<10.0} ocupación {:>7.3}%   discos {:>12.0}",
        whole.xhi - whole.xlo,
        whole.yhi - whole.ylo,
        100.0 * discs / whole.area(),
        discs,
    );
    println!(
        "  tiradas por grupo: p50 {}, p90 {}, máx {} — {one_run} de {groups} en una sola",
        percentile(&run_counts, 500),
        percentile(&run_counts, 900),
        run_counts.last().copied().unwrap_or(0),
    );
    println!(
        "  el eje de ids sigue el orden de grupo: {}",
        if monotone { "sí" } else { "NO" },
    );

    // The pyramid's own question: is a rung's cell a fixed area, or only a fixed
    // count? `dense_id >> shift` is the cell, exactly as the writer cuts it.
    let vertex_count = positions.len() as u64;
    println!(
        "  {:>5} {:>9} {:>12} {:>12} {:>12} {:>12} {:>11}",
        "rung", "celdas", "área mín", "área p50", "área p99", "área máx", "máx/mín"
    );
    for rung in 1u32..=6 {
        let Some(shift) = HolonTree::shift_at(DEFAULT_VERTICES_PER_CELL, rung) else {
            break;
        };
        let Some(cells) = HolonTree::holons_at(vertex_count, DEFAULT_VERTICES_PER_CELL, rung)
        else {
            break;
        };
        let mut boxes = vec![Bbox::EMPTY; cells as usize];
        for (v, &r) in rank.iter().enumerate() {
            boxes[(r >> shift) as usize].see(positions[v]);
        }
        // Cells the numbering leaves short — the last one, and any the tail of a
        // group left with a single member — carry a degenerate box that is not a
        // statement about the placement, so the range is taken over the full
        // ones only.
        let mut areas: Vec<u64> = boxes
            .iter()
            .enumerate()
            .filter(|(i, _)| (*i as u64 + 1) << shift <= vertex_count)
            .map(|(_, b)| b.area() as u64)
            .collect();
        areas.sort_unstable();
        let (lo, hi) = (
            areas.first().copied().unwrap_or(0),
            areas.last().copied().unwrap_or(0),
        );
        println!(
            "  {rung:>5} {cells:>9} {lo:>12} {:>12} {:>12} {hi:>12} {:>10.0}x",
            percentile(&areas, 500),
            percentile(&areas, 990),
            if lo == 0 {
                f64::INFINITY
            } else {
                hi as f64 / lo as f64
            },
        );
        if cells <= 1 {
            break;
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "apps/playground/bench/dblp/data/links.csv".to_string());
    let out: Option<PathBuf> = args.next().map(PathBuf::from);
    let chunk: u64 = args
        .next()
        .map_or(DEFAULT_CHUNK_SIZE, |a| a.parse().expect("chunk size"));

    let (vertex_count, edges) = read_edges(&path);
    println!(
        "grafo: {vertex_count} vértices, {} aristas no dirigidas",
        edges.len()
    );

    let mut degree = vec![0f64; vertex_count as usize];
    for &(a, b) in &edges {
        degree[a as usize] += 1.0;
        degree[b as usize] += 1.0;
    }
    let two_m = degree.iter().sum::<f64>();

    // The dendrogram, adopted whole: the level table below reads it by the same
    // lookup walk the cut does, rather than by a second copy of that walk.
    let tree = Dendrogram::of(vertex_count, &edges);
    println!("la jerarquía tiene {} niveles\n", tree.depth());

    println!(
        "{:>5} {:>10} {:>8} {:>8} {:>8} {:>8} {:>11} {:>9} {:>12}",
        "nivel", "comunid.", "p50", "p90", "máx", "%<=3", "modularidad", "q-aristas", "q-grado med"
    );

    for l in 0..tree.depth() {
        let label = tree.membership(l).expect("a level of this hierarchy");
        let n = label.iter().copied().max().map_or(0, |m| m + 1);

        let mut comms = vec![Community::default(); n as usize];
        for v in 0..vertex_count as usize {
            let c = label[v] as usize;
            comms[c].degree += degree[v];
            comms[c].members += 1;
        }
        // The quotient, and the internal weight, in one pass over the edges.
        let mut quotient: HashMap<(u32, u32), f64> = HashMap::new();
        for &(a, b) in &edges {
            let (ca, cb) = (label[a as usize], label[b as usize]);
            if ca == cb {
                comms[ca as usize].internal += 1.0;
            } else {
                *quotient.entry((ca.min(cb), ca.max(cb))).or_default() += 1.0;
            }
        }

        // Newman-Girvan on the original graph: sum over communities of
        // (internal / m) - (degree / 2m)^2.
        let m = two_m / 2.0;
        let modularity: f64 = comms
            .iter()
            .map(|c| (c.degree / two_m).mul_add(-(c.degree / two_m), c.internal / m))
            .sum();

        let mut sizes: Vec<u64> = comms.iter().map(|c| c.members).filter(|&s| s > 0).collect();
        sizes.sort_unstable();
        let small = sizes.iter().filter(|&&s| s <= 3).count();
        let q_edges = quotient.len();
        let q_degree = if n > 0 {
            2.0 * ratio(q_edges) / f64::from(n)
        } else {
            0.0
        };

        println!(
            "{:>5} {:>10} {:>8} {:>8} {:>8} {:>7.1}% {:>11.4} {:>9} {:>12.1}",
            l + 1,
            n,
            percentile(&sizes, 500),
            percentile(&sizes, 900),
            sizes.last().copied().unwrap_or(0),
            100.0 * ratio(small) / ratio(sizes.len().max(1)),
            modularity,
            q_edges,
            q_degree,
        );

        if let Some(dir) = &out {
            fs::create_dir_all(dir).expect("create out dir");
            let mut f = fs::File::create(dir.join(format!("membership-l{}.csv", l + 1)))
                .expect("membership file");
            writeln!(f, "vertex,community").unwrap();
            for (v, c) in label.iter().enumerate() {
                writeln!(f, "{v},{c}").unwrap();
            }
            let mut g = fs::File::create(dir.join(format!("quotient-l{}.csv", l + 1)))
                .expect("quotient file");
            writeln!(g, "src,dst,weight").unwrap();
            let mut rows: Vec<_> = quotient.into_iter().collect();
            rows.sort_unstable_by_key(|&((a, b), _)| (a, b));
            for ((a, b), w) in rows {
                writeln!(g, "{a},{b},{w}").unwrap();
            }
        }
    }

    // El corte declarado: de todos esos niveles, los que valen un paso.
    let cut = tree.cut(chunk);
    println!(
        "\ncorte declarado (factor {}, chunk {chunk}): {} de {} niveles",
        Cut::declared_branching(),
        cut.len(),
        tree.depth(),
    );
    println!(
        "{:>5} {:>10} {:>10} {:>13}",
        "nivel", "grupos", "techo", "contracción"
    );
    for (rung, ratio) in cut.rungs().iter().zip(cut.contractions()) {
        println!(
            "{:>5} {:>10} {:>10} {:>12.2}x",
            rung.level() + 1,
            rung.groups(),
            rung.ceiling(),
            ratio,
        );
    }
    let dropped: Vec<usize> = (0..tree.depth())
        .filter(|l| !cut.rungs().iter().any(|r| r.level() == *l))
        .map(|l| l + 1)
        .collect();
    if dropped.is_empty() {
        println!("ningún nivel se descarta");
    } else {
        println!("niveles descartados: {dropped:?} — ninguno es un cuarto del que tiene encima");
    }

    // El plano: la partición más fina es la que coloca, y las dos reglas que la
    // han colocado, una debajo de la otra.
    let mut placement = tree
        .membership(0)
        .unwrap_or_else(|| (0..vertex_count).collect());
    order_by_hierarchy(tree.levels(), 0, &mut placement);
    let groups = placement.iter().copied().max().map_or(0, |m| m + 1);
    println!(
        "\nel plano, sobre la partición que coloca: {groups} grupos, base {DEFAULT_VERTICES_PER_CELL} vértices/celda"
    );
    plane_report(
        "paso uniforme por el grupo máximo",
        &uniform_pitch_layout(&placement),
        &placement,
    );
    plane_report(
        "una celda por grupo",
        &cluster_layout(&placement),
        &placement,
    );

    if let Some(dir) = &out {
        println!("\nniveles volcados en {}", dir.display());
    }
}
