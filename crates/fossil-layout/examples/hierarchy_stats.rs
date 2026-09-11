//! **Is there a level of the community hierarchy worth drawing?**
//!
//! The write path finds a whole hierarchy and keeps two things out of it: the
//! FINEST level, which becomes the placement, and ONE level chosen by
//! `flatten_to_budget`, which becomes `cluster_id`. Every level between them,
//! and the quotient graph each was found on, is freed.
//!
//! That is what makes the question unanswerable from outside: on com-DBLP the
//! finest level is 98,505 communities of mean size 3.2 -- not a structure, very
//! nearly the graph itself -- and the coarsest is 1,229 whose quotient has a
//! mean degree of 80, which is dense enough that a force layout over it is a
//! hairball. Neither is a level a picture can be built on, and whether a middle
//! one exists is a property of the DATA that no amount of reasoning settles.
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
//! this again.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use fossil_layout::layout::community_hierarchy;

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
/// "communities of size <= 3" from 83.1% to 99.7% and stalled the coarsening
/// dead, which would have read as "this graph has no hierarchy" when what it
/// had was a counting error.
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

/// Fold a membership at level `l` down to one label per ORIGINAL vertex.
///
/// The levels compose by lookup rather than recomputation — level `l` is indexed
/// by level `l-1`'s community ids — which is the same walk `order_by_hierarchy`
/// does and the reason the hierarchy is cheap to carry.
fn flatten(levels: &[Vec<u32>], upto: usize, vertex_count: u32) -> Vec<u32> {
    let mut label: Vec<u32> = (0..vertex_count).collect();
    for level in levels.iter().take(upto + 1) {
        for l in &mut label {
            *l = level[*l as usize];
        }
    }
    label
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

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "apps/playground/bench/dblp/data/links.csv".to_string());
    let out: Option<PathBuf> = args.next().map(PathBuf::from);

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

    let levels = community_hierarchy(vertex_count, &edges);
    println!("la jerarquía tiene {} niveles\n", levels.len());

    println!(
        "{:>5} {:>10} {:>8} {:>8} {:>8} {:>8} {:>11} {:>9} {:>12}",
        "nivel", "comunid.", "p50", "p90", "máx", "%<=3", "modularidad", "q-aristas", "q-grado med"
    );

    for l in 0..levels.len() {
        let label = flatten(&levels, l, vertex_count);
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

    if let Some(dir) = &out {
        println!("\nniveles volcados en {}", dir.display());
    }
}
