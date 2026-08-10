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
//! - [`community_hierarchy`] — modularity communities, and the whole hierarchy
//!   of them, which is what [`enrich_layout`] partitions by. This is the pyramid
//!   the level-of-detail plan is built from (ADR-0041 §1).
//! - [`weakly_connected_components`] — reachability (union-find). It is a real
//!   graph property and stays, but it is **no longer what the layout uses**: on
//!   a connected graph it answers "one component", and measured on the million
//!   corpus that put 994,786 of a million vertices in a single cluster. It now
//!   earns its place as the contrast the hierarchy is tested against.
//! - [`cluster_layout`] — a deterministic community-grouped placement: clusters
//!   on a grid, nodes phyllotaxis-packed within their cell. Same-cluster nodes
//!   land near each other. `ForceAtlas2` refinement is a later slice; this gives
//!   the viewport real, stable coordinates without an iterative force sim.
//!
//! Both are pure (no `DuckDB`, no I/O, no RNG) so they unit-test in isolation and
//! the `materialize` integration can wire them with confidence.

// This is deliberate numeric code: dense ids / cluster counts cast to/from `f32`
// coordinates and `f64` grid maths, and tight index loops over `dense_id` arrays.
// The pedantic cast lints + the nursery loop/option rewrites read worse here than
// the explicit arithmetic, so they are declined for this algorithmic core.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::needless_range_loop,
    clippy::option_if_let_else
)]

/// Golden angle (radians) — the phyllotaxis constant `π(3−√5)`. Successive
/// nodes placed at multiples of this angle pack a disc evenly with no RNG.
const GOLDEN_ANGLE: f32 = 2.399_963_2;
/// Empty space left between one cluster's packing disc and the next. The grid
/// pitch is this plus the largest disc's diameter, so it is a margin and not the
/// pitch itself — see [`cluster_layout`].
const CLUSTER_SPACING: f32 = 100.0;
/// How many clusters `cluster_id` may carry.
///
/// Not an aesthetic choice: `viewport`'s aggregate mode answers one super-node
/// per `(type_idx, cluster_id)` and promises "≤ 10k super-nodes regardless of
/// total N" (`fossil-graph/src/exec.rs`, `viewport_aggregate`). That promise is
/// kept by a `LIMIT`, so a partition finer than the budget does not degrade —
/// it truncates, and the picture silently loses whole communities. A budget of
/// 2,048 leaves the promise intact for up to four vertex types.
const CLUSTER_BUDGET: u32 = 2_048;
/// Intra-cluster packing radius scale (kept well below [`CLUSTER_SPACING`] so
/// same-cluster nodes stay closer to each other than to other clusters).
const INTRA_CLUSTER_RADIUS: f32 = 12.0;
/// Empty space between one vertex type's region and the next — two cluster
/// cells, so the seam between types reads as deliberate rather than as a gap
/// that happened.
const TYPE_GUTTER: f32 = CLUSTER_SPACING * 2.0;

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
        let id = if let Some(id) = label[root as usize] {
            id
        } else {
            let id = next;
            label[root as usize] = Some(id);
            next += 1;
            id
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
/// by [`community_hierarchy`], [`flatten_to_budget`] and [`order_by_hierarchy`]).
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
/// [`CLUSTER_SPACING`] as the gap between them.
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

use std::fmt::Write as _;

use duckdb::Connection;
use duckdb::arrow::array::{Array, UInt32Array};
use fossil_base::probe::Probe;

/// One vertex type's layout target: its vertex Parquet URL plus the CSR Parquet
/// URLs of its **self-edges** (`src_type == dst_type == this type`), whose
/// `src_dense`/`dst_dense` live in this type's `dense_id` space. Cross-type
/// edges are excluded here — a global cross-type layout is a later slice.
///
/// URLs (not paths) so the same enrichment runs against `file://` and cloud
/// (`s3://`, `az://`) destinations alike — `read_parquet` / `COPY … TO` take the
/// URL verbatim and `DuckDB`'s httpfs/object-store extension dereferences it.
#[derive(Debug, Clone)]
pub struct VertexLayoutTarget {
    /// Schema label, e.g. `"Person"` — what an [`AdjacencyTarget`] names to say
    /// which `dense_id` space each of its two endpoint columns lives in.
    pub type_name: String,
    /// Vertex Parquet URL (e.g. `file://…/vertex/Person.parquet`, `s3://…`).
    ///
    /// This is the writer's single-file output and is **read, not written**: the
    /// enriched vertices are emitted as chunks under [`Self::chunk_prefix`].
    pub vertex_parquet: String,
    /// Where the tiles go — the manifest's `prefix`, e.g. `…/vertex/Person/`,
    /// trailing separator included. Files are named `chunk{k}.parquet`
    /// (ADR-0016).
    pub chunk_prefix: String,
    /// Rows per tile — the manifest's `chunk_size`. The manifest and the files
    /// have to agree, so this comes from whoever wrote the manifest rather than
    /// being a constant here.
    ///
    /// **A power of two, and refused otherwise.** Tile `k` is the `dense_id`
    /// range `[k·chunk_size, (k+1)·chunk_size)`, and the point of the range being
    /// fixed is that a reader finds it with `dense_id >> shift` instead of a
    /// division and a table (ADR-0042 §3.3). A size that is not a power of two
    /// still *emits* correctly and quietly costs every reader that arithmetic,
    /// which is why it is an error here rather than a rounding.
    pub chunk_size: u64,
    /// This type's self-edge CSR Parquet URLs.
    pub self_edge_csr: Vec<String>,
}

/// Which endpoint column an adjacency list is sorted by — `GraphAr`'s
/// `aligned_by`, i.e. CSR (`Src`) or CSC (`Dst`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// Ordered by `src_dense` — the CSR orientation.
    Src,
    /// Ordered by `dst_dense` — the CSC orientation.
    Dst,
}

/// One adjacency-list Parquet, and everything needed to rewrite it when the
/// `dense_id` numbering underneath it changes.
///
/// **Every** file referencing a renumbered vertex type has to be listed, on
/// either endpoint and in both orientations. The layout used to take only the
/// same-type `by_source` files, which was enough while it read edges and wrote
/// nothing back to them; the moment `dense_id` values change it is not, because
/// a `by_target` file and a cross-type file hold the same ids and would be left
/// pointing at whoever inherited their numbers. Nothing would fail — the column
/// is still a valid `UINTEGER` — so the corruption is silent, which is why the
/// caller is made to enumerate rather than the layout to guess.
#[derive(Debug, Clone)]
pub struct AdjacencyTarget {
    /// The adjacency Parquet URL.
    pub parquet: String,
    /// Schema label of the type `src_dense` indexes.
    pub src_type: String,
    /// Schema label of the type `dst_dense` indexes.
    pub dst_type: String,
    /// The manifest declares `ordered: true`, so this says ordered by *what*.
    pub ordered_by: Endpoint,
}

/// Failure modes of [`enrich_layout`].
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// A `DuckDB` query (count, edge read, stage, rewrite COPY) failed.
    #[error("layout DuckDB op on `{target}` failed: {source}")]
    Duck {
        target: String,
        #[source]
        source: duckdb::Error,
    },
    /// An [`AdjacencyTarget`] named a type no [`VertexLayoutTarget`] provides,
    /// so its endpoints could not be renumbered.
    #[error("adjacency `{target}` references vertex type `{vertex_type}`, which was not laid out")]
    UnknownVertexType { target: String, vertex_type: String },
    /// The renumbering join dropped rows, which means an endpoint referenced a
    /// `dense_id` no vertex has.
    ///
    /// Worth an error rather than a warning: the rewrite is an inner join, so a
    /// dangling endpoint does not fail, it *disappears* — and an adjacency list
    /// quietly missing edges reads downstream as a sparser graph, not as a bug.
    #[error("renumbering `{target}` dropped {dropped} of {before} rows — dangling endpoints")]
    DanglingEndpoint {
        target: String,
        before: u64,
        dropped: u64,
    },
    /// A self-edge CSR was listed with no target-ordered counterpart among the
    /// adjacencies, so the reverse edges have nowhere to be read from in order.
    #[error("adjacency `{target}` has no target-ordered counterpart beside it")]
    MissingOrientation { target: String },
    /// A tile size that is not a power of two, so `dense_id >> shift` does not
    /// name a tile.
    #[error(
        "vertex type `{vertex_type}` declares a tile of {rows} rows, which is not a power of two"
    )]
    TileSize { vertex_type: String, rows: u64 },
    /// A tile prefix that could not be created on the local filesystem.
    #[error("creating the tile directory `{prefix}` failed: {source}")]
    Prefix {
        prefix: String,
        #[source]
        source: std::io::Error,
    },
    /// An adjacency the manifest declares `ordered: true` is not.
    ///
    /// Worth an error for the same reason as [`Self::DanglingEndpoint`]: the
    /// layout reads the file as the CSR it claims to be, so a file out of order
    /// does not fail — it builds a different graph, and lays the corpus out by
    /// it. Checked while streaming, for one comparison per row.
    #[error("adjacency `{target}` is not ordered by `{column}`, which the manifest claims it is")]
    Disordered { target: String, column: String },
}

/// Replace the W0b placeholder `x`/`y`/`cluster_id` columns of each vertex
/// Parquet with a real community partition + deterministic placement, and
/// **renumber `dense_id` into Morton order**, remapping every adjacency list.
///
/// # Why the renumbering is here and not in the writer
///
/// `GraphAr` defines chunk *i* as the `dense_id` range `[i·chunk_size,
/// (i+1)·chunk_size)`, so a chunk is a spatial tile only if `dense_id` ascends
/// with position. `finalize_vertex` numbers in IRI order, and this pass used to
/// reorder the *rows* by Morton code while leaving the *values* alone — which
/// made the file's physical order spatial and its chunk definition not. Measured
/// on the five-million corpus in 41 chunks, a window touched 41 of 41 chunks by
/// `dense_id` and 6 of 41 by physical row order (ADR-0041 §2).
///
/// ADR-0041 assumed this meant moving the layout ahead of the edge phase. It
/// does not: this pass already runs last, holding the `DuckDB` connection, with
/// every adjacency already written as Parquet. Renumbering after the fact is a
/// join against a mapping table, which is strictly less invasive than reordering
/// the phases.
///
/// # What it does
///
/// Vertices first, all of them, because an adjacency spans two types and cannot
/// be rewritten until both mappings exist. Per type: count vertices, read
/// self-edges, run [`community_hierarchy`] + [`cluster_layout`], derive the
/// Morton rank of each vertex, and stage `dense_id → (new_dense_id, x, y,
/// cluster_id)`. The enriched vertices are then emitted **as tiles** under
/// [`VertexLayoutTarget::chunk_prefix`] — `chunk{k}.parquet`, `chunk_size` rows
/// each. The writer's single-file output is the input to this and is not written
/// back.
///
/// Then every adjacency: both endpoints remapped through their own type's
/// mapping, and **re-sorted**, because the manifest declares `ordered: true` and
/// a CSR sorted on `src_dense` stops being sorted the moment those values change.
///
/// And finally the source-ordered adjacencies are emitted as tiles too, under
/// `by_source/tile{k}.parquet`, **keyed by the same range as the vertices**: tile
/// `k` holds every edge whose `src_dense` is in vertex tile `k`. That is CSR, and
/// it is the placement ADR-0042 §3.2 measured against the alternative of hoisting
/// an edge to the deepest tile holding both its endpoints — which reads 2.29× to
/// 15.86× more edges across 200k/1M/5M/10M against CSR's flat 1.95× to 2.89×,
/// and touches 2.5–3.5× the tiles. The mechanism is that near the root of such a
/// tree there is no branching left to prune with. CSR needs no second request:
/// every drawable edge has its source on screen, so the tiles of the window are
/// an exact superset of what can be drawn.
///
/// The target-ordered adjacency is **not** tiled. It is the half that would
/// answer "an edge with one endpoint off screen", which is a different question
/// and doubles the addressing to ask it (ADR-0042 §3.6).
///
/// # Errors
///
/// Returns [`LayoutError`] on the first failing `DuckDB` op, on an adjacency
/// naming an unknown vertex type, or on a renumbering that dropped rows.
pub fn enrich_layout(
    conn: &Connection,
    targets: &[VertexLayoutTarget],
    adjacencies: &[AdjacencyTarget],
) -> Result<(), LayoutError> {
    // Where the next vertex type's grid starts, so the types do not stack. Each
    // is laid out independently and `cluster_layout` always begins at the
    // origin, so without this every type occupies the same coordinates and a
    // two-type graph renders as one blob with its communities interleaved at
    // random — and a bbox query answers with vertices that have nothing to do
    // with each other but their position. See `place_after`.
    let mut origin_x = 0.0f32;
    let mut probe = Probe::new(&format!(
        "enrich_layout — {} vertex type(s), {} adjacency target(s)",
        targets.len(),
        adjacencies.len()
    ));

    for (index, target) in targets.iter().enumerate() {
        let vurl = target.vertex_parquet.as_str();
        let vname = vurl.to_string();
        let duck = |source: duckdb::Error| LayoutError::Duck {
            target: vname.clone(),
            source,
        };
        // Checked before anything is written, so a bad tile size is a refusal
        // rather than a corpus that has to be thrown away.
        let shift = shift_for(target.chunk_size).ok_or_else(|| LayoutError::TileSize {
            vertex_type: target.type_name.clone(),
            rows: target.chunk_size,
        })?;
        ensure_prefix(&target.chunk_prefix)?;

        // Created even for an empty type, so phase two can join against it and
        // report a dangling endpoint rather than fail to find a table.
        let map = map_table(index);
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {map} \
             (dense_id UINTEGER, new_dense_id UINTEGER, x REAL, y REAL, cluster_id UINTEGER)"
        ))
        .map_err(duck)?;

        let vertex_count: u32 = conn
            .query_row(
                &format!(
                    "SELECT coalesce(max(dense_id) + 1, 0)::UINTEGER FROM read_parquet('{}')",
                    sql_lit(vurl)
                ),
                [],
                |r| r.get(0),
            )
            .map_err(duck)?;
        if vertex_count == 0 {
            continue;
        }

        // The artefact is already the CSR this needs — `by_source.parquet` has
        // zero disorders by `src_dense` over 71M rows and `by_target.parquet`
        // zero by `dst_dense`, and both have since the writer emitted them. What
        // used to stand here read the pairs into a `Vec<(u32, u32)>` (568 MB at
        // ten million), counted degrees in a second pass and scattered through a
        // cloned cursor (80 MB) doing random writes over the 568. None of the
        // three is asked for by the algorithm; all three exist because the
        // parameter was an unordered bag (ADR-0043 §1).
        //
        // Both orientations, because the layout is undirected: the file grouped
        // by source holds each vertex's out-neighbours and the one grouped by
        // target its in-neighbours, and a vertex's neighbourhood is the two
        // concatenated. Neither is transformed to get there.
        let mut sides = Vec::with_capacity(target.self_edge_csr.len() * 2);
        let mut self_loops = vec![0.0f64; vertex_count as usize];
        for csr in &target.self_edge_csr {
            let csc =
                csc_beside(adjacencies, csr).ok_or_else(|| LayoutError::MissingOrientation {
                    target: csr.clone(),
                })?;
            sides.push(read_orientation(
                conn,
                csr,
                "src_dense",
                "dst_dense",
                vertex_count,
                &mut self_loops,
            )?);
            sides.push(read_orientation(
                conn,
                csc,
                "dst_dense",
                "src_dense",
                vertex_count,
                &mut self_loops,
            )?);
        }
        // A self-loop is a row in *both* orientations, so the reads above saw
        // each of them twice. Halving is exact — the counts are integers.
        for count in &mut self_loops {
            *count /= 2.0;
        }
        probe.mark("read CSR + CSC");

        let levels = hierarchy(Weighted::finish(sides, self_loops));
        probe.mark("community_hierarchy");

        // The partition that is *written* and the partition that is *drawn*
        // answer different questions, so they are not the same partition.
        //
        // `cluster_id` is read by `viewport`'s aggregate mode, one super-node
        // per cluster under a LIMIT, so it must fit CLUSTER_BUDGET — which on
        // this corpus means the top of the hierarchy. Placement wants the
        // opposite: the finest level, whose communities are small enough that
        // several fit in one window. Using the budget partition for both was
        // measured and cost the ordering below its entire reason for existing —
        // at the top level every community is a root and there are no siblings
        // left to put side by side.
        let (clusters, _) = flatten_to_budget(&levels, vertex_count, CLUSTER_BUDGET);
        let mut placement = levels
            .first()
            .cloned()
            .unwrap_or_else(|| (0..vertex_count).collect());
        if !levels.is_empty() {
            order_by_hierarchy(&levels, 0, &mut placement);
        }
        let mut positions = cluster_layout(&placement);
        probe.mark("flatten + order + place");

        // Slide this type clear of the ones already placed. The Morton codes are
        // computed *after* the shift, because they quantise against the position
        // list's own bounding box — coding first would sort the rows by a
        // geometry the file no longer has, and the row-group statistics a bbox
        // query prunes on would describe somewhere else.
        origin_x = place_after(&mut positions, origin_x);

        let morton = morton_codes(&positions);
        let new_ids = morton_ranks(&morton);
        probe.mark("morton codes + ranks");

        // Stage dense_id → (new_dense_id, x, y, cluster_id) via the Appender.
        {
            let mut appender = conn.appender(&map).map_err(duck)?;
            for (dense_id, (&(x, y), (&cluster_id, &new_dense_id))) in positions
                .iter()
                .zip(clusters.iter().zip(new_ids.iter()))
                .enumerate()
            {
                appender
                    .append_row(duckdb::params![
                        dense_id as u32,
                        new_dense_id,
                        x,
                        y,
                        cluster_id
                    ])
                    .map_err(duck)?;
            }
            // appender flushes on drop (end of this block) before the COPY reads it.
        }
        probe.mark("stage map (appender)");

        // Stage the vertices in a temp table so the rewrite COPY reads from
        // memory, not from the very Parquet it overwrites. This replaces the
        // earlier `.tmp` sibling + `std::fs::rename` dance: a rename is a
        // local-filesystem primitive cloud object stores (`s3://`, `az://`)
        // don't offer, and the dataset is freshly written with no concurrent
        // readers, so an in-place overwrite is safe. One code path, local + cloud.
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE __fossil_vertices AS \
             SELECT * FROM read_parquet('{}')",
            sql_lit(vurl)
        ))
        .map_err(duck)?;
        probe.mark("materialise vertices (duck)");

        // The enriched rows: same columns, x/y/cluster_id and dense_id all
        // replaced from the mapping. Ordering by the new id *is* ordering by
        // Morton code — that is what the new id is — so a `dense_id` range and a
        // contiguous run of the picture are the same set of rows, which is the
        // property a tile needs and the one this pass used to leave broken.
        //
        // `ORDER BY` on the staging table and not only inside each tile's COPY:
        // a tile is a range predicate over this table, and DuckDB prunes a table
        // scan on the per-row-group min/max it keeps. Sorted, a tile reads one
        // row group; unsorted, it reads all of them, and at 4,096 rows a million
        // vertices is 245 tiles rather than 9 — the same full scan repeated 245
        // times. The tile size is what made this matter; it was invisible at
        // 122,880.
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE __fossil_enriched AS \
             SELECT v.* REPLACE (m.new_dense_id AS dense_id, m.x AS x, m.y AS y, \
             m.cluster_id AS cluster_id) \
             FROM __fossil_vertices v JOIN {map} m USING (dense_id) ORDER BY 1"
        ))
        .map_err(duck)?;
        probe.mark("join enriched (duck)");

        // One Parquet per tile, which is the whole point: a tile is an HTTP
        // resource a browser and a CDN can cache, and its address is
        // `dense_id >> shift` — no index, no listing, no discovery. The loop the
        // naming convention forces is not the cost it looks like: at five million
        // 200 files took 0.18 s to write. `PARTITION_BY` would be one statement
        // but emits `chunk=0/data_0.parquet` rather than the `chunk{k}.parquet`
        // ADR-0016 specifies, which is a directory listing wearing a filename.
        let tiles = u64::from(vertex_count).div_ceil(target.chunk_size);
        let mut emission = String::new();
        for k in 0..tiles {
            let (lo, hi) = (k << shift, (k + 1) << shift);
            let _ = write!(
                emission,
                "COPY (SELECT * FROM __fossil_enriched \
                 WHERE dense_id >= {lo} AND dense_id < {hi} ORDER BY dense_id) \
                 TO '{}chunk{k}.parquet' (FORMAT PARQUET);",
                sql_lit(&target.chunk_prefix),
            );
        }
        conn.execute_batch(&emission).map_err(duck)?;

        // Free the staged vertices before the next target (each type can be
        // large; the temp tables are single-use per iteration). The mapping stays
        // — phase two needs every type's at once.
        conn.execute_batch(
            "DROP TABLE IF EXISTS __fossil_vertices; DROP TABLE IF EXISTS __fossil_enriched",
        )
        .map_err(duck)?;
    }

    let index_of = |name: &str, target: &str| {
        targets
            .iter()
            .position(|t| t.type_name == name)
            .ok_or_else(|| LayoutError::UnknownVertexType {
                target: target.to_string(),
                vertex_type: name.to_string(),
            })
    };

    probe.mark("write vertex chunks");

    for adjacency in adjacencies {
        let aurl = adjacency.parquet.as_str();
        let aname = aurl.to_string();
        let duck = |source: duckdb::Error| LayoutError::Duck {
            target: aname.clone(),
            source,
        };
        let src_map = map_table(index_of(&adjacency.src_type, aurl)?);
        let dst_map = map_table(index_of(&adjacency.dst_type, aurl)?);

        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE __fossil_adjacency AS \
             SELECT * FROM read_parquet('{}')",
            sql_lit(aurl)
        ))
        .map_err(duck)?;
        let before: u64 = conn
            .query_row("SELECT count(*) FROM __fossil_adjacency", [], |r| r.get(0))
            .map_err(duck)?;

        // Re-sorted, not just remapped. `adj_lists` declares `ordered: true`, and
        // a CSR sorted on `src_dense` stops being sorted the instant those values
        // are replaced — with no error anywhere, because the column is still a
        // perfectly good UINTEGER. A reader trusting the manifest would binary
        // search a list that is no longer in order.
        let order = match adjacency.ordered_by {
            Endpoint::Src => "s.new_dense_id, d.new_dense_id",
            Endpoint::Dst => "d.new_dense_id, s.new_dense_id",
        };
        conn.execute_batch(&format!(
            "COPY (SELECT e.* REPLACE (s.new_dense_id AS src_dense, d.new_dense_id AS dst_dense) \
             FROM __fossil_adjacency e \
             JOIN {src_map} s ON s.dense_id = e.src_dense \
             JOIN {dst_map} d ON d.dense_id = e.dst_dense \
             ORDER BY {order}) \
             TO '{}' (FORMAT PARQUET)",
            sql_lit(aurl),
        ))
        .map_err(duck)?;

        let after: u64 = conn
            .query_row(
                &format!("SELECT count(*) FROM read_parquet('{}')", sql_lit(aurl)),
                [],
                |r| r.get(0),
            )
            .map_err(duck)?;
        if after != before {
            return Err(LayoutError::DanglingEndpoint {
                target: aname,
                before,
                dropped: before - after,
            });
        }

        conn.execute_batch("DROP TABLE IF EXISTS __fossil_adjacency")
            .map_err(duck)?;
    }
    probe.mark("remap adjacencies");

    // The edge half of the tiling, and the last thing written: an edge lives in
    // its source's tile, so this reads the file the loop above just re-sorted by
    // `src_dense` and cuts it on the same ranges the vertices were cut on.
    //
    // Read back from the Parquet rather than kept in a temp table on the way
    // past. The file is sorted on the very column each tile filters, so Parquet's
    // own row-group statistics prune the scan, and a staging table would hold a
    // second copy of the adjacency beside the one the remap already materialises
    // — 568 MB at ten million, which is the allocation ADR-0043 stage 1 spent
    // itself removing.
    for adjacency in adjacencies {
        if adjacency.ordered_by != Endpoint::Src {
            continue;
        }
        let aurl = adjacency.parquet.as_str();
        let aname = aurl.to_string();
        let duck = |source: duckdb::Error| LayoutError::Duck {
            target: aname.clone(),
            source,
        };
        let source = &targets[index_of(&adjacency.src_type, aurl)?];
        let shift = shift_for(source.chunk_size).ok_or_else(|| LayoutError::TileSize {
            vertex_type: source.type_name.clone(),
            rows: source.chunk_size,
        })?;
        let prefix = tile_prefix(aurl);
        ensure_prefix(&prefix)?;

        // Which tiles exist, asked rather than assumed. A vertex tile is always
        // full — dense ids are gapless — but a tile of 4,096 sources can hold no
        // edges at all, and writing the empty file to say so is a request the
        // reader pays for to learn nothing. A 404 says it for free.
        let occupied: Vec<u64> = {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT DISTINCT (src_dense >> {shift})::UBIGINT AS tile \
                     FROM read_parquet('{}') ORDER BY tile",
                    sql_lit(aurl)
                ))
                .map_err(duck)?;

            stmt.query_map([], |r| r.get::<_, u64>(0))
                .map_err(duck)?
                .collect::<Result<Vec<u64>, _>>()
                .map_err(duck)?
        };

        let mut emission = String::new();
        for k in occupied {
            let (lo, hi) = (k << shift, (k + 1) << shift);
            let _ = write!(
                emission,
                "COPY (SELECT * FROM read_parquet('{}') \
                 WHERE src_dense >= {lo} AND src_dense < {hi} \
                 ORDER BY src_dense, dst_dense) TO '{}tile{k}.parquet' (FORMAT PARQUET);",
                sql_lit(aurl),
                sql_lit(&prefix),
            );
        }
        conn.execute_batch(&emission).map_err(duck)?;
    }
    probe.mark("write edge tiles");
    probe.finish();

    Ok(())
}

/// The shift that addresses a tile of `rows` rows, or `None` if `rows` is not a
/// power of two and therefore addresses nothing.
const fn shift_for(rows: u64) -> Option<u32> {
    if rows == 0 || !rows.is_power_of_two() {
        return None;
    }
    Some(rows.trailing_zeros())
}

/// Where one adjacency's tiles go, from where the adjacency itself is:
/// `…/by_source.parquet` → `…/by_source/`.
///
/// Derived and not passed in, because the caller has nothing to say about it: the
/// tiles are the same relation cut on the same ranges, and a second path to
/// configure is a second path to get wrong. A URL that is not a `.parquet` gets
/// the separator appended, which cannot happen from the writer and is not worth
/// an error variant nobody can reach.
fn tile_prefix(adjacency: &str) -> String {
    format!(
        "{}/",
        adjacency.strip_suffix(".parquet").unwrap_or(adjacency)
    )
}

/// Create the directory a local prefix names, so `COPY` has somewhere to put a
/// tile.
///
/// A no-op for a cloud URL, and deliberately: an object store has no directories
/// and the prefix is part of the key, so there is nothing to create and an error
/// would be invented. Local paths and `file://` are the case `DuckDB` will not
/// create for itself.
fn ensure_prefix(prefix: &str) -> Result<(), LayoutError> {
    let local = prefix.strip_prefix("file://").unwrap_or(prefix);
    if local.contains("://") {
        return Ok(());
    }
    std::fs::create_dir_all(local).map_err(|source| LayoutError::Prefix {
        prefix: prefix.to_string(),
        source,
    })
}

/// Name of the temp table holding vertex type `index`'s `dense_id` mapping.
fn map_table(index: usize) -> String {
    format!("__fossil_map_{index}")
}

/// The target-ordered file sitting beside a source-ordered one — the same edge
/// table's other orientation, where the reverse edges are already grouped by the
/// endpoint the layout needs them under.
///
/// Not a new input, and deliberately not a new field on [`VertexLayoutTarget`]:
/// the caller already enumerates every adjacency file with the endpoint it is
/// ordered by, because renumbering rewrites both. The two orientations of one
/// edge table share a directory, which is what pairs them.
fn csc_beside<'a>(adjacencies: &'a [AdjacencyTarget], csr: &str) -> Option<&'a str> {
    fn dir(url: &str) -> &str {
        url.rsplit_once('/').map_or("", |(d, _)| d)
    }
    adjacencies
        .iter()
        .find(|a| a.ordered_by == Endpoint::Dst && dir(&a.parquet) == dir(csr))
        .map(|a| a.parquet.as_str())
}

/// Read one adjacency Parquet as the CSR it already is.
///
/// `stream_arrow` and not `query_map`: the latter materialises the whole result
/// set, which is what the first attempt at this measured — the process peak went
/// 17.0 → 24.4 GiB while the three parity tests stayed green (ADR-0043 §1). The
/// item of a stream is a `RecordBatch`, so the two columns arrive as the `u32`
/// slices the builder wants and no row is ever a Rust tuple.
fn read_orientation(
    conn: &Connection,
    url: &str,
    key: &str,
    value: &str,
    vertex_count: u32,
    self_loops: &mut [f64],
) -> Result<Csr, LayoutError> {
    let name = url.to_string();
    let duck = |source: duckdb::Error| LayoutError::Duck {
        target: name.clone(),
        source,
    };
    let sql = format!(
        "SELECT {key}::UINTEGER AS k, {value}::UINTEGER AS v FROM read_parquet('{}')",
        sql_lit(url)
    );
    // Reserved exactly, from the Parquet footer rather than by doubling: the
    // targets array is the one large allocation left and growing into it would
    // put a copy of it beside itself.
    let edges: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM read_parquet('{}')", sql_lit(url)),
            [],
            |r| r.get(0),
        )
        .map_err(duck)?;

    // `stream_arrow` wants the schema before the statement has run, and
    // `Statement::schema` panics until it has. A `LIMIT 0` execution answers it
    // for the price of the footer, which beats asserting a layout here and
    // finding out about it inside `from_ffi`.
    let schema = {
        let mut header = conn.prepare(&format!("{sql} LIMIT 0")).map_err(duck)?;
        header.query_arrow([]).map_err(duck)?.get_schema()
    };

    let mut builder = CsrBuilder::new(vertex_count as usize, edges.max(0) as usize);
    let mut stmt = conn.prepare(&sql).map_err(duck)?;
    for batch in stmt.stream_arrow([], schema).map_err(duck)? {
        let cast = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<UInt32Array>()
                .expect("the projection casts both columns to UINTEGER")
        };
        let (keys, values) = (cast(0), cast(1));
        if !builder.push(keys.values(), values.values(), self_loops) {
            return Err(LayoutError::Disordered {
                target: url.to_string(),
                column: key.to_string(),
            });
        }
    }
    Ok(builder.finish())
}

/// Rank each vertex by its Morton code — its position in the renumbering.
///
/// Ties are broken by the old `dense_id`, so the ranking is total and the same
/// input yields the same numbering on every run. Two vertices sharing a code is
/// the common case rather than an edge case: the codes quantise to 16 bits per
/// axis, and a community packs many vertices into far less than one bucket.
fn morton_ranks(morton: &[u32]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..morton.len() as u32).collect();
    order.sort_unstable_by_key(|&i| (morton[i as usize], i));
    let mut rank = vec![0u32; morton.len()];
    for (new_id, &old_id) in order.iter().enumerate() {
        rank[old_id as usize] = new_id as u32;
    }
    rank
}

/// Translate one vertex type's layout to start at `origin_x`, and answer where
/// the next type should start.
///
/// [`cluster_layout`] always begins at the origin, so laying several types out
/// independently puts every one of them in the same place. Read back through the
/// `viewport` verb that is worse than ugly: a rectangle answers with vertices
/// from unrelated types that share nothing but coordinates, and the picture
/// looks like a graph rather than like a mistake.
///
/// The gap is [`TYPE_GUTTER`], wide enough that the seam reads as a seam. This
/// separates the types; it does not lay them out together — cross-type edges
/// still pull on nothing, which is the later slice
/// (`.planning/W3-LAYOUT-PLAN.md` §5). Separated is wrong in a way a reader can
/// see and reason about; overlapped is wrong in a way that looks like data.
fn place_after(positions: &mut [(f32, f32)], origin_x: f32) -> f32 {
    let mut width = 0.0f32;
    for (x, _) in positions.iter_mut() {
        width = width.max(*x);
        *x += origin_x;
    }
    origin_x + width + TYPE_GUTTER
}

/// Escape a URL for embedding in a single-quoted `DuckDB` SQL string literal
/// (`read_parquet('…')` / `COPY … TO '…'` take literals, not bind params).
fn sql_lit(url: &str) -> String {
    url.replace('\'', "''")
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
        (a.0 - b.0).hypot(a.1 - b.1)
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

    #[test]
    fn shift_for_accepts_only_powers_of_two() {
        // The default tile, and the shift ADR-0045 §8 publishes for it.
        assert_eq!(shift_for(4_096), Some(12));
        assert_eq!(shift_for(2), Some(1));
        assert_eq!(shift_for(1), Some(0));
        // The retired chunk. It is refused rather than rounded: emitting under it
        // works and silently costs every reader a division and a table.
        assert_eq!(shift_for(122_880), None);
        assert_eq!(shift_for(0), None);
    }

    #[test]
    fn tile_prefix_is_the_adjacency_without_its_extension() {
        assert_eq!(
            tile_prefix("file:///c/edge/A_b_A/by_source.parquet"),
            "file:///c/edge/A_b_A/by_source/"
        );
        assert_eq!(tile_prefix("s3://b/by_source.parquet"), "s3://b/by_source/");
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

// ──────────────────────────────────────────────────────────────────────────
// W3.2 — the community hierarchy the LOD pyramid is built from.
// ──────────────────────────────────────────────────────────────────────────

/// Modularity-based community detection, returning the **whole hierarchy**
/// rather than one partition.
///
/// This is what [`weakly_connected_components`] cannot give. WCC is a
/// reachability partition, so on a connected graph it answers "one community"
/// — measured on a 5M-vertex benchmark corpus it put 1,998 of 2,000 vertices in
/// a single cluster, and a viewport window then retained **375 of 27,244
/// incident edges (1.4%)**, barely four times chance. Positions derived from
/// that partition are topology-blind, and a spatial index over topology-blind
/// positions is fast access to noise.
///
/// The hierarchy is the point, not a by-product: level *l* is a graph whose
/// nodes are level *l−1*'s communities, which is exactly the quotient graph a
/// level-of-detail pyramid needs. `contract` stops being a query verb and
/// becomes how a level is built.
///
/// # What this is and is not
///
/// This is **Louvain** (local moving + aggregation, maximising modularity), not
/// Leiden. Leiden adds a *refinement* pass that guarantees every community is
/// internally well-connected; Louvain can emit communities that are internally
/// disconnected, which for a viewport shows up as a "community" whose members
/// sit together on screen while having no path between them. Naming it honestly
/// matters more than claiming the better algorithm: the refinement pass is a
/// separate slice, and it slots in between the local-moving loop and the
/// aggregation below without changing this signature.
///
/// Deterministic: nodes are visited in index order and ties are broken by lower
/// community id, so the same input yields the same hierarchy on every run — no
/// RNG, like everything else in this module.
///
/// # Returns
///
/// One entry per level. `levels[0][v]` is the community of original vertex `v`;
/// `levels[l][c]` is the parent community of level-`l` community `c`. Community
/// ids at every level are dense (`0..k`). The vector is empty for an empty
/// graph, and stops as soon as a pass merges nothing.
#[must_use]
pub fn community_hierarchy(vertex_count: u32, edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    if vertex_count == 0 {
        return Vec::new();
    }
    hierarchy(Weighted::from_edges(vertex_count, edges))
}

/// [`community_hierarchy`] over a graph that is already built, which is how the
/// write path enters: it reads the two orientations the artefact stores and has
/// no bag of pairs to hand over.
fn hierarchy(mut graph: Weighted) -> Vec<Vec<u32>> {
    let mut levels: Vec<Vec<u32>> = Vec::new();
    loop {
        let membership = local_moving(&graph);
        let community_count = membership.iter().copied().max().map_or(0, |m| m + 1);
        // A pass that merges nothing is where the hierarchy ends: recording it
        // would add a level that is a copy of the one below.
        if community_count as usize == graph.node_count() {
            break;
        }
        graph = graph.contract(&membership, community_count);
        levels.push(membership);
        if community_count <= 1 {
            break;
        }
    }
    levels
}

/// Collapse a hierarchy to one community per vertex, taking the **finest level
/// that fits `budget`**.
///
/// A hierarchy has no single "the" partition, so something has to choose, and
/// the choice is not free in either direction. Too coarse and a community is
/// larger than any window, so grouping by it buys the viewport nothing. Too fine
/// and `viewport`'s aggregate mode, which answers one super-node per cluster
/// under a `LIMIT`, starts dropping communities off the end of the list rather
/// than reporting that it did — see [`CLUSTER_BUDGET`].
///
/// Levels compose by lookup, not by recomputation: level *l* is indexed by level
/// *l−1*'s community ids, so walking up is `c ← levels[l][c]`. The walk stops at
/// the first level within budget, and at the top level regardless — the coarsest
/// level is the smallest there is, so if it still exceeds the budget there is
/// nothing better to return.
///
/// An empty hierarchy means nothing merged (a graph with no edges), and then
/// every vertex is its own community, which is the truth about that graph.
/// Returns the membership and **which level it came from**, because the level is
/// what the placement above it still needs in order to know who is whose sibling.
#[must_use]
fn flatten_to_budget(
    levels: &[Vec<u32>],
    vertex_count: u32,
    budget: u32,
) -> (Vec<u32>, Option<usize>) {
    let Some(finest) = levels.first() else {
        return ((0..vertex_count).collect(), None);
    };
    let count = |m: &[u32]| m.iter().copied().max().map_or(0, |x| x + 1);

    let mut current = finest.clone();
    let mut chosen = 0;
    for level in &levels[1..] {
        if count(&current) <= budget {
            break;
        }
        current = current.iter().map(|&c| level[c as usize]).collect();
        chosen += 1;
    }
    (current, Some(chosen))
}

/// Renumber the chosen level's communities so that **siblings are consecutive**,
/// by sorting each community on the path of ancestors above it.
///
/// The number a community wears decides where [`cluster_layout`] puts it, and
/// until now that number came from the order vertices happened to be visited in
/// — which is `dense_id` order, which is IRI order, which has nothing to do with
/// the graph. Two communities with thousands of edges between them therefore
/// landed on opposite sides of the picture as often as not, and every one of
/// those edges left whatever window either of them was in.
///
/// Sorting by the ancestor path *is* a depth-first walk of the hierarchy, so no
/// tree is built to do it: level *l+1* maps a community to its parent, and
/// following that up to the top gives a key whose lexicographic order visits
/// each subtree contiguously. The community's own id goes last so the order is
/// total, and therefore reproducible.
///
/// This orders the communities. It does not lay out each level's quotient graph
/// — a community's *position among its siblings* is still its id and not its
/// connections, so this buys locality between subtrees, not within one.
fn order_by_hierarchy(levels: &[Vec<u32>], chosen: usize, membership: &mut [u32]) {
    let cluster_count = membership.iter().copied().max().map_or(0, |m| m + 1);
    if cluster_count == 0 {
        return;
    }
    let above = &levels[chosen + 1..];

    let mut keyed: Vec<(Vec<u32>, u32)> = (0..cluster_count)
        .map(|c| {
            let mut path = Vec::with_capacity(above.len() + 1);
            let mut current = c;
            for level in above {
                current = level[current as usize];
                path.push(current);
            }
            // Coarsest ancestor first, so the sort groups whole subtrees before
            // it ever looks at a finer distinction.
            path.reverse();
            path.push(c);
            (path, c)
        })
        .collect();
    keyed.sort_unstable();

    let mut rank = vec![0u32; cluster_count as usize];
    for (r, (_, c)) in keyed.iter().enumerate() {
        rank[*c as usize] = r as u32;
    }
    for m in membership.iter_mut() {
        *m = rank[*m as usize];
    }
}

/// Split a Morton code back into the two coordinates [`morton2`] interleaved.
///
/// Used to walk the cluster grid in Z-order rather than row by row. Row-major
/// numbering makes a run of consecutive clusters into a horizontal strip that
/// wraps at the edge, so a parent's children end up spread across a row and
/// broken over two; Z-order keeps a consecutive run inside a compact block, and
/// consecutive is exactly what [`order_by_hierarchy`] arranges for siblings.
const fn morton_decode(code: u32) -> (u32, u32) {
    const fn compact(n: u32) -> u32 {
        let mut n = n & 0x5555_5555;
        n = (n | (n >> 1)) & 0x3333_3333;
        n = (n | (n >> 2)) & 0x0f0f_0f0f;
        n = (n | (n >> 4)) & 0x00ff_00ff;
        (n | (n >> 8)) & 0x0000_ffff
    }
    (compact(code), compact(code >> 1))
}

/// What a half-edge weighs.
///
/// Level 0 is the graph the artefact stores and every edge there weighs one, so
/// the array is not stored at all; only [`Weighted::contract`] sums weights and
/// therefore has to keep them. Measured at ten million, the `f64` per half-edge
/// was 1,136 MB — 16 of the 53 B/edge the core costs (ADR-0043 §1).
enum Weights {
    Unit,
    Stored(Vec<f64>),
}

impl Weights {
    fn at(&self, i: usize) -> f64 {
        match self {
            Self::Unit => 1.0,
            Self::Stored(w) => w[i],
        }
    }

    /// Total weight over `range`, which for [`Self::Unit`] is its length: the
    /// sum of `k` ones is exactly `k` in `f64`, so this is the same number the
    /// stored variant produces and not an approximation of it.
    fn sum(&self, range: std::ops::Range<usize>) -> f64 {
        match self {
            Self::Unit => range.len() as f64,
            Self::Stored(w) => w[range].iter().sum(),
        }
    }
}

/// One orientation of an adjacency in compressed row form: vertex `v`'s
/// neighbours under this orientation are `targets[offsets[v]..offsets[v + 1]]`.
struct Csr {
    offsets: Vec<usize>,
    targets: Vec<u32>,
    weights: Weights,
}

impl Csr {
    fn range(&self, v: usize) -> std::ops::Range<usize> {
        self.offsets[v]..self.offsets[v + 1]
    }

    fn neighbours(&self, v: usize) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.range(v).map(|i| (self.targets[i], self.weights.at(i)))
    }
}

/// Build a [`Csr`] from rows that already arrive grouped by their key.
///
/// A run of equal keys **is** a vertex's neighbour list, so the offsets are
/// written as the runs close and nothing is counted twice or written out of
/// place. That is the whole of ADR-0043 stage 1: the degree pre-pass and the
/// cursor scatter [`Weighted::from_edges`] needs exist only because its
/// parameter is an unordered bag, and the file on disk has never been one.
struct CsrBuilder {
    n: usize,
    offsets: Vec<usize>,
    targets: Vec<u32>,
    /// The vertex whose run is open. Every key seen so far is `<=` it, which is
    /// what makes a file that lies about its order detectable in one comparison.
    open: usize,
}

impl CsrBuilder {
    fn new(n: usize, edges: usize) -> Self {
        let mut offsets = Vec::with_capacity(n + 1);
        offsets.push(0);
        Self {
            n,
            offsets,
            targets: Vec::with_capacity(edges),
            open: 0,
        }
    }

    /// One Arrow batch, as the two columns it already is. `false` means the keys
    /// went backwards, i.e. the file is not what it declares itself to be.
    fn push(&mut self, keys: &[u32], values: &[u32], self_loops: &mut [f64]) -> bool {
        for (&key, &value) in keys.iter().zip(values) {
            let (key, value) = (key as usize, value as usize);
            if key < self.open {
                return false;
            }
            // Keys ascend, so the first one past the last vertex ends the useful
            // part of the file. A clean writer emits none of these.
            if key >= self.n {
                break;
            }
            while self.open < key {
                self.open += 1;
                self.offsets.push(self.targets.len());
            }
            if value == key {
                self_loops[key] += 1.0;
            } else if value < self.n {
                self.targets.push(value as u32);
            }
        }
        true
    }

    fn finish(mut self) -> Csr {
        while self.open < self.n {
            self.open += 1;
            self.offsets.push(self.targets.len());
        }
        Csr {
            offsets: self.offsets,
            targets: self.targets,
            weights: Weights::Unit,
        }
    }
}

/// An undirected weighted graph, as however many oriented adjacencies it was
/// read from, with self-loops kept apart.
///
/// `sides` is a list rather than one array because that is what the artefact
/// hands over: a source-ordered file and a target-ordered file per edge table,
/// each already grouped by the endpoint it is ordered on. Merging them into one
/// adjacency would be a copy of the whole graph to buy nothing — a vertex's
/// neighbourhood is the concatenation of its run in each. A contraction builds
/// one symmetric side and so has a list of one.
///
/// Self-loops are separate because aggregation creates them — a community's
/// internal edges become one — and because they enter the degree twice while
/// appearing once in the adjacency. Folding them into `targets` would make every
/// later sum quietly wrong by a factor of two.
struct Weighted {
    sides: Vec<Csr>,
    /// Weight of each node's self-loop, counted **once**.
    self_loops: Vec<f64>,
    /// Sum of incident weights plus twice the self-loop — the `k_i` of the
    /// modularity formula.
    degrees: Vec<f64>,
    /// Total edge weight `m`, i.e. half the sum of all degrees.
    total: f64,
}

impl Weighted {
    const fn node_count(&self) -> usize {
        self.self_loops.len()
    }

    /// Build from an unweighted, possibly duplicated edge list. Parallel edges
    /// add their weights rather than being deduplicated: two links between the
    /// same pair really are a stronger tie, and modularity is defined over
    /// weights.
    ///
    /// This is what an unordered bag costs — a degree pass, a prefix sum and a
    /// scatter through a cloned cursor — and it is kept for callers that have
    /// one, which since ADR-0043 stage 1 means the tests and
    /// `examples/layout_memory.rs`. The write path reads [`Csr`]s instead.
    fn from_edges(vertex_count: u32, edges: &[(u32, u32)]) -> Self {
        let n = vertex_count as usize;
        let mut degree_count = vec![0usize; n];
        for &(a, b) in edges {
            if (a as usize) < n && (b as usize) < n && a != b {
                degree_count[a as usize] += 1;
                degree_count[b as usize] += 1;
            }
        }
        let mut offsets = Vec::with_capacity(n + 1);
        let mut acc = 0usize;
        offsets.push(0);
        for d in &degree_count {
            acc += *d;
            offsets.push(acc);
        }
        let mut cursor = offsets.clone();
        let mut targets = vec![0u32; acc];
        let mut self_loops = vec![0.0f64; n];
        for &(a, b) in edges {
            if (a as usize) >= n || (b as usize) >= n {
                continue;
            }
            if a == b {
                self_loops[a as usize] += 1.0;
                continue;
            }
            targets[cursor[a as usize]] = b;
            cursor[a as usize] += 1;
            targets[cursor[b as usize]] = a;
            cursor[b as usize] += 1;
        }
        Self::finish(
            vec![Csr {
                offsets,
                targets,
                weights: Weights::Unit,
            }],
            self_loops,
        )
    }

    fn finish(sides: Vec<Csr>, self_loops: Vec<f64>) -> Self {
        let n = self_loops.len();
        let mut degrees = vec![0.0f64; n];
        for v in 0..n {
            let incident: f64 = sides.iter().map(|s| s.weights.sum(s.range(v))).sum();
            degrees[v] = 2.0f64.mul_add(self_loops[v], incident);
        }
        let total = degrees.iter().sum::<f64>() / 2.0;
        Self {
            sides,
            self_loops,
            degrees,
            total,
        }
    }

    fn neighbours(&self, v: usize) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.sides.iter().flat_map(move |side| side.neighbours(v))
    }

    /// The quotient graph: one node per community, intra-community weight
    /// folded into a self-loop, inter-community weight summed.
    fn contract(&self, membership: &[u32], community_count: u32) -> Self {
        let k = community_count as usize;
        let mut acc: Vec<std::collections::HashMap<u32, f64>> =
            vec![std::collections::HashMap::new(); k];
        let mut self_loops = vec![0.0f64; k];
        for v in 0..self.node_count() {
            let cv = membership[v];
            // Each node's own self-loop carries over whole.
            self_loops[cv as usize] += self.self_loops[v];
            for (u, w) in self.neighbours(v) {
                let cu = membership[u as usize];
                if cu == cv {
                    // Counted once per direction, so half lands here and half
                    // when the other endpoint is visited.
                    self_loops[cv as usize] += w / 2.0;
                } else {
                    *acc[cv as usize].entry(cu).or_insert(0.0) += w;
                }
            }
        }
        let mut offsets = Vec::with_capacity(k + 1);
        let mut targets = Vec::new();
        let mut weights = Vec::new();
        offsets.push(0);
        for row in &acc {
            // Sorted so the structure is a pure function of the input, not of
            // hash iteration order.
            let mut entries: Vec<(u32, f64)> = row.iter().map(|(c, w)| (*c, *w)).collect();
            entries.sort_unstable_by_key(|(c, _)| *c);
            for (c, w) in entries {
                targets.push(c);
                weights.push(w);
            }
            offsets.push(targets.len());
        }
        Self::finish(
            vec![Csr {
                offsets,
                targets,
                weights: Weights::Stored(weights),
            }],
            self_loops,
        )
    }
}

/// One Louvain local-moving pass: repeatedly move each node to the neighbouring
/// community that most increases modularity, until a sweep moves nothing.
///
/// The gain of moving isolated node `i` into community `C` is proportional to
/// `k_i_in - Σ_tot(C)·k_i / (2m)`; the constant factor is the same for every
/// candidate, so only the comparison matters and it is never divided out.
fn local_moving(graph: &Weighted) -> Vec<u32> {
    let n = graph.node_count();
    let mut community: Vec<u32> = (0..n as u32).collect();
    if graph.total <= 0.0 {
        // No edges: every node is its own community and nothing can improve.
        return community;
    }
    // Σ_tot per community — the sum of degrees of its members.
    let mut totals: Vec<f64> = graph.degrees.clone();
    let two_m = 2.0 * graph.total;

    let mut moved = true;
    let mut sweeps = 0;
    // Bounded because a pathological tie could otherwise oscillate; Louvain
    // converges in a handful of sweeps in practice.
    while moved && sweeps < 32 {
        moved = false;
        sweeps += 1;
        for v in 0..n {
            let own = community[v];
            let k_v = graph.degrees[v];
            // Weight from v into each neighbouring community.
            let mut into: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
            for (u, w) in graph.neighbours(v) {
                *into.entry(community[u as usize]).or_insert(0.0) += w;
            }
            // Remove v from its community before comparing, so staying put is
            // evaluated on the same footing as moving.
            totals[own as usize] -= k_v;

            let mut best = own;
            let mut best_gain =
                totals[own as usize].mul_add(-k_v / two_m, into.get(&own).copied().unwrap_or(0.0));
            let mut candidates: Vec<(u32, f64)> = into.iter().map(|(c, w)| (*c, *w)).collect();
            candidates.sort_unstable_by_key(|(c, _)| *c);
            for (c, w_in) in candidates {
                if c == own {
                    continue;
                }
                let gain = totals[c as usize].mul_add(-k_v / two_m, w_in);
                // Strictly greater keeps the lower community id on a tie, which
                // is what makes the result reproducible.
                if gain > best_gain {
                    best_gain = gain;
                    best = c;
                }
            }
            totals[best as usize] += k_v;
            if best != own {
                community[v] = best;
                moved = true;
            }
        }
    }
    densify(&mut community);
    community
}

/// Relabel community ids to `0..k` in order of first appearance, so every level
/// hands the next one a dense node range.
fn densify(community: &mut [u32]) {
    let mut seen: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for c in community.iter_mut() {
        let next = seen.len() as u32;
        *c = *seen.entry(*c).or_insert(next);
    }
}

#[cfg(test)]
mod hierarchy_tests {
    use super::*;

    /// Two cliques joined by a single edge is the canonical case: modularity
    /// must find the two cliques, where WCC finds one component.
    #[test]
    fn two_cliques_joined_by_a_bridge_become_two_communities() {
        let mut edges = Vec::new();
        for a in 0..4u32 {
            for b in (a + 1)..4 {
                edges.push((a, b));
            }
        }
        for a in 4..8u32 {
            for b in (a + 1)..8 {
                edges.push((a, b));
            }
        }
        edges.push((0, 4)); // the bridge

        // What we are measured against: reachability says "one".
        let wcc = weakly_connected_components(8, &edges);
        assert_eq!(wcc.iter().copied().max(), Some(0), "the graph is connected");

        let levels = community_hierarchy(8, &edges);
        assert!(!levels.is_empty(), "a bridged pair of cliques must split");
        let first = &levels[0];
        assert_eq!(first.len(), 8);
        for v in 1..4 {
            assert_eq!(first[v], first[0], "clique A stays together");
        }
        for v in 5..8 {
            assert_eq!(first[v], first[4], "clique B stays together");
        }
        assert_ne!(first[0], first[4], "the two cliques are not one community");
    }

    /// Every level's ids are dense and every level indexes the one below, or the
    /// pyramid cannot be walked.
    #[test]
    fn levels_are_dense_and_stack() {
        let mut edges = Vec::new();
        // Four cliques of four, chained by single bridges.
        for c in 0..4u32 {
            let base = c * 4;
            for a in 0..4u32 {
                for b in (a + 1)..4 {
                    edges.push((base + a, base + b));
                }
            }
            if c > 0 {
                edges.push((base, base - 4));
            }
        }
        let levels = community_hierarchy(16, &edges);
        assert!(!levels.is_empty());
        assert_eq!(levels[0].len(), 16, "level 0 is indexed by vertex");
        for (l, level) in levels.iter().enumerate() {
            let count = level.iter().copied().max().map_or(0, |m| m + 1) as usize;
            let mut seen = vec![false; count];
            for &c in level {
                seen[c as usize] = true;
            }
            assert!(seen.iter().all(|s| *s), "level {l} ids must be dense");
            if let Some(next) = levels.get(l + 1) {
                assert_eq!(
                    next.len(),
                    count,
                    "level {} indexes level {l}'s output",
                    l + 1
                );
            }
        }
    }

    /// Determinism is a promise this module makes everywhere else, so it is
    /// tested rather than assumed.
    #[test]
    fn the_same_graph_gives_the_same_hierarchy() {
        let edges: Vec<(u32, u32)> = (0..40u32).map(|i| (i % 10, (i * 7) % 10)).collect();
        assert_eq!(
            community_hierarchy(10, &edges),
            community_hierarchy(10, &edges)
        );
    }

    /// The budget picks a level, and picking is the whole job: a hierarchy has
    /// no single "the" partition.
    #[test]
    fn flatten_takes_the_finest_level_that_fits() {
        // Six vertices → three pairs → one community. Levels are stated rather
        // than computed so the test is about the choosing, not about Louvain.
        let levels = vec![vec![0, 0, 1, 1, 2, 2], vec![0, 0, 0]];

        assert_eq!(
            flatten_to_budget(&levels, 6, 3),
            (vec![0, 0, 1, 1, 2, 2], Some(0)),
            "three communities fit a budget of three, so the finest level wins",
        );
        assert_eq!(
            flatten_to_budget(&levels, 6, 2),
            (vec![0, 0, 0, 0, 0, 0], Some(1)),
            "over budget, the walk composes up a level",
        );
        assert_eq!(
            flatten_to_budget(&levels, 6, 1),
            (vec![0, 0, 0, 0, 0, 0], Some(1)),
            "a budget nothing satisfies still stops at the coarsest level there is",
        );
    }

    /// A graph with no edges has no hierarchy, and then every vertex is its own
    /// community — which is the truth about that graph, not a fallback.
    #[test]
    fn flatten_without_a_hierarchy_isolates_every_vertex() {
        assert_eq!(flatten_to_budget(&[], 4, 2_048), (vec![0, 1, 2, 3], None));
    }

    /// The number a community wears decides where it is drawn, so siblings have
    /// to be consecutive — otherwise two communities with thousands of edges
    /// between them land on opposite sides of the picture as often as not.
    #[test]
    fn ordering_puts_siblings_next_to_each_other() {
        // Six communities under two parents, interleaved on purpose: the odd
        // ones belong to parent 0 and the even ones to parent 1, which is the
        // arrangement id order gets wrong.
        let levels = vec![Vec::new(), vec![1, 0, 1, 0, 1, 0]];
        let mut membership: Vec<u32> = vec![0, 1, 2, 3, 4, 5];
        order_by_hierarchy(&levels, 0, &mut membership);

        let parent = |c: u32| levels[1][c as usize];
        let mut by_rank: Vec<(u32, u32)> = (0..6).map(|c| (membership[c as usize], c)).collect();
        by_rank.sort_unstable();
        let families: Vec<u32> = by_rank.iter().map(|&(_, c)| parent(c)).collect();

        // Each family occupies one contiguous run, so the sequence changes hands
        // exactly once.
        let switches = families.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(
            switches, 1,
            "families must not be interleaved: {families:?}"
        );
    }

    /// Z-order is what turns "consecutive" into "nearby" on the grid: the first
    /// four cells are a 2x2 block, where row-major would be a 4x1 strip.
    #[test]
    fn morton_decode_walks_the_grid_in_blocks() {
        assert_eq!(morton_decode(0), (0, 0));
        assert_eq!(morton_decode(1), (1, 0));
        assert_eq!(morton_decode(2), (0, 1));
        assert_eq!(morton_decode(3), (1, 1));
        // And it is the exact inverse of the interleave the row order uses.
        for code in 0..64u32 {
            let (x, y) = morton_decode(code);
            assert_eq!(morton2(x as u16, y as u16), code);
        }
    }

    /// One orientation of `edges` as the writer emits it — keyed, sorted, then
    /// pushed through the very builder `read_orientation` feeds from Arrow.
    fn side(
        n: u32,
        edges: &[(u32, u32)],
        key: impl Fn(&(u32, u32)) -> (u32, u32),
        self_loops: &mut [f64],
    ) -> Csr {
        let mut rows: Vec<(u32, u32)> = edges.iter().map(key).collect();
        rows.sort_unstable();
        let (keys, values): (Vec<u32>, Vec<u32>) = rows.into_iter().unzip();
        let mut builder = CsrBuilder::new(n as usize, keys.len());
        assert!(builder.push(&keys, &values, self_loops));
        builder.finish()
    }

    /// The claim the whole of ADR-0043 stage 1 rests on: reading the two
    /// orientations the artefact already stores builds the **same graph** as
    /// handing the same edges over as an unordered bag. Exactly the same, not
    /// nearly — modularity is defined over sums, every weight at level 0 is one,
    /// and a sum of ones is exact in `f64`, so the two hierarchies are compared
    /// whole rather than by some tolerance.
    #[test]
    fn the_orientations_on_disk_and_the_bag_are_one_graph() {
        const N: u32 = 24;
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for c in 0..4u32 {
            let base = c * 6;
            for a in 0..6u32 {
                for b in (a + 1)..6 {
                    edges.push((base + a, base + b));
                }
            }
            if c > 0 {
                edges.push((base, base - 6));
            }
        }
        edges.push((7, 7)); // a self-loop, which is a row in *both* files

        let mut self_loops = vec![0.0f64; N as usize];
        let sides = vec![
            side(N, &edges, |&(a, b)| (a, b), &mut self_loops),
            side(N, &edges, |&(a, b)| (b, a), &mut self_loops),
        ];
        for count in &mut self_loops {
            *count /= 2.0;
        }

        assert_eq!(
            community_hierarchy(N, &edges),
            hierarchy(Weighted::finish(sides, self_loops)),
            "the CSR on disk and the bag in memory are the same graph",
        );
    }

    /// A file that is not in the order it declares does not fail, it builds a
    /// different graph — so the builder refuses it rather than believing it.
    #[test]
    fn a_key_that_goes_backwards_is_refused() {
        let mut self_loops = vec![0.0f64; 4];
        let mut builder = CsrBuilder::new(4, 3);
        assert!(builder.push(&[0, 2], &[1, 3], &mut self_loops));
        assert!(!builder.push(&[1], &[0], &mut self_loops));
    }

    /// Vertices with no edges are the common case at both ends of the range, and
    /// the file says nothing about them. Their offsets still have to be written
    /// — the ones it skips over and the tail it stops before — or one vertex's
    /// neighbour list reads off into another's.
    #[test]
    fn the_builder_fills_the_vertices_the_file_never_mentions() {
        let mut self_loops = vec![0.0f64; 5];
        let mut builder = CsrBuilder::new(5, 2);
        assert!(builder.push(&[1, 1], &[0, 3], &mut self_loops));
        let csr = builder.finish();
        assert_eq!(csr.offsets, vec![0, 0, 2, 2, 2, 2]);
        assert_eq!(csr.targets, vec![0, 3]);
    }

    /// Degenerate shapes must not panic or invent levels.
    #[test]
    fn empty_and_edgeless_graphs_have_no_hierarchy() {
        assert!(community_hierarchy(0, &[]).is_empty());
        assert!(
            community_hierarchy(5, &[]).is_empty(),
            "with no edges nothing merges, so there is no level to record"
        );
    }
}
