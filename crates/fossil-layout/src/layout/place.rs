//! Where a vertex goes on the plane, given which community it is in.
//!
//! Split out of `layout.rs` unchanged. [`cluster_layout`] is the deterministic
//! placement — every group an aligned square of the quaternary sized to its own
//! membership, handed out by bumping a Z-order frontier, members
//! phyllotaxis-packed inside — and [`place_after`] is what keeps two vertex
//! types from stacking.
//!
//! **Every position here is `derived`**, in the sense `/docs/design/position`
//! gives the word: the algorithm chose it because a picture needed coordinates,
//! and nothing measured it.

use super::morton::morton_decode;

/// Golden angle (radians) — the phyllotaxis constant `π(3−√5)`. Successive
/// nodes placed at multiples of this angle pack a disc evenly with no RNG.
const GOLDEN_ANGLE: f32 = 2.399_963_2;
/// The empty space around a group's packing disc, **as a fraction of that
/// disc's own radius**.
///
/// A ratio and not a constant, and that is a decision with a measurement behind
/// it. A margin added to a side that scales as `√m` is a margin that eats the
/// small end: over com-DBLP, a per-group cell keeping the 100-unit constant this
/// replaces spends 93% of the median group's cell on margin against 26% of the
/// largest group's — an 11× density difference across one partition, which is
/// the uniform-pitch defect in a smaller denomination. Spending the same
/// fraction at both ends is what leaves the plane at one density, and one
/// density is what makes an interval of `dense_id` an interval of area. See
/// `/docs/design/position`.
const CLUSTER_MARGIN_RATIO: f32 = 0.25;
/// **The side of the finest block: the cell a group of one member gets**, and
/// the unit every other cell is a power-of-two multiple of.
///
/// A single member packs into a disc of radius [`INTRA_CLUSTER_RADIUS`], so the
/// side is that diameter plus its margin. Everything else follows from it:
/// a group of `m` takes the smallest power of four of these that holds it, so
/// **one finest block holds one vertex** and the plane carries one vertex per
/// unit of area wherever it is sampled.
const CELL_UNIT: f32 = 2.0 * INTRA_CLUSTER_RADIUS * (1.0 + CLUSTER_MARGIN_RATIO);
/// How many clusters `cluster_id` may carry.
///
/// Not an aesthetic choice: a caller that draws the graph aggregates one
/// super-node per `(type_idx, cluster_id)`, and every read path out of
/// `fossil-graph` is row-capped — `ExecuteSqlParams::row_cap` defaults to
/// 10,000 and the executor applies an outer `LIMIT` whatever the SQL says. A
/// cap truncates, it does not degrade: a partition finer than the cap makes
/// the picture silently lose whole communities rather than coarsen. A budget
/// of 2,048 stays under 10,000 for up to four vertex types.
pub(super) const CLUSTER_BUDGET: u32 = 2_048;
/// Intra-cluster packing radius scale: the `k`-th member of a group sits at
/// `INTRA_CLUSTER_RADIUS · √(k+1)`, so a group of `m` fills a disc of area
/// proportional to `m` and every group is packed at the same density.
const INTRA_CLUSTER_RADIUS: f32 = 12.0;
/// Empty space between one vertex type's region and the next — four finest
/// blocks, so the seam between types reads as deliberate rather than as a gap
/// that happened.
const TYPE_GUTTER: f32 = CELL_UNIT * 4.0;

/// Deterministic 2-D positions from a per-vertex `cluster_id` list (as produced
/// by [`super::community::community_hierarchy`], `flatten_to_budget` and `order_by_hierarchy`).
/// **Every group gets a cell of its own size**: a group of `m` members is given
/// the smallest aligned square of the quaternary that holds `m` finest blocks,
/// and inside it the `k`-th vertex is placed at golden-angle phyllotaxis radius
/// `R·√(k+1)`. The mapping is a pure function of the input — no RNG, and no
/// table beyond the sizes this counts for itself.
///
/// # Why the cell is the group's own size and not a uniform pitch
///
/// The pitch used to be one number for every cell, `2·R·√largest` plus a
/// constant, and it was fixed by the biggest group in the partition. Over
/// com-DBLP that is a pitch of 3,534 on a partition whose median group holds
/// three members and packs into 41.6: the median disc covered **0.014% of its
/// own cell**, and the written plane came out **99.95% empty**.
///
/// The code here used to argue the uniform pitch, and the argument was that a
/// cluster's cell must be findable from its id alone. Nothing ever asked:
/// [`morton_decode`] has exactly one caller and it is this function, and a
/// reader locates a group by reading `x` and `y`, which it opens anyway. What
/// the argument was really protecting is that the placement is a function of
/// `cluster_ids` and of nothing else — and that survives untouched, because the
/// sizes a per-group cell needs are the ones already counted here.
///
/// # What the packing buys, and it is not the picture
///
/// `dense_id` is a vertex's **rank** in the Morton order of its position
/// ([`super::morton::morton_ranks`]), so a plane carrying one vertex per unit of
/// area makes the rank axis an area axis: an interval of `n` ids covers `n`
/// blocks of plane wherever on the plane it is taken. That is the whole
/// precondition of the cell pyramid — `/docs/design/cells` calls a cell row a
/// texel, and what is equal across a texel is area. Under the uniform pitch it
/// was not: sixteen consecutive ids inside com-DBLP's 20,459-member group are a
/// patch 85 units across, and sixteen out at the median group are five whole
/// grid cells, better than 7,000 across.
///
/// # Why the frontier never goes back
///
/// The allocation is a buddy allocator's — a block aligned to its own size — but
/// it refuses the free lists a buddy allocator normally keeps, and takes the
/// alignment fragmentation instead. `order_by_hierarchy` numbers these groups by
/// a depth-first walk of the dendrogram, so **a run of consecutive ids is a
/// subtree**; a frontier that only moves forward turns that into a run of
/// consecutive blocks, hence of consecutive Morton codes, hence a contiguous
/// interval of `dense_id`. Every `cluster_id` is therefore an interval of ids
/// too, being a union of consecutive groups — which is what stops a reader
/// colouring by it from colouring scattered packets. Reusing a hole behind the
/// frontier puts a later group in front of an earlier one and breaks exactly
/// that, for a plane that is being spent out of a 99.95% surplus.
#[must_use]
pub fn cluster_layout(cluster_ids: &[u32]) -> Vec<(f32, f32)> {
    let num_clusters = cluster_ids.iter().copied().max().map_or(0, |m| m + 1);
    if num_clusters == 0 {
        return Vec::new();
    }
    let mut sizes = vec![0u32; num_clusters as usize];
    for &c in cluster_ids {
        sizes[c as usize] += 1;
    }

    // **The square the groups are spread over**, and why they are spread rather
    // than packed against the origin.
    //
    // `morton_codes` quantises each axis over the extent the positions turned
    // out to have, INDEPENDENTLY — so a placement that fills a half of its root
    // hands the addressing a 2:1 plane, and the quaternary the corpus is
    // addressed on stops being made of squares. Packing the frontier tight does
    // exactly that: the first half of a Z-curve is the bottom half of its
    // square. Measured on a planted fixture of four thousand vertices, that is
    // an extent of 3,805 × 1,900 and a `dense_id` axis that no longer follows
    // the placement at all.
    //
    // So the frontier advances over the whole square instead: the root is the
    // smallest power of four that holds the blocks, and each group's position in
    // it is its own running total scaled to fit. The dilution is one factor
    // between one and four, the SAME factor for every group — which is what
    // leaves the density uniform, since a uniform scale is invisible to a
    // quantisation that normalises.
    let demand: u64 = sizes.iter().map(|&m| blocks_for(m)).sum();
    let root = next_power_of_four(demand);

    // The frontier, in finest blocks, walked in group-id order. A group takes
    // the smallest power of four that holds it, aligned to its own size — so its
    // block is one node of the quaternary and the ids inside it are one interval
    // — and `next` only ever moves forward.
    let mut centre = vec![(0.0f32, 0.0f32); num_clusters as usize];
    let mut next = 0u64;
    let mut asked = 0u64;
    for (c, &members) in sizes.iter().enumerate() {
        if members == 0 {
            // An id nothing carries takes no plane. `flatten_to_budget` densifies
            // and `order_by_hierarchy` is a permutation, so this is unreachable
            // through the pass; skipping is still the answer that keeps the
            // frontier a function of the sizes that exist.
            continue;
        }
        let blocks = blocks_for(members);
        let spread = if demand == 0 {
            0
        } else {
            asked * root / demand
        };
        asked += blocks;
        let start = next.max(spread).div_ceil(blocks) * blocks;
        next = start + blocks;

        let (col, row) = morton_decode(u32::try_from(start).unwrap_or(u32::MAX));
        // Half a side, which is where the phyllotaxis disc is centred: the disc
        // reaches `R·√m` and the half-side is `√blocks · R · (1 + margin)`, so it
        // fits with the margin to spare and does so at every size.
        let half = (blocks as f32).sqrt() * (CELL_UNIT / 2.0);
        centre[c] = (
            (col as f32).mul_add(CELL_UNIT, half),
            (row as f32).mul_add(CELL_UNIT, half),
        );
    }

    // Running per-cluster node counter for the intra-cluster phyllotaxis index.
    let mut seen = vec![0u32; num_clusters as usize];
    let mut out = Vec::with_capacity(cluster_ids.len());
    for &c in cluster_ids {
        let k = seen[c as usize];
        seen[c as usize] += 1;
        let angle = k as f32 * GOLDEN_ANGLE;
        let radius = INTRA_CLUSTER_RADIUS * ((k as f32) + 1.0).sqrt();
        let (cx, cy) = centre[c as usize];
        out.push((
            radius.mul_add(angle.cos(), cx),
            radius.mul_add(angle.sin(), cy),
        ));
    }
    out
}

/// How many finest blocks a group of `members` takes: the smallest power of
/// four that is at least `members`.
///
/// A power of **four** and not of two, because the block is a node of the
/// quaternary the whole corpus is addressed on — a square, aligned to its own
/// side, so that its members are one interval of Morton codes and therefore one
/// interval of `dense_id`.
fn blocks_for(members: u32) -> u64 {
    next_power_of_four(u64::from(members))
}

/// The smallest power of four that is at least `n`, and one for zero.
///
/// Searched rather than derived from a logarithm for
/// `crates/fossil-sinks/src/manifest.rs`'s reason: the answer has to be the same
/// integer every time it is asked, and `log2` of a `u64` in `f64` is not that
/// for every input.
const fn next_power_of_four(n: u64) -> u64 {
    let mut blocks = 1u64;
    while blocks < n {
        blocks *= 4;
    }
    blocks
}

// ──────────────────────────────────────────────────────────────────────────
// W3.1b — integration: apply the pure layout to the written GraphAr vertices.
// ──────────────────────────────────────────────────────────────────────────

/// Translate one vertex type's layout to start at `origin_x`, and answer where
/// the next type should start.
///
/// [`cluster_layout`] always begins at the origin, so laying several types out
/// independently puts every one of them in the same place. Read back by a camera
/// that is worse than ugly: a rectangle answers with vertices from unrelated
/// types that share nothing but coordinates, and the picture looks like a graph
/// rather than like a mistake.
///
/// The gap is [`TYPE_GUTTER`], wide enough that the seam reads as a seam. This
/// separates the types; it does not lay them out together — cross-type edges
/// still pull on nothing, and will not until a force-directed pass is seeded
/// from these positions. Separated is wrong in a way a reader can see and
/// reason about; overlapped is wrong in a way that looks like data.
pub(super) fn place_after(positions: &mut [(f32, f32)], origin_x: f32) -> f32 {
    let mut width = 0.0f32;
    for (x, _) in positions.iter_mut() {
        width = width.max(*x);
        *x += origin_x;
    }
    origin_x + width + TYPE_GUTTER
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    #[test]
    fn layout_positions_are_nonzero_and_deterministic() {
        let clusters = vec![0u32, 0, 1, 1];
        let p1 = cluster_layout(&clusters);
        let p2 = cluster_layout(&clusters);
        assert_eq!(p1, p2, "layout must be a pure deterministic function");
        // Not all at the origin (the W0b placeholder was 0,0 for every vertex).
        assert!(p1.iter().any(|&(x, y)| x != 0.0 || y != 0.0));
    }

    #[test]
    fn layout_groups_same_cluster_closer_than_cross_cluster() {
        // Two clusters of two nodes each: [c0, c0, c1, c1].
        let p = cluster_layout(&[0, 0, 1, 1]);
        let same = dist(p[0], p[1]); // both cluster 0
        let cross = dist(p[0], p[2]); // cluster 0 vs cluster 1
        assert!(
            same < cross,
            "same-cluster nodes ({same}) must be closer than cross-cluster ({cross})",
        );
    }

    #[test]
    fn layout_empty_input() {
        assert!(cluster_layout(&[]).is_empty());
    }

    /// The grid has to keep meaning something once clusters are the size real
    /// communities are. Under the fixed 100-unit pitch a cluster of 200 packed
    /// into a disc of radius `12·√200 ≈ 170` and reached two cells past its own,
    /// so vertices sat nearer a community they did not belong to. The single
    /// giant component the previous partition returned hid it — one cluster has
    /// no neighbour to overlap.
    ///
    /// Asserted against the clusters' own centroids rather than against the
    /// pitch, so the test states the property (a cluster is a place) instead of
    /// restating the arithmetic it is checking.
    #[test]
    fn layout_keeps_large_clusters_inside_their_own_cell() {
        const CLUSTERS: usize = 4;
        const PER: usize = 200;
        let ids: Vec<u32> = (0..CLUSTERS * PER).map(|i| (i % CLUSTERS) as u32).collect();
        let p = cluster_layout(&ids);

        let mut centroid = [(0.0f32, 0.0f32); CLUSTERS];
        for (i, &(x, y)) in p.iter().enumerate() {
            let c = ids[i] as usize;
            centroid[c].0 += x / PER as f32;
            centroid[c].1 += y / PER as f32;
        }

        for (i, &pos) in p.iter().enumerate() {
            let own = ids[i] as usize;
            let mine = dist(pos, centroid[own]);
            for (other, &c) in centroid.iter().enumerate() {
                if other != own {
                    assert!(
                        mine < dist(pos, c),
                        "vertex {i} of cluster {own} is nearer cluster {other}",
                    );
                }
            }
        }
    }

    /// **The defect the buddy allocation exists for**, as a property rather than
    /// as a number: one huge group must not decide how much plane a small one
    /// gets. Under the uniform pitch a partition of `1 + n·3` members put every
    /// group in a cell sized for the largest, so the whole picture inflated with
    /// the biggest community in it.
    ///
    /// Asserted on the area the placement fills, because that is what the pitch
    /// was spending: a run with one 4,000-member group among ninety-nine
    /// three-member ones must not cost very much more plane than the members
    /// themselves need.
    #[test]
    fn one_large_group_does_not_set_the_scale_for_the_small_ones() {
        let mut ids: Vec<u32> = (0..99).flat_map(|c| [c, c, c]).collect();
        ids.extend(std::iter::repeat_n(99u32, 4_000));
        let p = cluster_layout(&ids);

        let (mut xhi, mut yhi) = (f32::MIN, f32::MIN);
        for &(x, y) in &p {
            xhi = xhi.max(x);
            yhi = yhi.max(y);
        }
        // What the members themselves occupy: one finest block each.
        let needed = p.len() as f32 * CELL_UNIT * CELL_UNIT;
        let filled = xhi * yhi;
        assert!(
            filled < needed * 8.0,
            "the plane ({filled}) is more than eight times what its {} members need ({needed})",
            p.len(),
        );
    }

    /// Every group is one aligned square of the quaternary, and the squares are
    /// handed out along the Z-curve without going back. That is what makes a run
    /// of consecutive ids — which `order_by_hierarchy` arranges to be a subtree —
    /// a run of adjacent blocks rather than a scatter, and it is the half of the
    /// hidden-partition repair that lives in this function.
    ///
    /// Stated as the frontier's own property: each group's block starts at or
    /// after the end of the one before it, which is a claim about the allocation
    /// and not about any one corpus.
    #[test]
    fn the_frontier_hands_out_blocks_in_id_order_and_never_goes_back() {
        // Sizes chosen so the rounding to a power of four does real work: 1, 5
        // and 17 land just over 1, 4 and 16.
        let sizes = [1u32, 5, 3, 17, 2, 64, 1];
        let ids: Vec<u32> = sizes
            .iter()
            .enumerate()
            .flat_map(|(c, &n)| std::iter::repeat_n(c as u32, n as usize))
            .collect();
        let p = cluster_layout(&ids);

        // Each group's own box, from the members that are in it.
        let mut boxes = vec![(f32::MAX, f32::MAX, f32::MIN, f32::MIN); sizes.len()];
        for (i, &(x, y)) in p.iter().enumerate() {
            let b = &mut boxes[ids[i] as usize];
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        }

        for (c, &members) in sizes.iter().enumerate() {
            let (xlo, ylo, xhi, yhi) = boxes[c];
            let side = (blocks_for(members) as f32).sqrt() * CELL_UNIT;
            assert!(
                xhi - xlo <= side && yhi - ylo <= side,
                "group {c} of {members} spills its {side}-unit block: {}×{}",
                xhi - xlo,
                yhi - ylo,
            );
        }

        // And no two groups share plane: a block is one group's, whole.
        for a in 0..sizes.len() {
            for b in (a + 1)..sizes.len() {
                let (left, right) = (boxes[a], boxes[b]);
                let apart =
                    left.2 < right.0 || right.2 < left.0 || left.3 < right.1 || right.3 < left.1;
                assert!(apart, "groups {a} and {b} overlap on the plane");
            }
        }
    }

    /// The margin is a fraction of the disc rather than a constant, so the two
    /// ends of a partition are packed at the same density. With a constant, a
    /// three-member group spent nine tenths of its cell on margin while a
    /// twenty-thousand-member one spent a quarter — and a plane of two densities
    /// is a `dense_id` axis that is not an area axis.
    ///
    /// Measured on the plane the placement actually fills, at two group sizes two
    /// orders of magnitude apart, because the claim is about what comes out and
    /// not about the constant that goes in.
    #[test]
    fn the_small_groups_and_the_large_ones_are_packed_at_one_density() {
        let plane_per_member = |members: u32| {
            let ids: Vec<u32> = (0..64u32)
                .flat_map(|c| std::iter::repeat_n(c, members as usize))
                .collect();
            let p = cluster_layout(&ids);
            let (mut xhi, mut yhi) = (f32::MIN, f32::MIN);
            for &(x, y) in &p {
                xhi = xhi.max(x);
                yhi = yhi.max(y);
            }
            xhi * yhi / p.len() as f32
        };
        // Inside one factor of four, which is the power-of-four rounding and
        // nothing else: three members round up to four blocks and three hundred
        // to one thousand and twenty-four.
        let (small, large) = (plane_per_member(3), plane_per_member(300));
        assert!(
            small * 4.0 > large && large * 4.0 > small,
            "the two ends differ by more than the rounding: {small} against {large} per member",
        );
    }

    #[test]
    fn place_after_separates_types_instead_of_stacking_them() {
        // Two types laid out independently both start at the origin, which is
        // how a two-type graph rendered as one blob with its communities
        // interleaved at random — and a bbox query answered with vertices that
        // share nothing but a coordinate.
        let mut first = cluster_layout(&[0, 0, 1, 1]);
        let mut second = cluster_layout(&[0, 0, 1, 1]);
        assert_eq!(first, second, "independently, the two types coincide");

        let next = place_after(&mut first, 0.0);
        place_after(&mut second, next);

        let first_right = first.iter().fold(f32::MIN, |m, &(x, _)| m.max(x));
        let second_left = second.iter().fold(f32::MAX, |m, &(x, _)| m.min(x));
        assert!(
            second_left > first_right,
            "the second type starts ({second_left}) clear of the first ({first_right})",
        );
    }

    #[test]
    fn place_after_leaves_a_single_type_where_it_was() {
        // The common case is one vertex type, and it must not be pushed off the
        // origin by the machinery that exists for the several-type case.
        let mut only = cluster_layout(&[0, 0, 1]);
        let untouched = only.clone();
        place_after(&mut only, 0.0);
        assert_eq!(only, untouched);
    }
}
