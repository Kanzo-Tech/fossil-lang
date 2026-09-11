//! Where a vertex goes on the plane, given which community it is in.
//!
//! Split out of `layout.rs` unchanged. [`cluster_layout`] is the deterministic
//! placement — clusters on a Z-ordered grid, members phyllotaxis-packed inside
//! their cell — and [`place_after`] is what keeps two vertex types from stacking.
//!
//! **Every position here is `derived`**, in the sense `/docs/design/position`
//! gives the word: the algorithm chose it because a picture needed coordinates,
//! and nothing measured it.

use super::morton::morton_decode;

/// Golden angle (radians) — the phyllotaxis constant `π(3−√5)`. Successive
/// nodes placed at multiples of this angle pack a disc evenly with no RNG.
const GOLDEN_ANGLE: f32 = 2.399_963_2;
/// Empty space left between one cluster's packing disc and the next. The grid
/// pitch is this plus the largest disc's diameter, so it is a margin and not the
/// pitch itself — see [`cluster_layout`].
const CLUSTER_SPACING: f32 = 100.0;
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
/// Intra-cluster packing radius scale (kept well below `CLUSTER_SPACING` so
/// same-cluster nodes stay closer to each other than to other clusters).
const INTRA_CLUSTER_RADIUS: f32 = 12.0;
/// Empty space between one vertex type's region and the next — two cluster
/// cells, so the seam between types reads as deliberate rather than as a gap
/// that happened.
const TYPE_GUTTER: f32 = CLUSTER_SPACING * 2.0;

/// Deterministic 2-D positions from a per-vertex `cluster_id` list (as produced
/// by [`super::community::community_hierarchy`], `flatten_to_budget` and `order_by_hierarchy`).
/// Clusters occupy the cells of a Z-order grid; within a cell, the `k`-th vertex
/// is placed at golden-angle phyllotaxis radius `R·√k`. Same-cluster vertices
/// cluster visually; the mapping is a pure function of the input (stable across
/// runs — no RNG).
///
/// The grid is walked in Z-order and not row by row because the cluster ids
/// arriving here are a depth-first numbering of the hierarchy, so a run of
/// consecutive ids is a family. Row-major would smear that family along a row
/// and break it at the wrap; Z-order keeps it in a block. A count that is not a
/// power of four leaves the tail of the curve empty, which shows as a bite out
/// of one corner of the picture — a gap where there is no data, rather than a
/// crowding where there is.
///
/// # Why the grid pitch is measured and not a constant
///
/// A cluster of `n` vertices packs into a disc of radius `R·√n`, which passes
/// 100 units at 70 vertices. A fixed 100-unit pitch is therefore only correct
/// while every cluster is tiny, and the discs of anything larger overlap their
/// neighbours until the grid means nothing. That defect was invisible under the
/// weakly-connected-components partition for the reason that partition was
/// replaced: it returned one giant component, and a single cluster has no
/// neighbour to overlap. Real communities put several hundred vertices in every
/// cell at once, so the pitch is now the largest disc's diameter plus
/// `CLUSTER_SPACING` as the gap between them.
///
/// The pitch is uniform rather than per-cluster because a cluster's *cell* must
/// be findable from its id alone — that is what makes the placement a pure
/// function of `cluster_ids` and reproducible without carrying a table.
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
    let largest = sizes.iter().copied().max().unwrap_or(0);
    let pitch = 2.0f32.mul_add(
        INTRA_CLUSTER_RADIUS * (largest as f32).sqrt(),
        CLUSTER_SPACING,
    );

    // Running per-cluster node counter for the intra-cluster phyllotaxis index.
    let mut seen = vec![0u32; num_clusters as usize];
    let mut out = Vec::with_capacity(cluster_ids.len());
    for &c in cluster_ids {
        let (col, row) = morton_decode(c);
        let cell_x = col as f32 * pitch;
        let cell_y = row as f32 * pitch;
        let k = seen[c as usize];
        seen[c as usize] += 1;
        let angle = k as f32 * GOLDEN_ANGLE;
        let radius = INTRA_CLUSTER_RADIUS * ((k as f32) + 1.0).sqrt();
        out.push((cell_x + radius * angle.cos(), cell_y + radius * angle.sin()));
    }
    out
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
