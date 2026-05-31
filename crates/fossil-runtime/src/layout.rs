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

// ──────────────────────────────────────────────────────────────────────────
// W3.1b — integration: apply the pure layout to the written GraphAr vertices.
// ──────────────────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};

use duckdb::Connection;

/// One vertex type's layout target: its on-disk vertex Parquet plus the CSR
/// Parquets of its **self-edges** (`src_type == dst_type == this type`), whose
/// `src_dense`/`dst_dense` live in this type's `dense_id` space. Cross-type
/// edges are excluded here — a global cross-type layout is a later slice.
#[derive(Debug, Clone)]
pub struct VertexLayoutTarget {
    /// Local path to the vertex Parquet (e.g. `<dest>/vertex/Person.parquet`).
    pub vertex_parquet: PathBuf,
    /// Local paths to this type's self-edge CSR Parquets.
    pub self_edge_csr: Vec<PathBuf>,
}

/// Failure modes of [`enrich_layout`].
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// A `DuckDB` query (count, edge read, rewrite COPY) failed.
    #[error("layout DuckDB op on `{target}` failed: {source}")]
    Duck {
        target: String,
        #[source]
        source: duckdb::Error,
    },
    /// Renaming the rewritten temp Parquet over the original failed.
    #[error("layout rename for `{target}` failed: {source}")]
    Rename {
        target: String,
        #[source]
        source: std::io::Error,
    },
}

/// Replace the W0b placeholder `x`/`y`/`cluster_id` columns of each vertex
/// Parquet with a real WCC partition + deterministic placement.
///
/// Per target: count vertices (`max(dense_id)+1`), read self-edges, run
/// [`weakly_connected_components`] + [`cluster_layout`], stage the result in a
/// temp table, and rewrite the Parquet via `SELECT * REPLACE (...)`. Row ORDER
/// is preserved (no morton sort yet — a later slice), so `dense_id` values are
/// unchanged and every edge stays valid. The COPY writes a sibling `.tmp` then
/// renames over the original (never reads + writes the same file in one stmt).
///
/// # Errors
///
/// Returns [`LayoutError`] on the first failing `DuckDB` op or rename.
pub fn enrich_layout(conn: &Connection, targets: &[VertexLayoutTarget]) -> Result<(), LayoutError> {
    for target in targets {
        let vpath = target.vertex_parquet.as_path();
        let vname = vpath.display().to_string();
        let duck = |source: duckdb::Error| LayoutError::Duck {
            target: vname.clone(),
            source,
        };

        let vertex_count: u32 = conn
            .query_row(
                &format!(
                    "SELECT coalesce(max(dense_id) + 1, 0)::UINTEGER FROM read_parquet('{}')",
                    sql_lit(vpath)
                ),
                [],
                |r| r.get(0),
            )
            .map_err(duck)?;
        if vertex_count == 0 {
            continue;
        }

        let mut edges: Vec<(u32, u32)> = Vec::new();
        for csr in &target.self_edge_csr {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT src_dense, dst_dense FROM read_parquet('{}')",
                    sql_lit(csr)
                ))
                .map_err(duck)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))
                .map_err(duck)?;
            for row in rows {
                edges.push(row.map_err(duck)?);
            }
        }

        let clusters = weakly_connected_components(vertex_count, &edges);
        let positions = cluster_layout(&clusters);
        let morton = morton_codes(&positions);

        // Stage (dense_id, x, y, cluster_id, morton) in a temp table via the
        // Appender. `morton` is used only for ORDER BY — it is NOT carried into
        // the output Parquet (the rewrite SELECTs the vertex columns only).
        conn.execute_batch(
            "CREATE OR REPLACE TEMP TABLE __fossil_layout \
             (dense_id UINTEGER, x REAL, y REAL, cluster_id UINTEGER, morton UINTEGER)",
        )
        .map_err(duck)?;
        {
            let mut appender = conn.appender("__fossil_layout").map_err(duck)?;
            for (dense_id, (&(x, y), (&cluster_id, &morton_code))) in positions
                .iter()
                .zip(clusters.iter().zip(morton.iter()))
                .enumerate()
            {
                appender
                    .append_row(duckdb::params![
                        dense_id as u32,
                        x,
                        y,
                        cluster_id,
                        morton_code
                    ])
                    .map_err(duck)?;
            }
            // appender flushes on drop (end of this block) before the COPY reads it.
        }

        // Rewrite: same columns, x/y/cluster_id replaced from the temp table,
        // rows reordered by Morton(x,y) so a bbox viewport query prunes via
        // row-group stats. `dense_id` VALUES are unchanged (only the row order),
        // so every edge stays valid. Write a sibling `.tmp` then rename over the
        // original (never read + write the same file in one statement).
        let tmp = vpath.with_extension("parquet.tmp");
        conn.execute_batch(&format!(
            "COPY (SELECT v.* REPLACE (l.x AS x, l.y AS y, l.cluster_id AS cluster_id) \
             FROM read_parquet('{}') v JOIN __fossil_layout l USING (dense_id) \
             ORDER BY l.morton) \
             TO '{}' (FORMAT PARQUET)",
            sql_lit(vpath),
            sql_lit(&tmp),
        ))
        .map_err(duck)?;
        std::fs::rename(&tmp, vpath).map_err(|source| LayoutError::Rename {
            target: vname.clone(),
            source,
        })?;
    }
    Ok(())
}

/// Escape a path for embedding in a single-quoted `DuckDB` SQL string literal
/// (`read_parquet('…')` / `COPY … TO '…'` take literals, not bind params).
fn sql_lit(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// Interleave the low 16 bits of `x` and `y` into a 32-bit Morton (Z-order)
/// code (`x` in even bits, `y` in odd). Spatially-near points get
/// near-sequential codes, so sorting vertices by it groups nearby ones into the
/// same Parquet row group — a bbox viewport query then prunes via row-group
/// min/max stats (the larger-than-RAM predicate-pushdown contract).
fn morton2(x: u16, y: u16) -> u32 {
    fn spread(n: u16) -> u32 {
        let mut n = u32::from(n);
        n = (n | (n << 8)) & 0x00ff_00ff;
        n = (n | (n << 4)) & 0x0f0f_0f0f;
        n = (n | (n << 2)) & 0x3333_3333;
        n = (n | (n << 1)) & 0x5555_5555;
        n
    }
    spread(x) | (spread(y) << 1)
}

/// Morton codes for a position list — quantises each coordinate to `u16` over
/// the list's bounding box (a degenerate axis maps to 0). Index-aligned with
/// `positions`.
#[must_use]
fn morton_codes(positions: &[(f32, f32)]) -> Vec<u32> {
    if positions.is_empty() {
        return Vec::new();
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in positions {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let quantize = |v: f32, lo: f32, hi: f32| -> u16 {
        if hi <= lo {
            return 0;
        }
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        (t * f32::from(u16::MAX)).round() as u16
    };
    positions
        .iter()
        .map(|&(x, y)| morton2(quantize(x, min_x, max_x), quantize(y, min_y, max_y)))
        .collect()
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

    #[test]
    fn morton2_interleaves_bits() {
        // x bits in even positions, y bits in odd. (1,0)→0b01=1; (0,1)→0b10=2;
        // (1,1)→0b11=3; (3,0)→0b0101=5.
        assert_eq!(morton2(0, 0), 0);
        assert_eq!(morton2(1, 0), 1);
        assert_eq!(morton2(0, 1), 2);
        assert_eq!(morton2(1, 1), 3);
        assert_eq!(morton2(3, 0), 5);
    }

    #[test]
    fn morton_codes_quantize_and_order_spatially() {
        // Two points near the origin get closer codes than a far one.
        let codes = morton_codes(&[(0.0, 0.0), (1.0, 1.0), (100.0, 100.0)]);
        assert_eq!(codes.len(), 3);
        assert!(codes[0] < codes[2] && codes[1] < codes[2]);
    }

    #[test]
    fn morton_codes_degenerate_axis_no_panic() {
        // All same y (range 0 on that axis) → no div-by-zero.
        let codes = morton_codes(&[(0.0, 5.0), (10.0, 5.0)]);
        assert_eq!(codes.len(), 2);
    }
}
