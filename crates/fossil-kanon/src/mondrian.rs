//! The strict partitioner, and the publication of what it produces.
//!
//! # Strict, and the relaxed variant is not offered
//!
//! Mondrian cuts a dimension at the median. The trap is what happens when the median value itself
//! has many duplicates: every record holding it must fall on one side of the cut, so the two sides
//! come out unbalanced, and the relaxed variant fixes that by splitting the *tied* records evenly
//! across both sides. Doing so makes the two partitions overlap on that dimension — and a record on
//! the left whose value equals the split value is now published inside a span that also contains
//! records on the right holding the same value in a different class. The equivalence classes are no
//! longer determined by the published values, and classes smaller than k come out of it. Quietly:
//! nothing in the algorithm notices, the row counts look balanced, and the output is a table that
//! reports k and does not have it.
//!
//! Strict refuses the cut instead. A cut is performed only when **every** side of it already has k
//! members, and a dimension whose cut fails is struck off for that partition and its descendants
//! are tried on the others. `partition.allow[dim] = 0` in the reference implementation; `allow` in
//! [`search`] here.
//!
//! There is no `relaxed` flag, no feature and no constructor for it. A weaker guarantee reachable
//! by a parameter is a weaker guarantee reachable by a typo, and the guarantee is the crate.
//!
//! # The consequence: strict Mondrian barely suppresses
//!
//! Worth stating because it surprises people who arrive from Datafly. The root partition holds
//! every row, a cut happens only when both sides already satisfy k, and induction from there says
//! every leaf satisfies k. So suppression is not how strict Mondrian reaches k — it reaches k by
//! declining to cut. The only rows it withholds are the ones a [`crate::NullPolicy`] withheld
//! before the search started, and the whole table when the table is smaller than k.
//!
//! [`crate::Report::suppressed_by_safety_net`] is the third source, and it should always read zero.

use std::collections::BTreeMap;

use crate::ingest::{Dim, Values};
use crate::verify::Cell;

/// Where one partition sits on one dimension.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DimState {
    /// A levelled hierarchy, at this level. `0` is the implicit top.
    Level(usize),
    /// A numeric dimension, holding values in `[lo, hi]`. Used to normalise this dimension's width
    /// against the others when choosing where to cut; the PUBLISHED span is recomputed from the
    /// partition's own members and is never read from here.
    Range { lo: f64, hi: f64 },
}

#[derive(Debug, Clone)]
pub(crate) struct Partition {
    pub(crate) rows: Vec<u32>,
    pub(crate) state: Vec<DimState>,
}

/// Mondrian's recursion, as an explicit stack.
///
/// `live` is the rows still in play — [`crate::NullPolicy::Suppress`] has already removed what it
/// removes, so every row here will be published or withheld by the safety net and by nothing else.
pub(crate) fn search(dims: &[Dim], live: Vec<u32>, k: usize) -> Vec<Partition> {
    let root = Partition {
        state: dims
            .iter()
            .map(|d| match &d.values {
                Values::Numeric { global, .. } => {
                    let (lo, hi) = global.unwrap_or((0.0, 0.0));
                    DimState::Range { lo, hi }
                }
                Values::Levelled { .. } => DimState::Level(0),
            })
            .collect(),
        rows: live,
    };

    let mut finals = Vec::new();
    let mut stack = vec![root];
    while let Some(part) = stack.pop() {
        // `allow` is per visit and starts full, as in the reference implementation. A dimension
        // that failed in the parent cannot succeed in a child — the child's groups are subsets of
        // the parent's, so a side that was under k is still under k — so resetting it only costs a
        // recheck. It never costs a cut.
        let mut allow = vec![true; dims.len()];
        loop {
            let Some(d) = choose(dims, &part, &allow) else {
                finals.push(part);
                break;
            };
            if let Some(children) = try_split(dims, &part, d, k) {
                stack.extend(children);
                break;
            }
            allow[d] = false;
        }
    }
    finals
}

/// Mondrian's heuristic: cut the dimension that is currently widest, normalised against its own
/// global extent so that a column of ages and a column of postcodes are comparable.
///
/// Ties go to the lowest index. Not arbitrary — a `f64` comparison that resolved ties by hash order
/// would make the output depend on allocation addresses, and this crate's outputs are snapshot
/// tested and diffed between releases of the same table.
fn choose(dims: &[Dim], part: &Partition, allow: &[bool]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (d, dim) in dims.iter().enumerate() {
        if !allow[d] {
            continue;
        }
        let w = width(dim, part, d);
        if w > 0.0 && best.is_none_or(|(_, bw)| w > bw) {
            best = Some((d, w));
        }
    }
    best.map(|(d, _)| d)
}

fn width(dim: &Dim, part: &Partition, d: usize) -> f64 {
    match (&dim.values, &part.state[d]) {
        (Values::Numeric { global, .. }, DimState::Range { lo, hi }) => {
            match global {
                Some((gmin, gmax)) if gmax > gmin => (hi - lo) / (gmax - gmin),
                // One value in the whole column, or none: nothing to cut, and a width of zero is
                // what says so.
                _ => 0.0,
            }
        }
        (
            Values::Levelled {
                levels,
                leaf_distinct,
            },
            DimState::Level(l),
        ) => {
            if *l >= levels.len() || *leaf_distinct == 0 {
                return 0.0;
            }
            // The categorical analogue of a span: how much of the leaf domain this partition still
            // covers. The reference implementation counts the leaves under the current
            // generalisation node; counting the distinct leaf values actually present is the same
            // quantity for the hierarchies here and needs no tree.
            let leaf = &levels[levels.len() - 1];
            let mut seen = std::collections::HashSet::new();
            for &r in &part.rows {
                if let Some(v) = leaf[r as usize].as_ref() {
                    seen.insert(v);
                }
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "counts of distinct values in a table that fits in memory; the ratio is a heuristic weight, not a measurement"
            )]
            let w = seen.len() as f64 / *leaf_distinct as f64;
            w
        }
        // The two shapes are chosen together in `search` and cannot disagree.
        _ => 0.0,
    }
}

/// Attempt one cut. `None` means «not allowable» — the dimension is struck off, not the partition.
fn try_split(dims: &[Dim], part: &Partition, d: usize, k: usize) -> Option<Vec<Partition>> {
    match &dims[d].values {
        Values::Numeric { values, .. } => split_numeric(part, d, values, k),
        Values::Levelled { levels, .. } => split_levelled(part, d, levels, k),
    }
}

/// The median cut, strict.
///
/// A record whose value on this dimension is missing does not constrain the cut and is not counted
/// into the median: it is compatible with both sides, which is exactly what
/// [`crate::NullPolicy::Wildcard`] means. It is then counted into the k test of BOTH sides, because
/// it will be published with a wildcard on this dimension and is therefore indistinguishable from
/// the members of either. Where it physically lands is cosmetic — the smaller side, to keep the
/// tree balanced.
fn split_numeric(
    part: &Partition,
    d: usize,
    values: &[Option<f64>],
    k: usize,
) -> Option<Vec<Partition>> {
    let mut reals: Vec<(u32, f64)> = Vec::new();
    let mut nulls: Vec<u32> = Vec::new();
    for &r in &part.rows {
        match values[r as usize] {
            Some(v) => reals.push((r, v)),
            None => nulls.push(r),
        }
    }
    if reals.len() < 2 {
        return None;
    }
    reals.sort_by(|a, b| a.1.partial_cmp(&b.1).expect("ingest refuses NaN"));

    // The reference walks the sorted distinct values accumulating their frequencies and takes the
    // first value whose running total reaches the halfway mark. Doing it over duplicates rather
    // than over positions is what makes the cut land ON a value rather than between two copies of
    // one — which is the whole subject of the strict/relaxed distinction above.
    let middle = reals.len() / 2;
    let mut acc = 0usize;
    let mut split_val = reals[0].1;
    let mut i = 0;
    while i < reals.len() {
        let v = reals[i].1;
        let mut j = i;
        while j < reals.len() && reals[j].1 == v {
            j += 1;
        }
        acc += j - i;
        if acc >= middle {
            split_val = v;
            break;
        }
        i = j;
    }
    // Everything is on one side: the split value is the maximum, so there is nothing above it to
    // open a right-hand partition with and no cut to make. The reference calls this
    // `split_val == next_val` and strikes the dimension off.
    if !reals.iter().any(|&(_, v)| v > split_val) {
        return None;
    }

    let (left, right): (Vec<(u32, f64)>, Vec<(u32, f64)>) =
        reals.iter().partition(|&&(_, v)| v <= split_val);

    // Strict: BOTH sides must already satisfy k, counting the wildcards that will be compatible
    // with both. If either does not, the cut does not happen.
    if left.len() + nulls.len() < k || right.len() + nulls.len() < k {
        return None;
    }

    let span = |side: &[(u32, f64)]| {
        let lo = side.iter().map(|&(_, v)| v).fold(f64::INFINITY, f64::min);
        let hi = side
            .iter()
            .map(|&(_, v)| v)
            .fold(f64::NEG_INFINITY, f64::max);
        DimState::Range { lo, hi }
    };
    let (mut lrows, mut rrows): (Vec<u32>, Vec<u32>) = (
        left.iter().map(|&(r, _)| r).collect(),
        right.iter().map(|&(r, _)| r).collect(),
    );
    let (lstate, rstate) = (span(&left), span(&right));
    if lrows.len() <= rrows.len() {
        lrows.extend(nulls);
    } else {
        rrows.extend(nulls);
    }
    lrows.sort_unstable();
    rrows.sort_unstable();

    let child = |rows, state| {
        let mut s = part.state.clone();
        s[d] = state;
        Partition { rows, state: s }
    };
    Some(vec![child(lrows, lstate), child(rrows, rstate)])
}

/// Refine a levelled dimension by one rung, strict.
///
/// This is the hierarchy-aware split, and it is n-ary rather than binary: the children of a
/// generalisation node are its children, not two halves of an alphabet. A lexicographic median cut
/// over postcode strings would put `SW1A` and `SW1B` on opposite sides of a cut that also separates
/// `N1` from `NW1` — a partition whose published value would have to be an interval of strings, and
/// an interval of strings is not a postcode district. The published value here is always a node of
/// the declared hierarchy.
///
/// A refinement that yields a single child is still performed. It separates nothing, and it
/// publishes a strictly more informative value for the same class at no cost in k.
fn split_levelled(
    part: &Partition,
    d: usize,
    levels: &[Vec<Option<String>>],
    k: usize,
) -> Option<Vec<Partition>> {
    let DimState::Level(l) = part.state[d] else {
        return None;
    };
    if l >= levels.len() {
        return None;
    }
    // BTreeMap, not HashMap: the child order reaches the output, and an output that reorders
    // itself between two runs over the same table cannot be diffed or snapshot tested.
    let mut groups: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    let mut nulls: Vec<u32> = Vec::new();
    for &r in &part.rows {
        match levels[l][r as usize].as_deref() {
            Some(v) => groups.entry(v).or_default().push(r),
            None => nulls.push(r),
        }
    }
    if groups.is_empty() {
        return None;
    }
    if groups.values().any(|g| g.len() + nulls.len() < k) {
        return None;
    }

    let mut children: Vec<Vec<u32>> = groups.into_values().collect();
    // The wildcards are compatible with every child, so which one holds them changes no guarantee.
    // The smallest, for balance, and deterministically the earliest on a tie.
    let smallest = children
        .iter()
        .enumerate()
        .min_by_key(|(i, g)| (g.len(), *i))
        .map(|(i, _)| i)
        .expect("non-empty by the check above");
    children[smallest].extend(nulls);

    Some(
        children
            .into_iter()
            .map(|mut rows| {
                rows.sort_unstable();
                let mut s = part.state.clone();
                s[d] = DimState::Level(l + 1);
                Partition { rows, state: s }
            })
            .collect(),
    )
}

/// What one partition publishes on one dimension, and what it costs.
pub(crate) struct Published {
    /// The class's value. Every non-null row of the partition publishes this.
    pub(crate) cell: Cell,
    /// `max - min` over the partition's own members on a numeric dimension — how much resolution
    /// the class lost. `None` for a levelled dimension, which reports a level instead.
    pub(crate) span: Option<f64>,
    /// A value that fell outside the declared buckets, if one did.
    pub(crate) uncovered: Option<f64>,
}

/// Render one partition's value on dimension `d`.
pub(crate) fn publish(dim: &Dim, part: &Partition, d: usize) -> Published {
    match &dim.values {
        Values::Numeric {
            values,
            buckets,
            presentation,
            ..
        } => {
            let observed: Vec<f64> = part
                .rows
                .iter()
                .filter_map(|&r| values[r as usize])
                .collect();
            let (Some(&min), Some(&max)) = (
                observed
                    .iter()
                    .min_by(|a, b| a.partial_cmp(b).expect("ingest refuses NaN")),
                observed
                    .iter()
                    .max_by(|a, b| a.partial_cmp(b).expect("ingest refuses NaN")),
            ) else {
                // Every member is missing on this dimension. The class knows nothing about it, and
                // `*` is the only honest thing to publish.
                return Published {
                    cell: Cell::Wildcard,
                    span: None,
                    uncovered: None,
                };
            };
            let span = Some(max - min);
            match presentation {
                crate::hierarchy::NumericPresentation::ObservedRange => Published {
                    cell: Cell::Value(if min == max {
                        fmt(min)
                    } else {
                        format!("[{}, {}]", fmt(min), fmt(max))
                    }),
                    span,
                    uncovered: None,
                },
                crate::hierarchy::NumericPresentation::EnclosingBucket => {
                    let lo = buckets.iter().copied().filter(|&e| e <= min).next_back();
                    let hi = buckets.iter().copied().find(|&e| e > max);
                    let cell = match (lo, hi) {
                        (Some(a), Some(b)) => Cell::Value(format!("[{}, {})", fmt(a), fmt(b))),
                        (None, Some(b)) => Cell::Value(format!("(-inf, {})", fmt(b))),
                        (Some(a), None) => Cell::Value(format!("[{}, inf)", fmt(a))),
                        (None, None) => Cell::Wildcard,
                    };
                    Published {
                        cell,
                        span,
                        uncovered: match (lo, hi) {
                            (None, _) => Some(min),
                            (_, None) => Some(max),
                            _ => None,
                        },
                    }
                }
            }
        }
        Values::Levelled { levels, .. } => {
            let DimState::Level(l) = part.state[d] else {
                unreachable!("a levelled dimension holds a level")
            };
            // Level 0 is the implicit top. Above it, every non-null member of the partition shares
            // one value by construction — that is what a levelled split produced — so the first one
            // found is the class's value.
            let cell = if l == 0 {
                Cell::Wildcard
            } else {
                part.rows
                    .iter()
                    .find_map(|&r| levels[l - 1][r as usize].clone())
                    .map_or(Cell::Wildcard, Cell::Value)
            };
            Published {
                cell,
                span: None,
                uncovered: None,
            }
        }
    }
}

/// Format a float the way a published class value should read: `35` and not `35.0`, because a
/// bucket edge declared as `35` in a JSON hierarchy file is an integer to everyone who reads it.
pub(crate) fn fmt(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "guarded one line above: integral and within i64"
        )]
        return format!("{}", v as i64);
    }
    format!("{v}")
}
