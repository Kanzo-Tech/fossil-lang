//! Graph layout precompute (W3.1) — pure, host-agnostic algorithms.
//!
//! Per `.planning/W3-LAYOUT-PLAN.md`: the W0b writer emits `x`/`y`/`cluster_id`
//! as placeholders (`0`); W3 fills them so the viewport verb returns meaningful
//! positions and the Parquet can be morton-sorted for predicate pushdown.
//!
//! This module is the **algorithmic core** — deliberately decoupled from the
//! `materialize` hot path (which reads the edge set + rewrites the vertex
//! Parquet, a separate slice). Two pure functions over a `dense_id` edge list:
//!
//! - [`weakly_connected_components`] — the coarsest legitimate partition (WCC,
//!   union-find). Not a placeholder: it is the correct multi-component grouping
//!   and a real graph property. Leiden communities (modularity) refine it in a
//!   later slice; the two answer different questions (reachability vs density).
//! - [`cluster_layout`] — a deterministic community-grouped placement: clusters
//!   on a grid, nodes phyllotaxis-packed within their cell. Same-cluster nodes
//!   land near each other. ForceAtlas2 refinement is a later slice; this gives
//!   the viewport real, stable coordinates without an iterative force sim.
//!
//! Both are pure (no DuckDB, no I/O, no RNG) so they unit-test in isolation and
//! the `materialize` integration can wire them with confidence.

/// Golden angle (radians) — the phyllotaxis constant `π(3−√5)`. Successive
/// nodes placed at multiples of this angle pack a disc evenly with no RNG.
const GOLDEN_ANGLE: f32 = 2.399_963_2;
/// Distance between adjacent cluster cells on the grid.
const CLUSTER_SPACING: f32 = 100.0;
/// Intra-cluster packing radius scale (kept well below [`CLUSTER_SPACING`] so
/// same-cluster nodes stay closer to each other than to other clusters).
const INTRA_CLUSTER_RADIUS: f32 = 12.0;

/// Assign each vertex `0..vertex_count` a dense, contiguous `cluster_id` via
/// weakly-connected components (union-find with path halving).
///
/// `edges` are `(src_dense, dst_dense)` pairs; direction is ignored (weak
/// connectivity). Out-of-range endpoints are skipped (defensive — a clean
/// writer never emits them). Cluster ids are relabelled to `0..k` in order of
/// first appearance by ascending `dense_id`, so the numbering is stable and
/// gap-free regardless of union order.
#[must_use]
pub fn weakly_connected_components(vertex_count: u32, edges: &[(u32, u32)]) -> Vec<u32> {
    let n = vertex_count as usize;
    let mut parent: Vec<u32> = (0..vertex_count).collect();

    for &(a, b) in edges {
        if (a as usize) < n && (b as usize) < n {
            let ra = find(&mut parent, a);
            let rb = find(&mut parent, b);
            if ra != rb {
                parent[ra as usize] = rb;
            }
        }
    }

    // Relabel roots to dense 0..k in first-appearance order.
    let mut label: Vec<Option<u32>> = vec![None; n];
    let mut next = 0u32;
    let mut out = vec![0u32; n];
    for i in 0..n {
        let root = find(&mut parent, i as u32);
        let id = match label[root as usize] {
            Some(id) => id,
            None => {
                let id = next;
                label[root as usize] = Some(id);
                next += 1;
                id
            }
        };
        out[i] = id;
    }
    out
}

/// Union-find root with path halving (iterative — no recursion, no stack risk
/// on million-node chains).
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let grand = parent[parent[x as usize] as usize];
        parent[x as usize] = grand;
        x = grand;
    }
    x
}

/// Deterministic 2-D positions from a per-vertex `cluster_id` list (as produced
/// by [`weakly_connected_components`]). Clusters occupy a near-square grid of
/// cells; within a cell, the `k`-th vertex is placed at golden-angle
/// phyllotaxis radius `R·√k`. Same-cluster vertices cluster visually; the
/// mapping is a pure function of the input (stable across runs — no RNG).
#[must_use]
pub fn cluster_layout(cluster_ids: &[u32]) -> Vec<(f32, f32)> {
    let num_clusters = cluster_ids.iter().copied().max().map_or(0, |m| m + 1);
    if num_clusters == 0 {
        return Vec::new();
    }
    // Near-square grid: ceil(sqrt(k)) columns.
    let cols = (f64::from(num_clusters).sqrt().ceil() as u32).max(1);

    // Running per-cluster node counter for the intra-cluster phyllotaxis index.
    let mut seen = vec![0u32; num_clusters as usize];
    let mut out = Vec::with_capacity(cluster_ids.len());
    for &c in cluster_ids {
        let cell_x = (c % cols) as f32 * CLUSTER_SPACING;
        let cell_y = (c / cols) as f32 * CLUSTER_SPACING;
        let k = seen[c as usize];
        seen[c as usize] += 1;
        let angle = k as f32 * GOLDEN_ANGLE;
        let radius = INTRA_CLUSTER_RADIUS * ((k as f32) + 1.0).sqrt();
        out.push((cell_x + radius * angle.cos(), cell_y + radius * angle.sin()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn wcc_two_disjoint_edges_two_clusters() {
        // 0—1 and 2—3 → {0,1}=0, {2,3}=1.
        let c = weakly_connected_components(4, &[(0, 1), (2, 3)]);
        assert_eq!(c, vec![0, 0, 1, 1]);
    }

    #[test]
    fn wcc_chain_is_one_cluster() {
        let c = weakly_connected_components(3, &[(0, 1), (1, 2)]);
        assert_eq!(c, vec![0, 0, 0]);
    }

    #[test]
    fn wcc_no_edges_each_isolated() {
        let c = weakly_connected_components(3, &[]);
        assert_eq!(c, vec![0, 1, 2]);
    }

    #[test]
    fn wcc_labels_are_dense_and_union_order_independent() {
        // Union in a different order must yield the same dense labelling.
        let a = weakly_connected_components(5, &[(3, 4), (0, 1)]);
        let b = weakly_connected_components(5, &[(0, 1), (3, 4)]);
        assert_eq!(a, b);
        // dense + gap-free: {0,1}=0, {2}=1, {3,4}=2.
        assert_eq!(a, vec![0, 0, 1, 2, 2]);
    }

    #[test]
    fn wcc_skips_out_of_range_endpoints() {
        // Endpoint 9 is out of range → ignored, no panic.
        let c = weakly_connected_components(2, &[(0, 9)]);
        assert_eq!(c, vec![0, 1]);
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
}
