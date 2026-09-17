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
//!
//! **And then it asks what the sibling order costs.** `order_by_hierarchy`
//! fixed the top half of one defect and says so: a community's position among
//! its SIBLINGS is still its id, which is `dense_id` order, which is IRI order,
//! which has nothing to do with the graph. The last section prices the half
//! left standing — five sibling rules over one dendrogram and one
//! `cluster_layout`, so every difference is the rule. It answers three
//! questions per rule: how much of the edge list never leaves one tile, how
//! full each rung's quotient is, and how far that rung's cell areas spread.
//!
//! Two of the five rules are controls and they are what make the other three
//! readable. **`Flat`** is the raw Louvain numbering — the defect entire, before
//! `order_by_hierarchy` — so the distance from it to `Id` is what fixing the top
//! half bought, in the same units. **`Size`** orders siblings by descending
//! member count and reads no quotient at all, which separates a gain in the
//! graph from a gain in the buddy allocator's fragmentation.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

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
/// there is no version of the crate that holds both.
///
/// What keeps the copy honest is that **it reproduces**: over com-DBLP this
/// prints an extent of **902,220 × 902,343** at an occupancy of **0.018%**,
/// which is the baseline `/docs/design/position` states in the row that prices
/// the repair against it.
///
/// It used to cite *560,176* and *0.046%* instead, and that warrant was
/// inverted: those are the figures the page **retracted** — they came from a
/// simulation that was never in the tree, and the same paragraph that retracts
/// them puts the baseline extent at 902,343. A copy is kept true by what a run
/// of it prints, so a citation of a discarded number is evidence of nothing.
/// The 8.8× the discarded pair implied is 29.4× against what this actually
/// emits.
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

// ──────────────────────── ¿y cuánto cuesta el orden entre hermanos? ──
//
// `order_by_hierarchy` sorts a community on the path of ancestors above it and
// then on **its own id**, and it says so of itself: it buys locality *between*
// subtrees, not within one. Inside a parent the defect it was written to fix is
// intact — a community's rank among its siblings is still the order vertices
// happened to be visited in, which is `dense_id` order, which is IRI order,
// which has nothing to do with the graph.
//
// What follows prices that, over ONE dendrogram and ONE placement function, so
// every difference in the tables is the sibling rule and nothing else. Three
// rules, and the first of them is the one shipping today.

/// Which rule puts a community before its sibling.
///
/// Every one of them is a **total, deterministic** order over a sibling set:
/// byte-identity of the written corpus across processes is a property this repo
/// holds, and a sibling order that varies by run would end it. Nothing below
/// reads a hash map's iteration order, and every tie falls back to the id.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Siblings {
    /// **No hierarchy order at all** — the raw Louvain numbering, which is the
    /// order vertices happened to be visited in.
    ///
    /// Not a sibling rule and here as the control: it is the state
    /// `order_by_hierarchy` was written against, so the distance from this row
    /// to the next one is what fixing the TOP half bought, in the same units as
    /// the rows below it. Without it a two-tenths-of-a-point move has no scale
    /// to be small against.
    Flat,
    /// The id the community happens to wear — what `order_by_hierarchy` does
    /// today, reproduced here rather than called so that all three rows come
    /// off one instrument. `main` asserts it agrees with the real function.
    Id,
    /// **Descending member count**, ties by id — and it reads no quotient at
    /// all.
    ///
    /// The confound control, and the reason it is here rather than in a
    /// footnote: [`Siblings::Chain`] starts each family at its heaviest sibling,
    /// and heavy correlates with large. `cluster_layout` hands every group the
    /// smallest aligned square of the quaternary that holds it, so the ORDER OF
    /// SIZES inside a family decides the allocator's fragmentation on its own —
    /// which is a best-fit-decreasing effect and has nothing to do with the
    /// graph. If this row carries most of what the chain gains, then the gain is
    /// about packing and the edges never entered into it.
    Size,
    /// **Greedy nearest-neighbour chain.** Start at the sibling with the most
    /// weight inside the family, then repeatedly take the unplaced sibling with
    /// the most weight to everything placed so far.
    ///
    /// `O(E_f + n_f²)` per family, and the square is over a *family*, not a
    /// level: com-DBLP's branching is 6.0, 5.5, 2.5 and 1.2, so the term is
    /// noise. This is the one that is plausible in the pass — the quotient it
    /// needs is the one `hierarchy` already contracted and threw away.
    Chain,
    /// **Fiedler.** Order each family by the second eigenvector of its induced
    /// Laplacian, by power iteration on `sI − L` with the constant vector
    /// projected out.
    ///
    /// The upper bound, measured to say what a *good* ordering is worth against
    /// what a cheap one delivers. [`FIEDLER_ITERS`] passes over the family's
    /// edges rather than one, which is why it is here as a ceiling rather than
    /// as a proposal — though on this graph it turns out to cost the same
    /// order of milliseconds, because the families are tiny.
    Fiedler,
}

/// How many power iterations [`Siblings::Fiedler`] spends per component.
///
/// Fixed, not convergence-tested, and that is the determinism rule doing the
/// choosing: a tolerance makes the iteration count depend on floating-point
/// accumulation, and the whole claim of this section is that two orderings
/// differ by the RULE and not by the run.
const FIEDLER_ITERS: usize = 128;

/// How many tiles apart the two ends of an edge may fall and still be counted.
///
/// One tile is the share a single range read captures; two and four are what a
/// reader that is already paying for a small prefetch gets for it. The chunk
/// itself is `DEFAULT_CHUNK_SIZE` unless the third argument overrides it.
const WINDOWS: [u64; 3] = [1, 2, 4];

/// The start vector's stride — the golden ratio's fractional part, which gives
/// a low-discrepancy sequence with a non-zero projection on the eigenvector
/// being sought. A constant start has none at all, being the very vector the
/// iteration projects out.
const GOLDEN_FRACTION: f64 = 0.618_033_988_749_894_9;

/// **How much room a sibling rule has**, per tier.
///
/// A family of one or two has no order to get wrong: whatever weight two
/// siblings share is between them whichever way round they go. So the share of
/// a tier that sits in a family of three or more is the ceiling on everything
/// the rules below can move, and it is printed before them rather than inferred
/// from the tables afterwards.
fn family_room(tree: &Dendrogram) {
    let levels = tree.levels();
    let counts = tree.group_counts();
    println!(
        "  {:>5} {:>10} {:>10} {:>12} {:>14}",
        "tier", "nodos", "familias", "ramificación", "en familia ≥3"
    );
    for (t, &n) in counts.iter().enumerate() {
        let families = levels.get(t + 1).map_or(1, |_| counts[t + 1] as usize);
        let mut sizes = vec![0u32; families];
        for c in 0..n {
            sizes[levels.get(t + 1).map_or(0, |up| up[c as usize] as usize)] += 1;
        }
        let choosable: u32 = sizes.iter().filter(|&&s| s >= 3).sum();
        println!(
            "  {:>5} {n:>10} {families:>10} {:>11.2}x {:>13.1}%",
            t + 1,
            f64::from(n) / ratio(families.max(1)),
            100.0 * f64::from(choosable) / f64::from(n.max(1)),
        );
    }
}

/// The weighted quotient of one tier, as neighbour lists over that tier's own
/// community ids.
///
/// Recomputed from the edge list and the membership, for the reason the header
/// gives: the pass contracts this graph per level and frees it, so there is
/// nothing to reach for. Rows are sorted by neighbour id, so no walk below
/// depends on a hash map's order.
fn tier_quotient(n: u32, memb: &[u32], edges: &[(u32, u32)]) -> Vec<Vec<(u32, f64)>> {
    let mut acc: HashMap<(u32, u32), f64> = HashMap::new();
    for &(a, b) in edges {
        let (ca, cb) = (memb[a as usize], memb[b as usize]);
        if ca != cb {
            *acc.entry((ca.min(cb), ca.max(cb))).or_default() += 1.0;
        }
    }
    let mut rows: Vec<((u32, u32), f64)> = acc.into_iter().collect();
    rows.sort_unstable_by_key(|&((a, b), _)| (a, b));
    let mut adj = vec![Vec::new(); n as usize];
    for ((a, b), w) in rows {
        adj[a as usize].push((b, w));
        adj[b as usize].push((a, w));
    }
    for row in &mut adj {
        row.sort_unstable_by_key(|&(nb, _)| nb);
    }
    adj
}

/// One sibling set, ordered.
///
/// `group` is the family's own tier ids, ascending; `adj` is the whole tier's
/// quotient. Two siblings are the same picture either way round — whatever
/// weight they share is between them wherever they sit — so the rules only
/// start at three.
fn order_siblings(
    group: &[u32],
    adj: &[Vec<(u32, f64)>],
    sizes: &[u64],
    rule: Siblings,
) -> Vec<u32> {
    if matches!(rule, Siblings::Flat | Siblings::Id) || group.len() < 3 {
        return group.to_vec();
    }
    if rule == Siblings::Size {
        let mut out = group.to_vec();
        out.sort_unstable_by_key(|&c| (std::cmp::Reverse(sizes[c as usize]), c));
        return out;
    }
    let local: HashMap<u32, usize> = group.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let mut inner: Vec<Vec<(usize, f64)>> = vec![Vec::new(); group.len()];
    for (i, &c) in group.iter().enumerate() {
        for &(nb, w) in &adj[c as usize] {
            if let Some(&j) = local.get(&nb) {
                inner[i].push((j, w));
            }
        }
    }
    let order = match rule {
        Siblings::Flat | Siblings::Id | Siblings::Size => (0..group.len()).collect(),
        Siblings::Chain => chain_order(&inner),
        Siblings::Fiedler => fiedler_order(&inner),
    };
    order.into_iter().map(|i| group[i]).collect()
}

/// [`Siblings::Chain`] over one family's induced quotient, in local indices.
///
/// The affinity array is what keeps it linear-ish: a placed sibling pushes its
/// weight onto its unplaced neighbours once, and the next pick is the argmax of
/// that array. A family whose quotient is disconnected needs no special case —
/// every affinity is zero and the tie-break takes the heaviest, then the lowest
/// id, which is where the walk would have started anyway.
fn chain_order(adj: &[Vec<(usize, f64)>]) -> Vec<usize> {
    let n = adj.len();
    let degree: Vec<f64> = adj
        .iter()
        .map(|row| row.iter().map(|&(_, w)| w).sum())
        .collect();
    let mut placed = vec![false; n];
    let mut affinity = vec![0f64; n];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut best = usize::MAX;
        for i in 0..n {
            if placed[i] {
                continue;
            }
            let better = best == usize::MAX
                || match affinity[i].total_cmp(&affinity[best]) {
                    Ordering::Greater => true,
                    Ordering::Less => false,
                    // Equal affinity: the heavier sibling first, and equal
                    // weight leaves the lower id, which `best` already holds.
                    Ordering::Equal => degree[i].total_cmp(&degree[best]) == Ordering::Greater,
                };
            if better {
                best = i;
            }
        }
        placed[best] = true;
        out.push(best);
        for &(j, w) in &adj[best] {
            if !placed[j] {
                affinity[j] += w;
            }
        }
    }
    out
}

/// [`Siblings::Fiedler`] over one family, component by component.
///
/// Components are discovered from the lowest unvisited index upward and emitted
/// in that order, so a family whose quotient says nothing keeps exactly the
/// order it had: the rule is a **refinement** of the current one and never a
/// reshuffle for its own sake. Running the iteration across components instead
/// would converge on the null space, where the ordering inside a component is
/// whatever the arithmetic left there.
fn fiedler_order(adj: &[Vec<(usize, f64)>]) -> Vec<usize> {
    let n = adj.len();
    let mut seen = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for s in 0..n {
        if seen[s] {
            continue;
        }
        let mut stack = vec![s];
        seen[s] = true;
        let mut members = Vec::new();
        while let Some(v) = stack.pop() {
            members.push(v);
            for &(u, _) in &adj[v] {
                if !seen[u] {
                    seen[u] = true;
                    stack.push(u);
                }
            }
        }
        members.sort_unstable();
        out.extend(fiedler_component(adj, &members));
    }
    out
}

/// The Fiedler order of one connected component, by power iteration.
///
/// `sI − L` with `s = 2·max degree` is positive semidefinite with the same
/// eigenvectors as `L` and the order reversed, so the dominant direction *after
/// the constant vector is projected out at every step* is the one belonging to
/// λ₂ — the Fiedler vector. Projecting out at every step rather than once is
/// what stops rounding from feeding the λ₁ = 0 direction back in.
fn fiedler_component(adj: &[Vec<(usize, f64)>], members: &[usize]) -> Vec<usize> {
    if members.len() < 3 {
        return members.to_vec();
    }
    let index: HashMap<usize, usize> = members.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let n = members.len();
    let rows: Vec<Vec<(usize, f64)>> = members
        .iter()
        .map(|&v| {
            adj[v]
                .iter()
                .filter_map(|&(u, w)| index.get(&u).map(|&j| (j, w)))
                .collect()
        })
        .collect();
    let degree: Vec<f64> = rows
        .iter()
        .map(|row| row.iter().map(|&(_, w)| w).sum())
        .collect();
    let shift = 2.0 * degree.iter().copied().fold(0.0, f64::max);
    if shift <= 0.0 {
        return members.to_vec();
    }

    let mut x: Vec<f64> = (0..n)
        .map(|i| (ratio(i) * GOLDEN_FRACTION).fract() - 0.5)
        .collect();
    if !recentre(&mut x) {
        return members.to_vec();
    }
    let mut y = vec![0f64; n];
    for _ in 0..FIEDLER_ITERS {
        for i in 0..n {
            let mut neighbours = 0.0;
            for &(j, w) in &rows[i] {
                neighbours += w * x[j];
            }
            y[i] = (shift - degree[i]).mul_add(x[i], neighbours);
        }
        if !recentre(&mut y) {
            return members.to_vec();
        }
        x.copy_from_slice(&y);
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| x[a].total_cmp(&x[b]).then(a.cmp(&b)));
    order.into_iter().map(|i| members[i]).collect()
}

/// Project out the constant vector and normalise. `false` when there is nothing
/// left to normalise, which is the caller's signal to keep the order it had.
fn recentre(v: &mut [f64]) -> bool {
    let mean = v.iter().sum::<f64>() / ratio(v.len());
    for x in v.iter_mut() {
        *x -= mean;
    }
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm < 1e-12 {
        return false;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    true
}

/// The placement partition, renumbered under a stated sibling rule.
///
/// The key is the same depth-first walk `order_by_hierarchy` does, with one
/// substitution: every component is a **rank among siblings** instead of a raw
/// id. Under [`Siblings::Id`] the rank within a family is the id order within
/// that family, so the two keys are order-isomorphic and the result is
/// identical — which is the assertion `main` makes, and what licenses reading
/// the other two rows as a difference in the rule alone.
fn order_with(tree: &Dendrogram, edges: &[(u32, u32)], rule: Siblings) -> Vec<u32> {
    let levels = tree.levels();
    let depth = levels.len();
    let mut membership = tree
        .membership(0)
        .unwrap_or_else(|| (0..tree.vertex_count()).collect());
    if depth == 0 || rule == Siblings::Flat {
        return membership;
    }
    let counts = tree.group_counts();

    // A sibling rank per tier. Tier `t` is what `levels[0..=t]` composes to, and
    // `levels[t + 1]` is its parent column; the coarsest tier has no parent
    // column, so all of it is one family.
    let mut sibling_rank: Vec<Vec<u32>> = Vec::with_capacity(depth);
    for t in 0..depth {
        let n = counts[t];
        let memb = tree.membership(t).expect("a tier of this hierarchy");
        let adj = tier_quotient(n, &memb, edges);
        // Members per tier community, which only [`Siblings::Size`] reads. One
        // pass over the vertices, so carrying it costs the rules that ignore it
        // nothing worth a branch.
        let mut sizes = vec![0u64; n as usize];
        for &c in &memb {
            sizes[c as usize] += 1;
        }
        let families = levels.get(t + 1).map_or(1, |_| counts[t + 1] as usize);
        let mut groups: Vec<Vec<u32>> = vec![Vec::new(); families];
        for c in 0..n {
            let family = levels.get(t + 1).map_or(0, |up| up[c as usize] as usize);
            groups[family].push(c);
        }
        let mut rank = vec![0u32; n as usize];
        for group in &groups {
            for (r, c) in order_siblings(group, &adj, &sizes, rule)
                .into_iter()
                .enumerate()
            {
                rank[c as usize] = u32::try_from(r).expect("a family smaller than u32::MAX");
            }
        }
        sibling_rank.push(rank);
    }

    let leaves = counts[0];
    let mut keyed: Vec<(Vec<u32>, u32)> = (0..leaves)
        .map(|c| {
            let mut path = Vec::with_capacity(depth);
            path.push(sibling_rank[0][c as usize]);
            let mut current = c;
            for (t, level) in levels.iter().enumerate().skip(1) {
                current = level[current as usize];
                path.push(sibling_rank[t][current as usize]);
            }
            // Coarsest first, so the sort settles whole subtrees before it looks
            // at any finer distinction — the same shape of key, one rank deeper.
            path.reverse();
            (path, c)
        })
        .collect();
    keyed.sort_unstable();
    let mut rank = vec![0u32; leaves as usize];
    for (r, (_, c)) in keyed.iter().enumerate() {
        rank[*c as usize] = u32::try_from(r).expect("fewer communities than u32::MAX");
    }
    for m in &mut membership {
        *m = rank[*m as usize];
    }
    membership
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
/// **What one sibling order costs the id axis**, over the three questions the
/// camera actually asks.
///
/// Everything here is downstream of `placement` through the SAME
/// `cluster_layout` and the same Morton ranking the writer uses, so `dense_id`
/// below is the id a reader would compute, not a proxy for it.
fn sibling_report(label: &str, placement: &[u32], edges: &[(u32, u32)], chunk: u64) {
    let positions = cluster_layout(placement);
    let codes = morton_codes(&positions);
    let (rank, _) = morton_ranks(&codes);
    let vertex_count = positions.len() as u64;

    // 1. The span of every adjacency edge on the id axis, and — the number a
    //    range read feels — how much of the edge list never leaves one tile.
    let mut spans: Vec<u64> = Vec::with_capacity(edges.len());
    let mut inside = [0u64; WINDOWS.len()];
    for &(a, b) in edges {
        let (ra, rb) = (rank[a as usize], rank[b as usize]);
        spans.push(u64::from(ra.abs_diff(rb)));
        let gap = (u64::from(ra) / chunk).abs_diff(u64::from(rb) / chunk);
        for (slot, window) in inside.iter_mut().zip(WINDOWS) {
            if gap < window {
                *slot += 1;
            }
        }
    }
    spans.sort_unstable();
    let edge_count = ratio(spans.len().max(1));
    let mean = spans.iter().sum::<u64>() as f64 / edge_count;

    println!("\n── {label}");
    println!(
        "  salto en dense_id: p50 {:>8}  p90 {:>8}  p99 {:>8}  medio {mean:>10.0}",
        percentile(&spans, 500),
        percentile(&spans, 900),
        percentile(&spans, 990),
    );
    println!(
        "  aristas dentro de 1 tesela {:>6.2}%   de 2 {:>6.2}%   de 4 {:>6.2}%   (tesela = {chunk})",
        100.0 * inside[0] as f64 / edge_count,
        100.0 * inside[1] as f64 / edge_count,
        100.0 * inside[2] as f64 / edge_count,
    );

    // 2 and 3. Per rung: how full the quotient over that rung's cells is, and
    //    how far the cell areas spread. Both are cut on `dense_id >> shift`,
    //    exactly as the writer cuts them, and the área column reproduces
    //    `plane_report`'s for the ordering that ships.
    println!(
        "  {:>5} {:>9} {:>11} {:>10} {:>12} {:>13} {:>11}",
        "rung", "celdas", "q-aristas", "densidad", "área mín", "área máx", "máx/mín"
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
        let mut quotient: HashSet<(u32, u32)> = HashSet::new();
        for &(a, b) in edges {
            let (ca, cb) = (rank[a as usize] >> shift, rank[b as usize] >> shift);
            if ca != cb {
                quotient.insert((ca.min(cb), ca.max(cb)));
            }
        }
        // The same filter `plane_report` applies: a cell the numbering left
        // short carries a degenerate box, which is a fact about the tail and not
        // about the placement.
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
        let possible = cells.saturating_mul(cells.saturating_sub(1)) / 2;
        println!(
            "  {rung:>5} {cells:>9} {:>11} {:>9.2}% {lo:>12} {hi:>13} {:>10.0}x",
            quotient.len(),
            100.0 * ratio(quotient.len()) / ratio(possible.max(1) as usize),
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

    // El orden entre hermanos: la mitad del defecto que sigue en pie.
    println!("\nel orden entre hermanos, sobre la misma partición y la misma colocación");
    family_room(&tree);
    for (name, rule) in [
        (
            "sin jerarquía — el control, el defecto entero",
            Siblings::Flat,
        ),
        (
            "por id — lo que hace hoy `order_by_hierarchy`",
            Siblings::Id,
        ),
        ("por tamaño decreciente — sin leer el grafo", Siblings::Size),
        ("cadena voraz por peso de arista", Siblings::Chain),
        ("Fiedler del cociente de hermanos", Siblings::Fiedler),
    ] {
        let started = Instant::now();
        let ordered = order_with(&tree, &edges, rule);
        let elapsed = started.elapsed();
        if rule == Siblings::Id {
            // The instrument, checked against the thing it claims to reproduce.
            // If this ever fires, every row below it is a comparison against
            // something that is not what ships.
            assert_eq!(
                ordered, placement,
                "the id rule must reproduce order_by_hierarchy exactly"
            );
        }
        sibling_report(
            &format!("{name}  ({:.0} ms)", elapsed.as_secs_f64() * 1000.0),
            &ordered,
            &edges,
            chunk,
        );
    }

    if let Some(dir) = &out {
        println!("\nniveles volcados en {}", dir.display());
    }
}
