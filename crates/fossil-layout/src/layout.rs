//! Graph layout precompute — pure, host-agnostic algorithms, and the pass that
//! applies them to a written corpus.
//!
//! The writer emits `x`/`y`/`cluster_id` as placeholders (`0`); this fills them,
//! so a reader addressing the corpus gets meaningful positions and the Parquet
//! can be morton-sorted for predicate pushdown.
//!
//! The pure half, over a `dense_id` edge list:
//!
//! - [`community_hierarchy`] — modularity communities, and the whole hierarchy
//!   of them, which is what [`enrich_layout`] partitions by. This is the pyramid
//!   the level-of-detail plan is built from.
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
//! Each is pure (no I/O, no RNG, no engine) so they unit-test in isolation and
//! [`enrich_layout`] can wire them with confidence.
//!
//! And so is the second half of this file. `enrich_layout` reads and writes
//! Parquet — through `parquet-rs` and `arrow-rs`, and through the one encoder the
//! writer itself uses. There is no database here: see
//! `docs/design/one-engine.mdx`.

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
/// Not an aesthetic choice: a caller that draws the graph aggregates one
/// super-node per `(type_idx, cluster_id)`, and every read path out of
/// `fossil-graph` is row-capped — `ExecuteSqlParams::row_cap` defaults to
/// 10,000 and the executor applies an outer `LIMIT` whatever the SQL says. A
/// cap truncates, it does not degrade: a partition finer than the cap makes
/// the picture silently lose whole communities rather than coarsen. A budget
/// of 2,048 stays under 10,000 for up to four vertex types.
const CLUSTER_BUDGET: u32 = 2_048;
/// Intra-cluster packing radius scale (kept well below `CLUSTER_SPACING` so
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
/// by [`community_hierarchy`], `flatten_to_budget` and `order_by_hierarchy`).
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

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Float32Array, RecordBatch, RecordBatchReader, StringArray, UInt32Array,
};
use arrow::compute::{cast, interleave_record_batch};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::error::ArrowError;
use fossil_df::files::TileWriter;
use fossil_mem_probe::Probe;

use crate::io::{LayoutIo, LocalFs, Sink};
use fossil_sinks::manifest::{TILE_CODES_FILE, TILES_FILE};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// Rows per Arrow batch when scanning a Parquet column chunk. `parquet`'s own
/// default is 1,024, which at seventy million edges is seventy thousand trips
/// through the decoder for two `u32` columns; this is the same figure the
/// executor's default batch size uses and nothing here is sensitive to it
/// beyond that.
const SCAN_BATCH_ROWS: usize = 8_192;

/// `row_of_dense[d]` when no row of the vertex file carries `dense_id` `d`.
///
/// A sentinel and not an `Option<u32>`, because the array is one per vertex of
/// the largest type in the corpus and `Option` doubles it. `u32::MAX` is
/// unreachable as a row index for the same reason it is unreachable as a
/// `dense_id`: the count is a `u32` and the last one is `u32::MAX - 1` at worst.
const NO_ROW: u32 = u32::MAX;

/// One vertex type's layout target: its vertex Parquet URL plus the CSR Parquet
/// URLs of its **self-edges** (`src_type == dst_type == this type`), whose
/// `src_dense`/`dst_dense` live in this type's `dense_id` space. Cross-type
/// edges are excluded here — a global cross-type layout is a later slice.
///
/// URLs (not paths), and **local ones only**. The type is a URL because the
/// callers hold URLs and because the destination side of the language is
/// specified in them; what dereferences one here is `std::fs`, so a plain path
/// and `file://` are the two forms that resolve and anything with another scheme
/// is [`LayoutError::Remote`].
///
/// That is narrower than it reads, and it is narrower than the sentence this
/// comment replaced: the previous pass handed the URL verbatim to `read_parquet`
/// / `COPY … TO` and let an embedded engine's httpfs extension dereference it,
/// which described a cloud capability **no caller could reach** — the only
/// caller refuses a non-local destination several frames earlier. Reaching an
/// object store from here is registering one, not passing a string along, and it
/// is a decision rather than a translation.
#[derive(Debug, Clone)]
pub struct VertexLayoutTarget {
    /// Schema label, e.g. `"Person"` — what an [`AdjacencyTarget`] names to say
    /// which `dense_id` space each of its two endpoint columns lives in.
    pub type_name: String,
    /// Vertex Parquet URL (e.g. `file://…/vertex/Person.parquet`, `s3://…`).
    ///
    /// This is the writer's staged output and is **read, not written**: the
    /// enriched vertices are emitted under [`Self::chunk_prefix`], and the
    /// caller deletes this afterwards.
    pub vertex_parquet: String,
    /// Where the tiles go — the manifest's `prefix`, e.g. `…/vertex/Person/`,
    /// trailing separator included. The set is one Parquet there, named by
    /// `TILES_FILE`, and its row groups are the tiles.
    pub chunk_prefix: String,
    /// Rows per tile — the manifest's `chunk_size`. The manifest and the files
    /// have to agree, so this comes from whoever wrote the manifest rather than
    /// being a constant here.
    ///
    /// **A power of two, and refused otherwise.** Tile `k` is the `dense_id`
    /// range `[k·chunk_size, (k+1)·chunk_size)`, and the point of the range being
    /// fixed is that a reader finds it with `dense_id >> shift` instead of a
    /// division and a table. A size that is not a power of two
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
    /// Decoding a Parquet the pass reads — a vertex file, an adjacency, or one
    /// of the two orientations of a CSR.
    #[error("layout read of `{target}` failed: {source}")]
    Read {
        target: String,
        #[source]
        source: parquet::errors::ParquetError,
    },
    /// Encoding a Parquet the pass writes — a vertex tile, a rewritten
    /// adjacency, or an edge tile.
    #[error("layout write of `{target}` failed: {source}")]
    Write {
        target: String,
        #[source]
        source: parquet::errors::ParquetError,
    },
    /// Opening a file to read it, or putting the encoded bytes back.
    #[error("layout io on `{target}` failed: {source}")]
    Io {
        target: String,
        #[source]
        source: std::io::Error,
    },
    /// An Arrow kernel — the cast or the gather — refused what it was handed.
    /// Reachable only through a column whose type is not what the writer
    /// declares.
    #[error("layout arrow op on `{target}` failed: {source}")]
    Arrow {
        target: String,
        #[source]
        source: ArrowError,
    },
    /// A URL naming a scheme this pass does not dereference. See
    /// [`VertexLayoutTarget::vertex_parquet`]: local paths and `file://` are
    /// what `std::fs` resolves, and an object store is a registration rather
    /// than a string.
    #[error("layout reads and writes local paths; `{url}` names a scheme it cannot dereference")]
    Remote { url: String },
    /// A Parquet missing a column the rewrite replaces (`dense_id`, `x`, `y`,
    /// `cluster_id` on a vertex; `src_dense` / `dst_dense` on an adjacency).
    ///
    /// Its own variant rather than an Arrow error because it is a statement
    /// about the *corpus*: the file is well-formed Parquet and is not a
    /// `GraphAr` vertex or adjacency, which is a different thing to be told.
    #[error("`{target}` has no `{column}` column, so the layout has nothing to replace")]
    MissingColumn { target: String, column: String },
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
    /// An adjacency Parquet is not the two `u32` columns an adjacency is.
    ///
    /// The remap reads each orientation as one packed `u64` per row and never
    /// materialises it as a `RecordBatch`, which is what took the phase from
    /// +1.95 GiB to +0.55 at ten million — and a packed pair has nowhere to put
    /// a third column. The writer emits exactly `src_dense` and `dst_dense`
    /// (`fossil_df`'s `write_to_dir`) and the format says so, so this is a file
    /// that came from somewhere else; refusing it is the honest answer, where
    /// the alternative is dropping columns nobody declared and writing the
    /// result as if it were the same relation.
    #[error(
        "adjacency `{target}` is `{columns}`; the remap reads an adjacency as two `u32` columns, \
         `src_dense` and `dst_dense`"
    )]
    AdjacencyShape { target: String, columns: String },
    /// An adjacency the manifest declares `ordered: true` is not.
    ///
    /// Worth an error for the same reason as [`Self::DanglingEndpoint`]: the
    /// layout reads the file as the CSR it claims to be, so a file out of order
    /// does not fail — it builds a different graph, and lays the corpus out by
    /// it. Checked while streaming, for one comparison per row.
    #[error("adjacency `{target}` is not ordered by `{column}`, which the manifest claims it is")]
    Disordered { target: String, column: String },
    /// The run declared a memory budget smaller than what the pass will hold for
    /// a corpus of this shape — see [`estimated_peak_bytes`].
    ///
    /// **A refusal and not a degradation**, and the two are not interchangeable.
    /// This pass has no spill path: there is no disk manager under it and no
    /// operator in it that can be told no and give something back, so the two
    /// honest answers to "it does not fit" are *refuse* and *produce a different
    /// corpus*. The second one is unavailable here for a reason that is
    /// structural rather than squeamish — `fossil run` and the browser tab write
    /// the same tree byte for byte, and only one of them has a `--memory-gib` to
    /// declare, so a budget that changed the output would make the two paths
    /// disagree exactly when a budget was declared.
    ///
    /// So the budget decides **whether the pass runs**, never what it writes.
    ///
    /// # The message says *this stage*, because the number is per stage
    ///
    /// One declared `--memory-gib` is spent twice on a `fossil run` — once by the
    /// executor's `FairSpillPool`, once here — and neither half knows what the
    /// other took, so the write path is bounded at something closer to double the
    /// number. That is decided rather than outstanding: the flag bounds **each
    /// stage**, not their sum, and the two alternatives are refused on
    /// `/docs/design/streaming` with the measurements that refuse them. A message
    /// that said "the run declared" without saying which part of the run spends
    /// it would be the same silence that let `let _ = memory_bytes;` stand.
    #[error(
        "the layout pass needs about {} GiB for {vertex_count} vertices and {adjacency_rows} \
         adjacency rows, and the run declared {} GiB — `--memory-gib` bounds each stage of the \
         write path rather than their sum, and this is the layout's. Raise it, or omit it to run \
         unbounded",
        .needed_bytes / (1 << 30),
        .declared_bytes / (1 << 30)
    )]
    OverBudget {
        /// Rows across every vertex Parquet the pass was handed.
        vertex_count: u64,
        /// Rows across every adjacency Parquet, both orientations.
        adjacency_rows: u64,
        /// What [`estimated_peak_bytes`] says this corpus costs.
        needed_bytes: u64,
        /// The run's `--memory-gib`, in bytes.
        declared_bytes: u64,
    },
}

// ──────────────────────────────────────────────────────────────────────────
// The budget, in bytes
// ──────────────────────────────────────────────────────────────────────────

/// Bytes the pass holds per vertex, in the arrays it allocates.
///
/// | array | bytes per vertex |
/// | --- | --- |
/// | `dense_of_row` | 4 |
/// | two CSR `offsets` (`usize`) | 16 |
/// | `self_loops`, `degrees` (`f64`) | 16 |
/// | Louvain `community`, `totals`, `levels[0]` | 16 |
/// | [`Neighbourhood`], and [`Weighted::contract`]'s counting sort under it | 21 |
/// | `clusters`, `placement`, `positions`, `morton`, `new_ids`, `order` | 28 |
/// | `row_of_dense`, `gather`, `new_dense`, `xs`, `ys`, `cluster_ids` | 24 |
///
/// That is 125, and this is 128 — the remainder is the allocator's, and it is a
/// term rather than a rounding. `vec![0u32; n]` asks for `4n` and the OS hands
/// over whole pages of whichever size class holds them.
///
/// # Summed, and that is now an over-estimate rather than the reading
///
/// This used to say the sum **is** the peak, on the grounds that every delta
/// `FOSSIL_MEM_PROBE` printed for the pass was positive and none ever came back.
/// That was true of the code it described and is not true of this one: with the
/// array of hash tables gone from [`Weighted::contract`], the ten-million run
/// prints `flatten + order + place  −2.93G` — the level-zero quotient graph
/// being dropped, in pages the allocator does hand back. The resident set rises,
/// falls by three gigabytes, and rises again to its high-water mark in the last
/// phase.
///
/// So the sum is kept, and kept deliberately: it is now a bound on the maximum
/// rather than a reading of it, which is the direction
/// [`estimated_peak_bytes`] is calibrated in.
const VERTEX_ARRAY_BYTES: u64 = 128;

/// Bytes per adjacency row, counting each orientation's rows separately.
///
/// **The one constant here that is measured rather than derived, and it is the
/// one that decides the answer.** It bounds whichever phase holds the most per
/// adjacency row, and that phase has now moved twice. It was
/// `community_hierarchy` at 48, when one line of [`Weighted::contract`] built a
/// hash table per community. It was `remap adjacencies + write edge tiles` at
/// 14, the one step holding a whole orientation as Arrow — `concat_batches`,
/// `lexsort_to_indices` and a `take` — measuring 13.2 / 14.5 / 15.0 B/row at
/// two, four and ten million.
///
/// It is **`read CSR + CSC`** now, and that step is arithmetic rather than a
/// guess: one `u32` of `targets` per row plus one offset per vertex, and Louvain
/// holds it for the whole of its 127 s at ten million. Measured across the five
/// calibration fixtures it is **5.0 to 6.7 bytes per adjacency row**:
///
/// | fixture | adjacency rows | `read CSR + CSC` | B/row |
/// | --- | --- | --- | --- |
/// | 2,000,000 · degree 14 | 27,974,508 | +0.16 GiB | 6.1 |
/// | 4,000,000 · degree 6 | 23,978,362 | +0.15 GiB | 6.7 |
/// | 4,000,000 · degree 14 | 55,949,862 | +0.31 GiB | 6.0 |
/// | 4,000,000 · degree 28 | 111,899,830 | +0.52 GiB | 5.0 |
/// | 10,000,000 · degree 14 | 139,874,560 | +0.78 GiB | 6.0 |
///
/// The remap's own term is now 4 B/row — one `Vec<u64>` per orientation, which
/// is eight bytes for each of an orientation's rows and therefore four for each
/// of the rows counted here — and it is not live at the same time as the CSR.
/// Eight is the larger of the two, rounded up: the constant has to bound a peak,
/// and the peak holds one of them.
///
/// # Why it is not fitted on the pass's total any more
///
/// Because that regression has stopped being trustworthy in the direction that
/// matters. Holding V at four million and moving the degree, the pass measures
/// 1.07 / 1.23 / **1.17** GiB at degree 6 / 14 / 28 — it goes *down* at the
/// densest point, and a slope through those three is 1.2 B/row, below what
/// `read CSR + CSC` is measured to hold. The confound is in the fixture rather
/// than in the pass: `examples/enrich_memory` builds the corpus in the same
/// process, so the baseline the pass is measured from already contains the
/// generator's retained heap — 0.36, 0.72 and 1.01 GiB at those three degrees —
/// and the pass reuses those pages instead of asking for more. **`peak − start`
/// is biased low, and increasingly so with degree.** A per-phase delta taken
/// from inside the pass is not, which is why the constant is fitted on one.
///
/// What would let it be fitted on the total again is a measurement whose process
/// did not build the fixture — a second binary handed a corpus already on disk.
///
/// (`examples/enrich_memory <N> <degree>`, 2026-08-28, Mac16,8 — 14 cores,
/// 48 GiB, macOS 26.2 / Darwin 25.2.0.)
const ADJACENCY_ROW_BYTES: u64 = 8;

/// What the pass holds whatever the corpus is.
///
/// The three terms below are all proportional to something the corpus has, and
/// a corpus small enough makes all three of them small — while the Parquet
/// reader's `SCAN_BATCH_ROWS` batches and the writer's row-group buffers are the
/// size they are. Measured: a **ten-thousand**-vertex corpus holds 6.21 MB
/// against 4.00 MB of linear terms, and a sixty-thousand-vertex one holds 25.89
/// against 23.94, which puts the floor between two and five megabytes.
///
/// Sixteen mebibytes rather than five, and the reason is where the number is
/// used rather than where it was measured. At the small end this term **is** the
/// answer, and the run-to-run spread is a larger fraction of it than of anything
/// else here: the same sixty-thousand-vertex corpus holds 25.89 MB in a release
/// build and 30.10 in the debug build `cargo test` produces. A floor fitted to
/// the optimised build is a floor that fails on the guard.
///
/// Nothing at the sizes the rest of this file is calibrated on notices: it is
/// 0.4% of the ten-million estimate.
const LAYOUT_BASE_BYTES: u64 = 16 * 1024 * 1024;

/// What one uncompressed byte of a vertex Parquet costs once it is Arrow, in
/// thousandths.
///
/// The vertex file is the one input whose cost is **not** a function of the row
/// count: a row is a subject IRI and every property the mapping emitted, and a
/// corpus of four integer columns and a corpus of a dozen strings have the same
/// V. So this term reads the footer's `total_byte_size` — the uncompressed size
/// the file declares, free with the metadata — rather than pretending a row has
/// a width.
///
/// 1.30: at ten million the fixture's vertex Parquet declares 1,006 MiB
/// uncompressed and `read vertices` billed +1.28 GiB decoded.
const VERTEX_PAYLOAD_PERMILLE: u64 = 1_300;

/// What [`enrich_layout_within`] will hold, in bytes, for a corpus of this
/// shape — the terms above, summed: a floor the corpus does not change, one per
/// vertex, one per adjacency row, and one per uncompressed byte of the vertex
/// file.
///
/// **Every input is a footer read.** `vertex_count` and `adjacency_rows` are
/// `num_rows`, `vertex_payload_bytes` is the sum of the row groups'
/// `total_byte_size`, and all three are in the metadata a Parquet reader parses
/// before it decodes a single page. That is what makes a budget check something
/// the pass can afford to do *first*, at second zero, rather than discovering at
/// second two hundred that the machine cannot finish.
///
/// **Calibrated to over-estimate, on purpose.** An estimate that is a little too
/// large refuses a run that would have fitted, and the person who declared the
/// budget raises it and tries again; one that is a little too small accepts a
/// run and lets it exceed the number they were promised, which is the defect
/// this whole mechanism exists to remove. So it is fitted to the **largest** of
/// two runs of one build at every point, and it clears all five by 16% to 63%:
///
/// | fixture | adjacency rows | the pass | this returns | clears by |
/// | --- | --- | --- | --- | --- |
/// | 2,000,000 · degree 14 | 27,974,508 | 0.60 GiB | 0.74 GiB | +24% |
/// | 4,000,000 · degree 6 | 23,978,362 | 1.07 GiB | 1.25 GiB | +16% |
/// | 4,000,000 · degree 14 | 55,949,862 | 1.23 GiB | 1.49 GiB | +21% |
/// | 4,000,000 · degree 28 | 111,899,830 | 1.17 GiB | 1.90 GiB | +63% |
/// | 10,000,000 · degree 14 | 139,874,560 | 2.71 GiB | 3.69 GiB | +36% |
///
/// (`FOSSIL_MEM_PROBE=1 … --example enrich_memory -- <N> <degree>`, 2026-08-28,
/// Mac16,8 — 14 cores, 48 GiB, macOS 26.2 / Darwin 25.2.0.)
///
/// It is an estimate and it says so; it is a straight line, and a corpus far off
/// these five points is extrapolation. What it is *not* any more is a line
/// through one mean degree: three of the five rows share a vertex file and
/// differ only in how many edges hang off it, which is what
/// [`ADJACENCY_ROW_BYTES`] is now fitted on.
///
/// **Nor is it a guess about which phase dominates.** That is measured, and it
/// has moved twice — Louvain by more than every other phase together, then
/// `remap adjacencies + write edge tiles` once [`Weighted::contract`] stopped
/// building one hash table per community, and now **no one phase**: the peak is
/// `community_hierarchy`'s at ten million, `write identity index`'s at four
/// million and degree twenty-eight, and `remap adjacencies` in one of two
/// ten-million runs of one build. That is what a sum of terms is for. The
/// per-row term is fitted on the phase that holds the most per adjacency row,
/// which is `read CSR + CSC` — see [`ADJACENCY_ROW_BYTES`].
#[must_use]
pub const fn estimated_peak_bytes(
    vertex_count: u64,
    adjacency_rows: u64,
    vertex_payload_bytes: u64,
) -> u64 {
    LAYOUT_BASE_BYTES
        .saturating_add(VERTEX_ARRAY_BYTES.saturating_mul(vertex_count))
        .saturating_add(ADJACENCY_ROW_BYTES.saturating_mul(adjacency_rows))
        .saturating_add(vertex_payload_bytes.saturating_mul(VERTEX_PAYLOAD_PERMILLE) / 1_000)
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
/// `dense_id` and 6 of 41 by physical row order.
///
/// The obvious reading is that this means moving the layout ahead of the edge
/// phase. It
/// does not: this pass already runs last, with every adjacency already written
/// as Parquet. Renumbering after the fact is a gather through a mapping array,
/// which is strictly less invasive than reordering the phases.
///
/// # No engine
///
/// Everything below is `arrow-rs` and `parquet-rs`: the files are read with
/// `ParquetRecordBatchReaderBuilder` and written through
/// [`fossil_df::files::TileWriter`], the row-group container in the same module
/// as `batches_to_parquet`, which wrote the staged files being read. There was a
/// `DuckDB` connection here, and what it was used for was `COPY` — see
/// `docs/design/one-engine.mdx`.
///
/// The reason the substitution is small is that the relational work was never
/// relational. `dense_id` is a gapless `0..n`, so the join against the mapping
/// table is an array index; the new numbering is a permutation, so the `ORDER BY`
/// that sorts by it is that permutation applied as a gather; and a tile is a
/// contiguous range of the result, so the per-tile `WHERE` is a slice. One real
/// sort survives — the adjacency re-sort, which is a `sort_unstable` over one
/// packed `u64` per row; see [`read_adjacency`].
///
/// # What it does
///
/// Vertices first, all of them, because an adjacency spans two types and cannot
/// be rewritten until both mappings exist. Per type: count vertices, read
/// self-edges, run [`community_hierarchy`] + [`cluster_layout`], derive the
/// Morton rank of each vertex, and keep `dense_id → (new_dense_id, x, y,
/// cluster_id)` as four arrays. The enriched vertices are then emitted **as tiles** under
/// [`VertexLayoutTarget::chunk_prefix`] — one Parquet whose `k`th row group is
/// tile `k`, `chunk_size` rows each. The writer's staged output is the input to
/// this, is not written back, and is deleted by the caller.
///
/// Then every adjacency: both endpoints remapped through their own type's
/// mapping, and **re-sorted**, because the manifest declares `ordered: true` and
/// a CSR sorted on `src_dense` stops being sorted the moment those values change.
///
/// And finally every adjacency is emitted as tiles too, under
/// `by_source/tile{k}.parquet` and `by_target/tile{k}.parquet`, **keyed by the
/// same range as the vertices**: an edge lives in the tile of the endpoint its
/// file is ordered by, so `by_source` tile `k` holds every edge whose `src_dense`
/// is in vertex tile `k` of the source type, and `by_target` tile `k` every edge
/// whose `dst_dense` is in vertex tile `k` of the *destination* type. On a
/// cross-type edge those are two different `dense_id` spaces.
///
/// That is CSR/CSC, and it is the placement measured against the alternative of
/// hoisting an edge to the deepest tile holding both its endpoints — which reads
/// 2.29× to 15.86× more edges across 200k/1M/5M/10M against CSR's flat 1.95× to
/// 2.89×, and touches 2.5–3.5× the tiles. The mechanism is that near the root of
/// such a tree there is no branching left to prune with. CSR needs no second
/// request for the picture: every drawable edge has its source on screen, so the
/// tiles of the window are an exact superset of what can be drawn.
///
/// The target-ordered half is tiled because **drawing is not the only question**.
/// A hop is addressable exactly when both directions are: the out-edges of a
/// vertex are in the `by_source` tile its id falls in and the in-edges in the
/// `by_target` tile, so a neighbourhood is two reads and a filter and the work
/// follows the frontier rather than the corpus. Half the addressing gives half
/// the edges, which in a knowledge graph is a wrong answer and not a partial one.
///
/// # Errors
///
/// Returns [`LayoutError`] on the first failing read or write, on a URL naming a
/// scheme this pass cannot dereference, on an adjacency naming an unknown vertex
/// type, or on a renumbering that dropped rows.
pub fn enrich_layout(
    targets: &[VertexLayoutTarget],
    adjacencies: &[AdjacencyTarget],
) -> Result<(), LayoutError> {
    enrich_layout_with(&LocalFs, targets, adjacencies)
}

/// [`enrich_layout_with`], under the run's declared memory budget.
///
/// `memory_bytes` is `fossil run --memory-gib`, in bytes, and `None` is
/// unbounded — which is what every caller passed before this existed and what
/// the browser passes still, because a tab has no flag to declare one with.
///
/// **The budget decides whether the pass runs, never what it writes.** It is
/// checked once, before the first column chunk is decoded, against
/// [`estimated_peak_bytes`] over three numbers that come out of the Parquet
/// footers; over budget is [`LayoutError::OverBudget`] and the corpus is left
/// exactly as the writer staged it. Under budget the pass proceeds and emits
/// the tree it would have emitted unbounded, byte for byte — see
/// [`LayoutError::OverBudget`] for why degrading was not on the table.
///
/// This is a *declaration* honoured by refusing, not a pool honoured by
/// spilling. `fossil-df`'s half of the write path has a `FairSpillPool` and a
/// disk manager under it and can be told no mid-plan; this pass holds Rust
/// `Vec`s and has nowhere to put them. Told no, the only thing it can do that
/// is not a lie is stop.
///
/// # Errors
///
/// As [`enrich_layout`], plus [`LayoutError::OverBudget`].
pub fn enrich_layout_within(
    io: &dyn LayoutIo,
    targets: &[VertexLayoutTarget],
    adjacencies: &[AdjacencyTarget],
    memory_bytes: Option<u64>,
) -> Result<(), LayoutError> {
    if let Some(declared) = memory_bytes {
        let mut vertex_count = 0u64;
        let mut vertex_payload_bytes = 0u64;
        for target in targets {
            let (rows, bytes) = footprint(io, &target.vertex_parquet)?;
            vertex_count = vertex_count.saturating_add(rows);
            vertex_payload_bytes = vertex_payload_bytes.saturating_add(bytes);
        }
        let mut adjacency_rows = 0u64;
        for adjacency in adjacencies {
            let (rows, _) = footprint(io, &adjacency.parquet)?;
            adjacency_rows = adjacency_rows.saturating_add(rows);
        }
        let needed_bytes = estimated_peak_bytes(vertex_count, adjacency_rows, vertex_payload_bytes);
        if needed_bytes > declared {
            return Err(LayoutError::OverBudget {
                vertex_count,
                adjacency_rows,
                needed_bytes,
                declared_bytes: declared,
            });
        }
    }
    enrich_layout_with(io, targets, adjacencies)
}

/// Row count and uncompressed byte size of a Parquet, **from its footer alone**.
///
/// Both are in the metadata the reader parses before it touches a page, which is
/// what makes the budget check affordable at the top of the pass: a refusal
/// costs one footer per file and not one column chunk.
fn footprint(io: &dyn LayoutIo, url: &str) -> Result<(u64, u64), LayoutError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(io.open(url)?).map_err(read_err(url))?;
    let metadata = builder.metadata();
    let rows = metadata.file_metadata().num_rows().max(0) as u64;
    let bytes = metadata
        .row_groups()
        .iter()
        .map(|group| group.total_byte_size().max(0) as u64)
        .sum();
    Ok((rows, bytes))
}

/// [`enrich_layout`], against a filesystem the caller chooses.
///
/// The pass is arithmetic over Arrow and only a handful of its functions were
/// ever about files. This is that seam made a parameter, and the whole reason it
/// exists is that **a browser has no filesystem**: `fossil-df-wasm` hands the
/// corpus to JS as `{rel_path, bytes}` pairs, so the pass runs against
/// [`MemoryFs`](crate::io::MemoryFs) with the staged Parquet already in it and
/// the tiles coming back out the same way. `fossil run` and the tab then write
/// the same tree, rather than the same manifest over two different ones.
///
/// [`enrich_layout`] is this with [`LocalFs`], and is what every native caller
/// still calls.
///
/// # Errors
///
/// As [`enrich_layout`].
pub fn enrich_layout_with(
    io: &dyn LayoutIo,
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

    // `dense_id → new_dense_id` per vertex type, index-aligned with `targets`.
    //
    // A type with no vertices leaves an EMPTY map rather than no map, and that
    // is the same reason the mapping table this replaces was created even for an
    // empty type: an adjacency pointing into such a type must come out as a
    // reported dangling endpoint, not as a lookup with nothing to look in.
    let mut maps: Vec<Vec<u32>> = vec![Vec::new(); targets.len()];

    for (index, target) in targets.iter().enumerate() {
        let vurl = target.vertex_parquet.as_str();
        // Checked before anything is written, so a bad tile size is a refusal
        // rather than a corpus that has to be thrown away.
        let shift = shift_for(target.chunk_size).ok_or_else(|| LayoutError::TileSize {
            vertex_type: target.type_name.clone(),
            rows: target.chunk_size,
        })?;
        io.ensure_prefix(&target.chunk_prefix)?;

        // The `dense_id` column on its own, in file order, and two facts come
        // out of it that used to be two queries. The vertex count is `max + 1`
        // and not the row count — a corpus with a gap in its numbering must not
        // be told it has one fewer vertex than it numbers — and the position of
        // each id is what the join on `dense_id` was for.
        let dense_of_row = read_u32_column(io, vurl, "dense_id")?;
        let vertex_count = dense_of_row
            .iter()
            .copied()
            .max()
            .map_or(0, |m| m.saturating_add(1));
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
        // parameter was an unordered bag.
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
                io,
                csr,
                "src_dense",
                "dst_dense",
                vertex_count,
                &mut self_loops,
            )?);
            sides.push(read_orientation(
                io,
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
        // `cluster_id` is what a drawing reader colours and aggregates by, one
        // super-node per cluster under a row cap, so it must fit CLUSTER_BUDGET
        // — which on this corpus means the top of the hierarchy. Placement wants
        // the opposite: the finest level, whose communities are small enough
        // that several fit in one window. Using the budget partition for both was
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

        // The extent is taken out here rather than left inside `morton_codes`
        // because it is PUBLISHED now: it is half of what turns a rectangle in
        // the corpus's own coordinates into a code range, and a reader that
        // re-derives it from the written `x`/`y` gets it by reading every tile
        // back. Computed once, used for the codes and written beside them.
        let extent = extent_of(&positions);
        let morton = match extent {
            Some(extent) => morton_codes_within(&positions, extent),
            None => Vec::new(),
        };
        let (new_ids, order) = morton_ranks(&morton);
        probe.mark("morton codes + ranks");

        // The gather that replaces the staging table, the join and the ORDER BY.
        //
        // Row `p` of the enriched file is the vertex whose new id is `p`, and
        // `order[p]` says which old id that is; `row_of_dense` says which row of
        // the file carries it. `u32::MAX` marks an id no row has, and such an id
        // is SKIPPED rather than placed — the statement this replaces joined
        // inner, so a hole in the numbering dropped the id rather than
        // duplicating row zero into it.
        let mut row_of_dense = vec![NO_ROW; vertex_count as usize];
        for (row, &dense) in dense_of_row.iter().enumerate() {
            row_of_dense[dense as usize] = row as u32;
        }
        let rows = dense_of_row.len();
        drop(dense_of_row);

        let mut gather = Vec::with_capacity(rows);
        let mut new_dense = Vec::with_capacity(rows);
        let mut xs = Vec::with_capacity(rows);
        let mut ys = Vec::with_capacity(rows);
        let mut cluster_ids = Vec::with_capacity(rows);
        // The code of each row IN THE ORDER IT IS WRITTEN, which is the only
        // order the anchor can be cut on: `order` is the ranking over every
        // `dense_id`, and a `dense_id` no row carries is skipped just below, so
        // the k-th tile is the k-th slice of THIS list and not of `morton`.
        let mut row_codes = Vec::with_capacity(rows);
        for (new_id, &old) in order.iter().enumerate() {
            let row = row_of_dense[old as usize];
            if row == NO_ROW {
                continue;
            }
            let (x, y) = positions[old as usize];
            gather.push(row);
            new_dense.push(new_id as u32);
            xs.push(x);
            ys.push(y);
            cluster_ids.push(clusters[old as usize]);
            row_codes.push(morton[old as usize]);
        }
        drop(row_of_dense);

        // The vertices, every column, in file order — the read the staging table
        // used to be. It is the largest allocation this pass makes and it is
        // made *after* the community detection rather than before, which is the
        // order the staging did too: nothing about the partition needs a subject
        // IRI or a property, and holding the whole type through Louvain would
        // put the corpus beside the graph.
        //
        // **The batches are kept AS batches.** `concat_batches` into one
        // `RecordBatch` stood here, and it is a second full copy of the file
        // alive beside the first — and then `take_record_batch` over the whole
        // relation was a third. Measured on a one-million-vertex fixture
        // (`examples/enrich_memory`), that was `read vertices` +0.21 G and
        // `gather + replace` +0.11 G against a 117 MB file. Nothing needed the
        // relation to be contiguous: what follows wants ONE TILE at a time, and
        // `interleave_record_batch` gathers across batches, so the copy that was
        // made to enable a gather can be the tile the gather produces.
        let (_schema, batches) = read_parquet(io, vurl)?;
        let batch_refs: Vec<&RecordBatch> = batches.iter().collect();
        // Where each batch starts in file-row space, so a global row index —
        // which is what `gather` holds — becomes the `(batch, offset)` pair
        // `interleave` takes. `read_parquet` reads at a fixed batch size, but
        // this is computed rather than divided: the last batch is short, and a
        // reader that returns a different size is then a slower pass and not a
        // wrong one.
        let starts: Vec<u32> = batches
            .iter()
            .scan(0u32, |acc, b| {
                let start = *acc;
                *acc += u32::try_from(b.num_rows()).unwrap_or(u32::MAX);
                Some(start)
            })
            .collect();
        probe.mark("read vertices");

        // One row group per tile in ONE Parquet, which is the whole point: a
        // tile's address is `dense_id >> shift` — no index, no listing, no
        // discovery — and a run of consecutive tiles is a run of consecutive row
        // groups, which is ONE byte range. A file boundary is the only thing
        // that could prevent that merge, so there is not one: measured at five
        // million in 1,221 tiles, 5.6 range requests per window against 22.3 for
        // the same tiles as 1,221 files, and the same bytes stored.
        //
        // A range and not a filter. Ordering by the new id *is* ordering by
        // Morton code — that is what the new id is — and the ids are a gapless
        // `0..n`, so tile `k` is the row range `[k·size, (k+1)·size)` of the
        // permutation. The `WHERE dense_id >= lo AND dense_id < hi` that used to
        // stand here was a range predicate over a staging table whose row-group
        // statistics had to be made to prune it; the range is the slice of
        // `gather` itself and there is nothing left to prune.
        let rows = gather.len();
        let tiles = (rows as u64).div_ceil(target.chunk_size);
        let payload = format!("{}{TILES_FILE}", target.chunk_prefix);
        let mut writer = open_tiles(io, &payload, batch_refs[0].schema())?;
        for k in 0..tiles {
            let lo = (k << shift) as usize;
            let len = (rows - lo).min(target.chunk_size as usize);
            let picks: Vec<(usize, usize)> = gather[lo..lo + len]
                .iter()
                .map(|&row| locate(&starts, row))
                .collect();
            let tile = interleave_record_batch(&batch_refs, &picks).map_err(arrow_err(vurl))?;
            let enriched = replace_columns(
                &tile,
                vurl,
                &[
                    (
                        "dense_id",
                        Arc::new(UInt32Array::from(new_dense[lo..lo + len].to_vec())) as ArrayRef,
                    ),
                    ("x", Arc::new(Float32Array::from(xs[lo..lo + len].to_vec()))),
                    ("y", Arc::new(Float32Array::from(ys[lo..lo + len].to_vec()))),
                    (
                        "cluster_id",
                        Arc::new(UInt32Array::from(cluster_ids[lo..lo + len].to_vec())),
                    ),
                ],
            )?;
            writer.tile(&enriched).map_err(write_err(&payload))?;
        }
        writer.finish().map_err(write_err(&payload))?;
        probe.mark("gather + write vertex tiles");

        // The anchor, cut on the same loop bound the tiles were: `lo[k]` is the
        // code of tile `k`'s first row and `hi[k]` the code of its last. Both
        // are non-decreasing in `k` because `row_codes` is — the rows are in
        // rank order and rank order IS code order — which is what lets a reader
        // binary-search them.
        //
        // Written for a type with no rows too: a `tiles: 0` anchor says «this
        // type has no tiles», where a missing file says «this corpus was written
        // before the anchor existed». Those are not the same thing to be told,
        // and the manifest declares the path either way.
        if let Some(extent) = extent {
            let mut code_lo = Vec::with_capacity(tiles as usize);
            let mut code_hi = Vec::with_capacity(tiles as usize);
            for k in 0..tiles {
                let lo = (k << shift) as usize;
                let len = (rows - lo).min(target.chunk_size as usize);
                code_lo.push(row_codes[lo]);
                code_hi.push(row_codes[lo + len - 1]);
            }
            let anchor = format!("{}{TILE_CODES_FILE}", target.chunk_prefix);
            write_tile_codes(io, &anchor, extent, target.chunk_size, &code_lo, &code_hi)?;
            probe.mark("write tile codes");
        }

        // The identity index: the SAME rows a second time, ordered by `subject`
        // instead of by position, carrying only the identity and the address it
        // maps to.
        //
        // It cannot be a column of the tiles above and that is the whole reason
        // it is a second table: one table has one sort, this one's is Morton
        // because the spatial order IS the id space, and a lookup by identity
        // needs the other one. Without it `node(iri)` reads the `subject` column
        // of every tile of the type — the rows are in Morton order and subjects
        // are not, so no footer prunes — which at five million vertices is about
        // 40 MB per lookup.
        //
        // **Written here because the pass already holds everything it needs.**
        // `gather` is the permutation and `new_dense` the addresses it produced;
        // the subjects come out of the batches already in hand. A second pass
        // over the corpus would cost a full read, and this costs a sort.
        //
        // Rewritten in full on every relayout, which is affordable for exactly
        // the reason it is necessary: this pass already rewrites every tile.
        if let Some(subjects) = subject_pairs(&batch_refs, &starts, &gather, &new_dense, vurl)? {
            write_identity_index(io, &subjects, target)?;
            probe.mark("write identity index");
        }

        maps[index] = new_ids;
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

    // Every adjacency: both endpoints remapped through their own type's mapping,
    // **re-sorted** because the manifest declares `ordered: true` and a CSR
    // sorted on `src_dense` stops being sorted the instant those values are
    // replaced — with no error anywhere, because the column is still a perfectly
    // good `u32` — and then cut into tiles on the ranges of the endpoint it is
    // ordered by.
    //
    // **Both orientations, and the second one is not symmetry for its own sake.**
    // A hop is the question the source-ordered half cannot answer: the out-edges
    // of a vertex are in the `by_source` tile its `dense_id` falls in and its
    // in-edges in the `by_target` tile, so a neighbourhood is two addressed reads
    // and a filter. Following only the out-edges is a *wrong* answer rather than
    // a partial one — "the papers by this author" is an in-edge from the author.
    // The alternative measured against this was a recursive CTE over the whole
    // relation, and on 6.9M edges one hop from one seed had not returned after 45
    // seconds; it took the reader's connection with it.
    //
    // The remap and the tiling are ONE pass over each file, where they used to be
    // two loops over all of them. Two loops meant the tiling re-read from disk
    // the file the remap had just written — defensible when the alternative was a
    // second copy of the adjacency inside the engine, and pointless now that the
    // sorted relation is a value in hand. What it costs is that a failure leaves
    // some adjacencies fully rewritten and tiled and others untouched, where it
    // used to leave all of them rewritten and some tiled. Neither is a state
    // anything reads: the caller repoints the manifest only on success.
    for adjacency in adjacencies {
        let aurl = adjacency.parquet.as_str();
        let src_map = &maps[index_of(&adjacency.src_type, aurl)?];
        let dst_map = &maps[index_of(&adjacency.dst_type, aurl)?];

        // Sampled step by step, not at the phase boundary. This loop is the pass's
        // high-water mark, and a single delta across the `for` cannot say which
        // line of it is the peak — the last time that distinction was skipped the
        // wrong phase was blamed for two sessions. See `Probe::sample`.
        let step = short_name(aurl);

        // **One `Vec<u64>`, and that is the whole orientation.** This used to be
        // five allocations of it — the reader's batches, `concat_batches` into
        // one, two remapped `Vec<u32>`, `lexsort_to_indices`'s permutation, and
        // the `take` that applied it — and sampled from the inside at ten million
        // it billed +0.39 / +0.26 / +0.52 / +0.78 / +0.00 GiB. The suspect was
        // `concat_batches` and it was 13%; the SORT was 40%, because arrow's
        // `lexsort_to_indices` over more than one column routes through a
        // `RowConverter` and encodes every row into a comparable byte string
        // first — ten bytes plus an eight-byte offset for a pair that is eight
        // bytes wide.
        //
        // A pair of `u32` ordered lexicographically IS a `u64` ordered by value,
        // so there is nothing to encode: pack the key endpoint into the high half
        // and the other into the low, and `sort_unstable` is the same order for
        // no bytes at all. Ties are pairs equal in both columns, which is the
        // whole row, so an unstable sort is not an unstable result.
        let (schema, mut keys) = read_adjacency(io, aurl, src_map, dst_map, adjacency.ordered_by)?;
        probe.sample(&format!("{step}: read + remap"));

        keys.sort_unstable();
        probe.sample(&format!("{step}: sort"));

        // The remapped relation is NOT written back over its input: publishing
        // the uncut relation beside its own cut is two containers for one set of
        // rows, which is what `apps/corpus`'s `declared-tiling` fires on. The
        // input is a staging artefact and the caller deletes it, exactly as it
        // deletes the staged vertex Parquet.

        // Which endpoint addresses this file is which endpoint it is ordered by.
        // The tile space is that endpoint's type's, and the two are different
        // spaces on a cross-type edge: `by_target` of `Author authored Paper` is
        // cut on `Paper`'s ranges, not on `Author`'s.
        let endpoint_type = match adjacency.ordered_by {
            Endpoint::Src => &adjacency.src_type,
            Endpoint::Dst => &adjacency.dst_type,
        };
        let endpoint = &targets[index_of(endpoint_type, aurl)?];
        let tile_shift = shift_for(endpoint.chunk_size).ok_or_else(|| LayoutError::TileSize {
            vertex_type: endpoint.type_name.clone(),
            rows: endpoint.chunk_size,
        })?;
        let prefix = tile_prefix(aurl);
        io.ensure_prefix(&prefix)?;

        // Which tiles exist, read off the order the relation is already in rather
        // than asked for with a `DISTINCT`: it was just sorted on this very
        // column, so a tile is a run of it and every run is found in one pass.
        //
        // A vertex tile is always full — dense ids are gapless — but a tile of
        // 4,096 sources can hold no edges at all, and it contributes no rows, so
        // it gets no row group. **That is why a row-group ordinal cannot address
        // an adjacency** and no writer can fix it: the ordinals are dense and
        // the tile numbers are not. What locates a tile here is the footer's box
        // on the key column, and the runs below are written in ascending order,
        // so the boxes ascend and do not overlap — which is the property
        // `apps/corpus`'s `tile-of` asks an adjacency for.
        //
        // A run is unpacked into two `u32` arrays as it is written, so the only
        // Arrow this loop holds is one tile — tens of thousands of rows against
        // the seventy million the relation is.
        let payload = format!("{prefix}{TILES_FILE}");
        let mut writer = open_tiles(io, &payload, Arc::clone(&schema))?;
        let mut start = 0usize;
        while start < keys.len() {
            let tile = (keys[start] >> 32) >> tile_shift;
            let mut end = start + 1;
            while end < keys.len() && (keys[end] >> 32) >> tile_shift == tile {
                end += 1;
            }
            let batch = unpack(&schema, aurl, &keys[start..end], adjacency.ordered_by)?;
            writer.tile(&batch).map_err(write_err(&payload))?;
            start = end;
        }
        writer.finish().map_err(write_err(&payload))?;
        probe.sample(&format!("{step}: write tiles"));
    }
    probe.mark("remap adjacencies + write edge tiles");
    probe.finish();

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// The I/O seam: Parquet in, Parquet out, and nothing between them that an
// engine would have done.
// ──────────────────────────────────────────────────────────────────────────

/// Every `(subject, dense_id)` pair of one vertex type, or `None` when the
/// payload carries no `subject` column.
///
/// `None` is a legal corpus rather than a failure: where the identity lives when
/// the drawing tile does not carry it is an open convention, and a type without
/// one simply has no index. `apps/corpus`'s `identity-is-the-subject` reports the
/// same absence and does not fail on it either.
fn subject_pairs(
    batches: &[&RecordBatch],
    starts: &[u32],
    gather: &[u32],
    new_dense: &[u32],
    url: &str,
) -> Result<Option<Vec<(String, u32)>>, LayoutError> {
    let Some(first) = batches.first() else {
        return Ok(None);
    };
    if first.schema().index_of("subject").is_err() {
        return Ok(None);
    }

    // Downcast once per batch rather than once per row: `column()` is cheap and
    // `as_any().downcast_ref()` is not free, and this runs `n` times.
    let columns: Vec<StringArray> = batches
        .iter()
        .map(|batch| {
            let index =
                batch
                    .schema()
                    .index_of("subject")
                    .map_err(|_| LayoutError::MissingColumn {
                        target: url.to_string(),
                        column: "subject".to_string(),
                    })?;
            let column = batch.column(index);
            let column = if column.data_type() == &DataType::Utf8 {
                Arc::clone(column)
            } else {
                cast(column, &DataType::Utf8).map_err(arrow_err(url))?
            };
            Ok(column
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("a cast to Utf8 yields a StringArray")
                .clone())
        })
        .collect::<Result<_, LayoutError>>()?;

    let mut pairs: Vec<(String, u32)> = Vec::with_capacity(gather.len());
    for (position, &row) in gather.iter().enumerate() {
        let (batch, offset) = locate(starts, row);
        pairs.push((
            columns[batch].value(offset).to_string(),
            new_dense[position],
        ));
    }
    // Lexicographic, and it is the order a reader searches in. `sort_unstable_by`
    // is safe here because subjects are unique — `identity-is-the-subject` is the
    // guard that says so — and two equal keys would be a corpus defect either way.
    pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(pairs))
}

/// Write the identity index tiles for one vertex type.
///
/// Tiled with the same `chunk_size` and the same arithmetic as the payload, so a
/// reader that can seek a chunk needs nothing new to seek this. Tile `k` is the
/// `k`th slice of the SORTED order, which has nothing to do with the `dense_id`
/// range tile `k` of the payload holds — the manifest declares the two sizes
/// separately for that reason, and this emits the same number because there is
/// no reason yet for them to differ.
fn write_identity_index(
    io: &dyn LayoutIo,
    pairs: &[(String, u32)],
    target: &VertexLayoutTarget,
) -> Result<(), LayoutError> {
    let prefix = format!("{}index/", target.chunk_prefix);
    io.ensure_prefix(&prefix)?;

    let tiles = (pairs.len() as u64).div_ceil(target.chunk_size);
    let payload = format!("{prefix}{TILES_FILE}");
    let mut writer: Option<TileWriter<Sink>> = None;
    for k in 0..tiles {
        let lo = (k * target.chunk_size) as usize;
        let len = (pairs.len() - lo).min(target.chunk_size as usize);
        let slice = &pairs[lo..lo + len];
        let subjects: StringArray = slice.iter().map(|(s, _)| Some(s.as_str())).collect();
        let addresses = UInt32Array::from(slice.iter().map(|&(_, d)| d).collect::<Vec<_>>());
        let batch = RecordBatch::try_from_iter(vec![
            ("subject", Arc::new(subjects) as ArrayRef),
            ("dense_id", Arc::new(addresses) as ArrayRef),
        ])
        .map_err(arrow_err(&prefix))?;
        // Opened from the first tile's schema rather than declared up front,
        // because that schema is built here and a second statement of it is a
        // second thing to keep in step.
        let writer = match &mut writer {
            Some(open) => open,
            slot => slot.insert(open_tiles(io, &payload, batch.schema())?),
        };
        writer.tile(&batch).map_err(write_err(&payload))?;
    }
    if let Some(writer) = writer {
        writer.finish().map_err(write_err(&payload))?;
    }
    Ok(())
}

/// `parquet` errors from `url`, as a [`LayoutError::Read`].
fn read_err(url: &str) -> impl Fn(parquet::errors::ParquetError) -> LayoutError + '_ {
    move |source| LayoutError::Read {
        target: url.to_string(),
        source,
    }
}

/// Arrow-kernel errors on `url`, as a [`LayoutError::Arrow`].
fn arrow_err(url: &str) -> impl Fn(ArrowError) -> LayoutError + '_ {
    move |source| LayoutError::Arrow {
        target: url.to_string(),
        source,
    }
}

/// The `(batch, offset)` pair a file-row index names, given where each batch
/// starts.
///
/// A binary search and not a division: `read_parquet` asks for a fixed batch
/// size and the last batch is short, and nothing in the Parquet reader's
/// contract promises the others are not. Dividing would be right today and
/// silently wrong the day a reader splits on a row group instead — and wrong
/// here means a vertex tile holding the wrong rows, which is a well-formed
/// corpus that means something else.
fn locate(starts: &[u32], row: u32) -> (usize, usize) {
    let batch = match starts.binary_search(&row) {
        Ok(exact) => exact,
        // `Err(0)` cannot happen: `starts[0]` is 0 and every row index is ≥ 0.
        Err(after) => after.saturating_sub(1),
    };
    (batch, (row - starts[batch]) as usize)
}

/// Every `RecordBatch` of a Parquet, plus the schema they share.
fn read_parquet(
    io: &dyn LayoutIo,
    url: &str,
) -> Result<(SchemaRef, Vec<RecordBatch>), LayoutError> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(io.open(url)?)
        .map_err(read_err(url))?
        .with_batch_size(SCAN_BATCH_ROWS)
        .build()
        .map_err(read_err(url))?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(arrow_err(url))?;
    Ok((schema, batches))
}

/// One `u32` column of a Parquet, read on its own.
///
/// Projected, so the other columns are never decoded: the vertex file carries
/// the subject IRI and every property, and what this asks for is `dense_id`.
fn read_u32_column(io: &dyn LayoutIo, url: &str, name: &str) -> Result<Vec<u32>, LayoutError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(io.open(url)?).map_err(read_err(url))?;
    // Reserved exactly, from the footer rather than by doubling.
    let rows = builder.metadata().file_metadata().num_rows().max(0) as usize;
    if builder.schema().index_of(name).is_err() {
        return Err(LayoutError::MissingColumn {
            target: url.to_string(),
            column: name.to_string(),
        });
    }
    let mask = ProjectionMask::columns(builder.parquet_schema(), [name]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(SCAN_BATCH_ROWS)
        .build()
        .map_err(read_err(url))?;

    let mut out = Vec::with_capacity(rows);
    for batch in reader {
        let batch = batch.map_err(arrow_err(url))?;
        out.extend_from_slice(u32_column(&batch, url, name)?.values());
    }
    Ok(out)
}

/// One column of a batch as a `UInt32Array`, cast if the file declares another
/// numeric type — the `::UINTEGER` the projection used to carry.
///
/// **By name, never by position.** A projection does not reorder a file: asking
/// for `dst_dense, src_dense` yields the two columns in the order the *file*
/// declares them, so an orientation read positionally gets its endpoints
/// swapped, silently, in exactly one of the two orientations.
fn u32_column(batch: &RecordBatch, url: &str, name: &str) -> Result<UInt32Array, LayoutError> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| LayoutError::MissingColumn {
            target: url.to_string(),
            column: name.to_string(),
        })?;
    let column = batch.column(index);
    let column = if column.data_type() == &DataType::UInt32 {
        Arc::clone(column)
    } else {
        cast(column, &DataType::UInt32).map_err(arrow_err(url))?
    };
    Ok(column
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("a cast to UInt32 yields a UInt32Array")
        .clone())
}

/// Replace named columns of `batch`, keeping its schema and every other column.
///
/// The `SELECT * REPLACE (…)` of the statements this pass used to emit, and the
/// property that made that spelling the right one holds here too: a column
/// nobody named survives untouched, so a property the writer emits does not have
/// to be enumerated by the pass that renumbers the rows it sits on.
fn replace_columns(
    batch: &RecordBatch,
    url: &str,
    replacements: &[(&str, ArrayRef)],
) -> Result<RecordBatch, LayoutError> {
    let schema = batch.schema();
    let mut columns = batch.columns().to_vec();
    for (name, array) in replacements {
        let index = schema
            .index_of(name)
            .map_err(|_| LayoutError::MissingColumn {
                target: url.to_string(),
                column: (*name).to_string(),
            })?;
        columns[index] = Arc::clone(array);
    }
    RecordBatch::try_new(schema, columns).map_err(arrow_err(url))
}

/// Write one vertex type's **tile-code anchor** — the document
/// `fossil_sinks::manifest::VertexCodes` declares and `@fossil-lang/corpus`'s
/// `parseTileCodes` reads.
///
/// # What is in it
///
/// `lo[k]` is the Morton code of the first row of tile `k` and `hi[k]` the code
/// of its last. The rows within a tile are in code order and the tiles are in
/// rank order, so both arrays are non-decreasing and two binary searches over
/// them turn a code range into a run of tiles. `extent` is what the codes were
/// quantised against, so a rectangle in the corpus's own coordinates can be
/// turned into a code range in the first place.
///
/// # Why this pass writes it and nothing else can
///
/// `morton` and `order` are both in hand here — the codes are what the ranking
/// was made from — so this is a projection of two values the pass already holds
/// and not a second computation over the corpus. Anything else would have to
/// re-derive the codes from the written `x`/`y`, which means reading every tile
/// back and quantising again, and getting a *different* answer wherever the two
/// quantisations disagree by a unit.
///
/// # Hand-written JSON, deliberately
///
/// The document is a flat object with three scalars, four floats and two integer
/// arrays, and writing it by hand keeps `serde_json` out of a crate whose
/// dependency list argues for every line in it. Every number here is exact:
/// `u32` and `u64` print exactly, and Rust's `f32` `Display` emits the shortest
/// decimal that round-trips **to `f32`** — which is what a reader recovers with
/// `Math.fround(parseFloat(s))`. A non-finite coordinate cannot reach here (the
/// positions come from `cluster_layout`, which is arithmetic on integers), and
/// if one ever did it would be `null` in JSON rather than the unparseable `inf`
/// Rust prints, so it is refused instead.
fn write_tile_codes(
    io: &dyn LayoutIo,
    url: &str,
    extent: Extent,
    chunk_size: u64,
    lo: &[u32],
    hi: &[u32],
) -> Result<(), LayoutError> {
    use std::fmt::Write as _;
    use std::io::Write as _;

    let finite = [extent.xlo, extent.ylo, extent.xhi, extent.yhi]
        .iter()
        .all(|v| v.is_finite());
    if !finite {
        return Err(LayoutError::Io {
            target: url.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the layout produced a non-finite extent, which has no Morton grid",
            ),
        });
    }

    // `write!` into a `String` is infallible, so the `Result` is discarded once
    // rather than threaded through: `fmt::Error` is only reachable from a
    // `Display` impl that fails, and every value written here is a primitive.
    let mut out = String::with_capacity(24 * (lo.len() + hi.len()) + 256);
    out.push_str("{\n");
    // Declared rather than assumed: a reader that hard-codes 16 bits per axis is
    // right today and has no way to notice the day it stops being.
    let _ = writeln!(out, "  \"morton_bits\": {MORTON_BITS},");
    let _ = writeln!(out, "  \"chunk_size\": {chunk_size},");
    let _ = writeln!(out, "  \"tiles\": {},", lo.len());
    let _ = writeln!(
        out,
        "  \"extent\": {{ \"xlo\": {}, \"ylo\": {}, \"xhi\": {}, \"yhi\": {} }},",
        extent.xlo, extent.ylo, extent.xhi, extent.yhi
    );
    let array = |name: &str, values: &[u32], tail: &str, out: &mut String| {
        let _ = write!(out, "  \"{name}\": [");
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{v}");
        }
        out.push(']');
        out.push_str(tail);
    };
    array("lo", lo, ",\n", &mut out);
    array("hi", hi, "\n", &mut out);
    out.push_str("}\n");

    let mut sink = io.create(url)?;
    sink.write_all(out.as_bytes())
        .map_err(|source| LayoutError::Io {
            target: url.to_string(),
            source,
        })?;
    sink.flush().map_err(|source| LayoutError::Io {
        target: url.to_string(),
        source,
    })?;
    Ok(())
}

/// Open the row-group container one payload set goes into, at `url`.
///
/// Through [`TileWriter`] and not through a writer of its own, because one row
/// group per tile is a property of the format rather than of this pass — the
/// reader's index is the footer, and one box per tile is what makes it one.
/// Uncompressed, which is what that encoder does: the same encoder wrote the
/// file being read here.
///
/// The bytes go straight to the [`Sink`]. Encoding to a `Vec` first would put
/// the whole encoded type beside the type — 75 MB at five million, for nothing —
/// which is the same argument `try_for_each_file` makes on the sink side. On
/// [`LocalFs`] that means the peak is one row group; on
/// [`MemoryFs`](crate::io::MemoryFs) the `Vec` is unavoidable and is the sink
/// itself, because a browser has nowhere else to put it.
fn open_tiles(
    io: &dyn LayoutIo,
    url: &str,
    schema: SchemaRef,
) -> Result<TileWriter<Sink>, LayoutError> {
    TileWriter::new(io.create(url)?, schema).map_err(write_err(url))
}

/// `parquet` errors encoding into `url`, as a [`LayoutError::Write`].
fn write_err(url: &str) -> impl Fn(parquet::errors::ParquetError) -> LayoutError + '_ {
    move |source| LayoutError::Write {
        target: url.to_string(),
        source,
    }
}

/// The shift that addresses a tile of `rows` rows, or `None` if `rows` is not a
/// power of two and therefore addresses nothing.
const fn shift_for(rows: u64) -> Option<u32> {
    if rows == 0 || !rows.is_power_of_two() {
        return None;
    }
    Some(rows.trailing_zeros())
}

/// The two column indices of an adjacency Parquet, `(src_dense, dst_dense)`, or
/// [`LayoutError::AdjacencyShape`] if it is not the two `u32` columns an
/// adjacency is.
///
/// Checked once per file, from the schema, before a page is decoded. The
/// strictness is the point rather than an omission: the remap holds a row as a
/// packed `u64` and has nowhere to put a third column, so a file with one is a
/// file this pass would silently narrow. It came from somewhere other than the
/// writer, and saying so is better than writing a different relation under the
/// same name.
fn adjacency_columns(schema: &SchemaRef, url: &str) -> Result<(usize, usize), LayoutError> {
    let shaped = schema.fields().len() == 2
        && schema
            .fields()
            .iter()
            .all(|f| f.data_type() == &DataType::UInt32);
    let src = schema.index_of("src_dense").ok();
    let dst = schema.index_of("dst_dense").ok();
    match (shaped, src, dst) {
        (true, Some(src), Some(dst)) => Ok((src, dst)),
        _ => Err(LayoutError::AdjacencyShape {
            target: url.to_string(),
            columns: schema
                .fields()
                .iter()
                .map(|f| format!("{}: {}", f.name(), f.data_type()))
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

/// One orientation, read and remapped and packed, in a single streaming pass.
///
/// Returns the file's own [`SchemaRef`] — the one the tiles are written back
/// under, so the output carries the field names, nullability and schema metadata
/// the input had and the bytes do not move — and one `u64` per row: the endpoint
/// the relation is **ordered by** in the high half, the other in the low.
///
/// # Why the pair is packed rather than kept as two arrays
///
/// Because the sort is then free. A pair of `u32` compared lexicographically is
/// the packed `u64` compared by value, so ordering the relation is
/// `sort_unstable` over a slice that is already the size of the data — against
/// `lexsort_to_indices`, which for more than one column encodes every row
/// through a `RowConverter` into ten bytes plus an eight-byte offset and then
/// holds a permutation to apply. That sort was **40% of the phase that sets the
/// pass's peak**, measured from inside it.
///
/// # And the reader's batches are never collected
///
/// The `Vec` is reserved exactly, from the footer's row count, and each batch is
/// consumed into it and dropped. The phase used to hold the batches, the
/// `concat_batches` of them, and two remapped `Vec<u32>` beside each other; this
/// holds the batch it is decoding.
///
/// # Errors
///
/// [`LayoutError::AdjacencyShape`] if the file is not two `u32` columns,
/// [`LayoutError::DanglingEndpoint`] if any row names a dense id outside its
/// type's mapping — counted over the whole file and reported before anything is
/// written, so the file it would have corrupted is still the file it was.
fn read_adjacency(
    io: &dyn LayoutIo,
    url: &str,
    src_map: &[u32],
    dst_map: &[u32],
    ordered_by: Endpoint,
) -> Result<(SchemaRef, Vec<u64>), LayoutError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(io.open(url)?).map_err(read_err(url))?;
    let rows = builder.metadata().file_metadata().num_rows().max(0) as usize;
    let reader = builder
        .with_batch_size(SCAN_BATCH_ROWS)
        .build()
        .map_err(read_err(url))?;
    let schema = reader.schema();
    adjacency_columns(&schema, url)?;

    let mut keys = Vec::with_capacity(rows);
    let mut before = 0u64;
    let mut dropped = 0u64;
    for batch in reader {
        let batch = batch.map_err(arrow_err(url))?;
        before += batch.num_rows() as u64;
        let src = u32_column(&batch, url, "src_dense")?;
        let dst = u32_column(&batch, url, "dst_dense")?;
        for (&s, &d) in src.values().iter().zip(dst.values()) {
            let (Some(&s), Some(&d)) = (src_map.get(s as usize), dst_map.get(d as usize)) else {
                // Counted rather than returned on, so the message names how many
                // of how many: a single dangling endpoint and a mapping that is
                // wholesale wrong are the same error with very different numbers
                // in it, and the count is what tells them apart.
                dropped += 1;
                continue;
            };
            let (high, low) = match ordered_by {
                Endpoint::Src => (s, d),
                Endpoint::Dst => (d, s),
            };
            keys.push((u64::from(high) << 32) | u64::from(low));
        }
    }
    if dropped > 0 {
        return Err(LayoutError::DanglingEndpoint {
            target: url.to_string(),
            before,
            dropped,
        });
    }
    Ok((schema, keys))
}

/// One tile's worth of packed rows, back as the `RecordBatch` the writer takes.
///
/// The inverse of the packing in [`read_adjacency`], and the only Arrow the
/// remap holds: a run of the sorted slice, tens of thousands of rows against the
/// seventy million the relation is at ten million vertices.
///
/// The columns go back in the schema's own order, which is why the indices are
/// looked up rather than assumed — `by_target` is ordered by `dst_dense` and the
/// file still declares `src_dense` first.
///
/// # Errors
///
/// [`LayoutError::AdjacencyShape`] as [`read_adjacency`], and
/// [`LayoutError::Arrow`] if the batch does not match the schema it is built
/// against.
fn unpack(
    schema: &SchemaRef,
    url: &str,
    keys: &[u64],
    ordered_by: Endpoint,
) -> Result<RecordBatch, LayoutError> {
    let (src_index, dst_index) = adjacency_columns(schema, url)?;
    let high: Vec<u32> = keys.iter().map(|k| (k >> 32) as u32).collect();
    let low: Vec<u32> = keys.iter().map(|k| *k as u32).collect();
    let (src, dst) = match ordered_by {
        Endpoint::Src => (high, low),
        Endpoint::Dst => (low, high),
    };
    let empty: ArrayRef = Arc::new(UInt32Array::from(Vec::<u32>::new()));
    let mut columns: Vec<ArrayRef> = vec![Arc::clone(&empty), empty];
    columns[src_index] = Arc::new(UInt32Array::from(src));
    columns[dst_index] = Arc::new(UInt32Array::from(dst));
    RecordBatch::try_new(Arc::clone(schema), columns).map_err(arrow_err(url))
}

/// The last path component of an adjacency URL, without its extension —
/// `…/by_source.parquet` → `by_source`.
///
/// Only ever a label in a memory report, which is why it is total rather than
/// fallible: a URL with no separator and no dot is its own short name. Two
/// orientations of one edge type are two iterations of the same loop, and a
/// report that labelled both of them `remap` would be a report that could not be
/// read.
fn short_name(url: &str) -> &str {
    let tail = url.rsplit(['/', '\\']).next().unwrap_or(url);
    tail.rsplit_once('.').map_or(tail, |(stem, _)| stem)
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
/// Streamed batch by batch and never collected: the whole point is that the two
/// columns arrive as the `u32` slices [`CsrBuilder`] wants and no row is ever a
/// Rust tuple. The first attempt at this materialised the result set instead,
/// and the process peak went 17.0 → 24.4 GiB while the three parity tests stayed
/// green.
///
/// The projection is by NAME on the way in and read back by name on the way out
/// — see [`u32_column`]. A projection is a filter over the file's columns and
/// not a reordering of them, so `by_target`, which asks for `dst_dense` first,
/// gets `src_dense` first anyway.
fn read_orientation(
    io: &dyn LayoutIo,
    url: &str,
    key: &str,
    value: &str,
    vertex_count: u32,
    self_loops: &mut [f64],
) -> Result<Csr, LayoutError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(io.open(url)?).map_err(read_err(url))?;
    // Reserved exactly, from the Parquet footer rather than by doubling: the
    // targets array is the one large allocation left and growing into it would
    // put a copy of it beside itself. The count is a field of the footer now,
    // where it used to be a `SELECT count(*)` that read one.
    let edges = builder.metadata().file_metadata().num_rows().max(0) as usize;
    for column in [key, value] {
        if builder.schema().index_of(column).is_err() {
            return Err(LayoutError::MissingColumn {
                target: url.to_string(),
                column: column.to_string(),
            });
        }
    }
    let mask = ProjectionMask::columns(builder.parquet_schema(), [key, value]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(SCAN_BATCH_ROWS)
        .build()
        .map_err(read_err(url))?;

    let mut csr = CsrBuilder::new(vertex_count as usize, edges);
    for batch in reader {
        let batch = batch.map_err(arrow_err(url))?;
        let keys = u32_column(&batch, url, key)?;
        let values = u32_column(&batch, url, value)?;
        if !csr.push(keys.values(), values.values(), self_loops) {
            return Err(LayoutError::Disordered {
                target: url.to_string(),
                column: key.to_string(),
            });
        }
    }
    Ok(csr.finish())
}

/// Rank each vertex by its Morton code — its position in the renumbering — and
/// the ranking's inverse.
///
/// Both, because both are used and the sort produces both. `rank[old] = new` is
/// what an adjacency's endpoints are remapped through; `order[new] = old` is the
/// gather that puts the vertex rows in the new order, and it is the sorted array
/// itself. Returning only the first and recovering the second is a second pass
/// over `n` to undo what the first line did.
///
/// Ties are broken by the old `dense_id`, so the ranking is total and the same
/// input yields the same numbering on every run. Two vertices sharing a code is
/// the common case rather than an edge case: the codes quantise to 16 bits per
/// axis, and a community packs many vertices into far less than one bucket.
fn morton_ranks(morton: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut order: Vec<u32> = (0..morton.len() as u32).collect();
    order.sort_unstable_by_key(|&i| (morton[i as usize], i));
    let mut rank = vec![0u32; morton.len()];
    for (new_id, &old_id) in order.iter().enumerate() {
        rank[old_id as usize] = new_id as u32;
    }
    (rank, order)
}

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
fn place_after(positions: &mut [(f32, f32)], origin_x: f32) -> f32 {
    let mut width = 0.0f32;
    for (x, _) in positions.iter_mut() {
        width = width.max(*x);
        *x += origin_x;
    }
    origin_x + width + TYPE_GUTTER
}

/// Bits per axis on the grid positions are quantised onto, so a code is a `u32`.
///
/// Published in the code anchor rather than left implicit in [`morton2`]'s `u16`
/// argument, because a reader that hard-codes it is right today and has no way
/// to notice the day it stops being. The three implementations that agree on it
/// — this one, `apps/corpus/guards/arithmetic.mjs` and
/// `@fossil-lang/corpus`'s `MORTON_BITS` — agree against
/// `apps/corpus/guards/vectors.json` and not against each other.
pub const MORTON_BITS: u32 = 16;

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

/// One coordinate onto the `u16` grid, over `[lo, hi]`; a degenerate axis maps
/// to 0 rather than dividing by zero.
///
/// **The width is part of the contract, not an implementation detail.** The
/// subtraction, the division, the multiply by 65535 and the rounding are all
/// binary32, and a port that does the same arithmetic in binary64 disagrees:
/// `quantize(147, 0, 167)` is 57687 here and 57686 there. One unit is a
/// different Morton code, a different rank, a different `dense_id` and a
/// different tile — so the two engines that read a corpus stop agreeing about
/// which vertices are in it.
///
/// It was a closure inside [`morton_codes`] and therefore untestable, which is
/// how the writer came to be the one side of this contract nothing executed:
/// `apps/corpus/guards/vectors.json` publishes the table, `guards/arithmetic.mjs`
/// is checked against it, and until `quantize_agrees_with_the_published_table`
/// existed the Rust could have been switched to `f64` with every test in the
/// workspace still green.
fn quantize(v: f32, lo: f32, hi: f32) -> u16 {
    if hi <= lo {
        return 0;
    }
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    (t * f32::from(u16::MAX)).round() as u16
}

/// The bounding box a vertex type's positions were quantised against — **the
/// other half of the anchor**, and the half a reader cannot guess either.
///
/// A Morton code is `quantize(x, xlo, xhi)` interleaved with
/// `quantize(y, ylo, yhi)`, so a rectangle in the corpus's own coordinates is a
/// rectangle on the code grid only once this is known. It is derivable from the
/// corpus — it is the min and max of the `x` and `y` columns — but deriving it
/// means reading every tile's footer, which is the dependency the arithmetic
/// address exists to remove. Four numbers published beside the codes settle it.
///
/// **`f32`, and that is load-bearing.** The quantisation is binary32 at every
/// step (see [`quantize`]); a reader that widens these to binary64 before
/// dividing gets a different grid cell for values near a boundary, and that is a
/// different code, a different rank and a different tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    pub xlo: f32,
    pub ylo: f32,
    pub xhi: f32,
    pub yhi: f32,
}

/// The bounding box of a position list. `None` for an empty one, which has no
/// box rather than a degenerate one at the origin.
#[must_use]
fn extent_of(positions: &[(f32, f32)]) -> Option<Extent> {
    if positions.is_empty() {
        return None;
    }
    let (mut xlo, mut ylo, mut xhi, mut yhi) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in positions {
        xlo = xlo.min(x);
        ylo = ylo.min(y);
        xhi = xhi.max(x);
        yhi = yhi.max(y);
    }
    Some(Extent { xlo, ylo, xhi, yhi })
}

/// Morton codes for a position list, quantised against a **given** box.
///
/// Split from [`morton_codes`] because the box is now published rather than
/// discarded: it was computed inside the map and thrown away, and a reader that
/// wants to turn a rectangle into a code range needs the same four numbers the
/// writer used. Computing it twice from the same slice would be equal by luck
/// rather than by construction.
#[must_use]
fn morton_codes_within(positions: &[(f32, f32)], extent: Extent) -> Vec<u32> {
    positions
        .iter()
        .map(|&(x, y)| {
            morton2(
                quantize(x, extent.xlo, extent.xhi),
                quantize(y, extent.ylo, extent.yhi),
            )
        })
        .collect()
}

/// Morton codes for a position list — quantises each coordinate to `u16` over
/// the list's bounding box (a degenerate axis maps to 0). Index-aligned with
/// `positions`.
///
/// **The pass does not call this**, and that is the point of the split above it:
/// it takes the box out of [`extent_of`] and passes the *same value* to
/// [`morton_codes_within`] and to the anchor it writes, so the codes and the box
/// published beside them cannot be two computations that happen to agree. What
/// is left here is the composition the tests are written against.
#[cfg(test)]
#[must_use]
fn morton_codes(positions: &[(f32, f32)]) -> Vec<u32> {
    match extent_of(positions) {
        None => Vec::new(),
        Some(extent) => morton_codes_within(positions, extent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    /// The writer side of the quantisation contract, read off the published
    /// table instead of restated beside it.
    ///
    /// `apps/corpus/guards/vectors.json` is the deliverable — a second
    /// implementation is checked against that table and not against a sentence
    /// — and `apps/corpus/guards/arithmetic.mjs` executes it. **The engine that
    /// writes the corpus did not.** Nothing in Rust read that file; the two
    /// `morton_codes` tests below assert relative ordering and no-panic, which
    /// hold for either float width. So the reference implementation was pinned
    /// to the border cases and the producer was free to drift past them.
    ///
    /// Proved red twice, and the second one is the point: with `quantize`'s
    /// arithmetic widened to `f64` (`(f64::from(v) - f64::from(lo)) / …`, the
    /// change that leaves the whole workspace green) this fails on the
    /// 147/0/167 row with `57686, want 57687`.
    ///
    /// **What it cannot prove.** That the `DuckDB` half agrees:
    /// `apps/corpus/guards/guards.mjs` spells the width `::FLOAT` in SQL, and
    /// that `FLOAT / FLOAT` stays binary32 rather than promoting is evidenced
    /// by a passing guard, not by a width proof. And it says nothing about the
    /// bounding box the coordinates are quantised against — `morton_codes`
    /// derives that from the positions it is handed, and no vector covers it.
    #[test]
    fn quantize_agrees_with_the_published_table() {
        // Widening to binary64 is the drift this guard exists to catch, so it
        // has to be computable here — otherwise the table could quietly lose
        // the one row that separates the two widths and still pass.
        fn in_binary64(v: f64, lo: f64, hi: f64) -> u16 {
            if hi <= lo {
                return 0;
            }
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            (t * f64::from(u16::MAX)).round() as u16
        }

        let table = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("CARGO_MANIFEST_DIR has two parents")
            .join("apps/corpus/guards/vectors.json");
        let published: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&table)
                .unwrap_or_else(|e| panic!("read {}: {e}", table.display())),
        )
        .expect("vectors.json is JSON");

        let rows = published["quantize"]["vectors"]
            .as_array()
            .expect("vectors.json declares quantize.vectors");
        assert!(!rows.is_empty(), "the published table is empty");

        let f = |row: &serde_json::Value, key: &str| -> f64 {
            row[key]
                .as_f64()
                .unwrap_or_else(|| panic!("row {row} has no numeric {key}"))
        };

        let mut separates_the_widths = false;
        for row in rows {
            let (v, lo, hi) = (f(row, "v"), f(row, "lo"), f(row, "hi"));
            let want =
                u16::try_from(row["q"].as_u64().expect("q is an integer")).expect("q fits in u16");
            let got = quantize(v as f32, lo as f32, hi as f32);
            assert_eq!(
                got,
                want,
                "quantize({v}, {lo}, {hi}) = {got}, want {want} — {}",
                row["why"].as_str().unwrap_or("(no why)")
            );
            separates_the_widths |= in_binary64(v, lo, hi) != want;
        }

        assert!(
            separates_the_widths,
            "every published vector is exact in both float widths, so this \
             guard passes against a binary64 implementation and proves nothing \
             about the width the table calls part of the contract. Restore a \
             row that separates them — 147 over [0, 167] is 57687 in binary32 \
             and 57686 in binary64."
        );
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

    /// [`locate`] turns a file-row index into the `(batch, offset)` pair
    /// `interleave_record_batch` takes, and it is the one piece of the tiled
    /// gather whose failure is SILENT: an off-by-one picks a real row from a
    /// real batch, so the tile is well-formed Parquet describing the wrong
    /// vertex.
    ///
    /// The batch lengths here are deliberately UNEQUAL. The reader asks for a
    /// fixed size and the last batch is short, so a division would pass a test
    /// with even batches and be wrong on every real file — which is why this
    /// states the boundaries rather than the arithmetic.
    #[test]
    fn a_file_row_locates_in_the_batch_that_holds_it() {
        // Three batches of 4, 4 and 2 rows: starts at 0, 4, 8, ten rows total.
        let starts = [0u32, 4, 8];
        let all: Vec<(usize, usize)> = (0..10).map(|r| locate(&starts, r)).collect();
        assert_eq!(
            all,
            vec![
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3), // first batch
                (1, 0),
                (1, 1),
                (1, 2),
                (1, 3), // second
                (2, 0),
                (2, 1), // the short one
            ]
        );
    }

    /// A single batch is the degenerate case the binary search must not get
    /// wrong, and it is the shape every small fixture in this tree has — so a
    /// bug reachable only with more than one batch would pass the whole suite.
    #[test]
    fn one_batch_locates_every_row_in_itself() {
        let starts = [0u32];
        assert_eq!(locate(&starts, 0), (0, 0));
        assert_eq!(locate(&starts, 7), (0, 7));
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
        // The default tile, and the shift a reader addresses it with.
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

    /// **The packing IS the ordering**, which is the whole claim the remap rests
    /// on: sorting one `u64` per row has to be the same relation `lexsort` over
    /// two `u32` columns produced, or the corpus moves. Asserted against a sort
    /// of the pairs themselves rather than against a recorded answer.
    #[test]
    fn a_packed_pair_sorts_lexicographically() {
        let pairs: Vec<(u32, u32)> = vec![
            (1, 7),
            (0, u32::MAX),
            (u32::MAX, 0),
            (1, 0),
            (0, 0),
            (2, 3),
            (1, 7),
        ];
        let mut packed: Vec<u64> = pairs
            .iter()
            .map(|&(a, b)| (u64::from(a) << 32) | u64::from(b))
            .collect();
        packed.sort_unstable();
        let mut expected = pairs;
        expected.sort_unstable();
        let unpacked: Vec<(u32, u32)> = packed
            .iter()
            .map(|&k| ((k >> 32) as u32, k as u32))
            .collect();
        assert_eq!(unpacked, expected);
    }

    /// The shape check, and the reason it is strict: the remap holds a row as a
    /// packed `u64` and has nowhere to put a third column, so a file with one is
    /// refused rather than silently narrowed. The error names what it found,
    /// because "this is not an adjacency" is unactionable without it.
    #[test]
    fn an_adjacency_is_two_u32_columns_and_says_so_when_it_is_not() {
        let two: SchemaRef = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("src_dense", DataType::UInt32, false),
            arrow::datatypes::Field::new("dst_dense", DataType::UInt32, false),
        ]));
        assert_eq!(adjacency_columns(&two, "u").unwrap(), (0, 1));

        // Ordered by the other endpoint, and the file still declares `src_dense`
        // first — which is why the indices are looked up and not assumed.
        let swapped: SchemaRef = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("dst_dense", DataType::UInt32, false),
            arrow::datatypes::Field::new("src_dense", DataType::UInt32, false),
        ]));
        assert_eq!(adjacency_columns(&swapped, "u").unwrap(), (1, 0));

        let three: SchemaRef = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("src_dense", DataType::UInt32, false),
            arrow::datatypes::Field::new("dst_dense", DataType::UInt32, false),
            arrow::datatypes::Field::new("weight", DataType::Float32, false),
        ]));
        let message = adjacency_columns(&three, "u").unwrap_err().to_string();
        assert!(
            message.contains("weight: Float32"),
            "the error has to name what it found: {message}"
        );

        let wide: SchemaRef = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("src_dense", DataType::UInt64, false),
            arrow::datatypes::Field::new("dst_dense", DataType::UInt64, false),
        ]));
        assert!(adjacency_columns(&wide, "u").is_err());
    }

    /// A tile goes back out under the file's own schema and in its own column
    /// order, both endpoints where the file put them. `by_target` is the case
    /// that would go unnoticed: it is ordered by `dst_dense`, so the packed high
    /// half is the SECOND column of the file.
    #[test]
    fn unpacking_puts_each_endpoint_back_in_its_own_column() {
        let schema: SchemaRef = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("src_dense", DataType::UInt32, false),
            arrow::datatypes::Field::new("dst_dense", DataType::UInt32, false),
        ]));
        // key 5, other 9.
        let keys = [(5u64 << 32) | 9u64];

        let by_source = unpack(&schema, "u", &keys, Endpoint::Src).unwrap();
        assert_eq!(
            u32_column(&by_source, "u", "src_dense").unwrap().value(0),
            5
        );
        assert_eq!(
            u32_column(&by_source, "u", "dst_dense").unwrap().value(0),
            9
        );

        let by_target = unpack(&schema, "u", &keys, Endpoint::Dst).unwrap();
        assert_eq!(
            u32_column(&by_target, "u", "src_dense").unwrap().value(0),
            9
        );
        assert_eq!(
            u32_column(&by_target, "u", "dst_dense").unwrap().value(0),
            5
        );
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
/// — measured on a benchmark corpus it put 1,998 of 2,000 vertices in a single
/// cluster, and a viewport window then retained **375 of 27,244
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
/// larger than any window, so grouping by it buys a camera nothing. Too fine and
/// an aggregate read, which answers one super-node per cluster under a row cap,
/// starts dropping communities off the end of the list rather than reporting
/// that it did — see [`CLUSTER_BUDGET`].
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
/// was 1,136 MB — 16 of the 53 B/edge the core costs.
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
/// place. That is the whole of it: the degree pre-pass and the
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
    /// one, which now means the tests and
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
    ///
    /// # This is where the layout pass's memory was
    ///
    /// One row of the quotient was one `HashMap<u32, f64>`, and there is one row
    /// per community — 748,647 of them at two million vertices and **3,757,900**
    /// at ten, because the *first* contraction is the one that barely contracts:
    /// level zero stops on the 32-sweep cap without converging, and the ten
    /// thousand planted communities of the fixture are not found until level one.
    ///
    /// Sampled per step **inside** the hierarchy rather than at the phase
    /// boundary, that array of maps is **+3.41 GiB of the +3.82 GiB**
    /// `community_hierarchy` bills at ten million, and the quotient CSR built out
    /// of it is another +1.04. It holds 110,070,012 entries between its 3,757,900
    /// tables — about 33 bytes per surviving inter-community half-edge, which is
    /// a `(u32, f64)` bucket plus its control byte plus what rounding a table up
    /// to a power of two costs. Beside it, `local_moving`'s resident set is flat
    /// to three decimal places across all thirty-two sweeps.
    ///
    /// So `community_hierarchy`'s bill was never the hundreds of millions of
    /// transient maps it was read as. It was this one array of long-lived ones.
    ///
    /// (`examples/enrich_memory 10000000 14`, 2026-08-28, with a per-level probe
    /// compiled in — which costs that run about 40 s and 0.3 GiB of its own, so
    /// the +3.82 above is its `community_hierarchy` and not the 228.2 s / +3.47
    /// GiB the same build measures without it.)
    ///
    /// So the maps are gone and nothing replaces them. **Group the nodes by
    /// community first** — a counting sort, `n` `u32` and two arrays of `k` —
    /// then build one row at a time into the same sparse accumulator
    /// [`local_moving`] uses, emitting it into the CSR before the next row
    /// starts. What was `k` hash tables live at once is now one dense array of
    /// `k`, and the quotient's own `targets`/`weights` are the only thing that
    /// scales with the surviving edges.
    ///
    /// # Why the output is bit-identical
    ///
    /// The counting sort is stable by construction — nodes are counted and
    /// scattered in ascending order — so a community's members are visited in
    /// exactly the order the `0..n` loop visited them. Every `f64` in
    /// `self_loops` and in `weights` is therefore the same sequence of additions
    /// as before, and the rows come out sorted by community id, which is what
    /// the `entries.sort_unstable_by_key` it replaces was for.
    fn contract(&self, membership: &[u32], community_count: u32) -> Self {
        let k = community_count as usize;
        let n = self.node_count();

        // Members of each community, ascending, as a counting sort: `starts` is
        // the prefix sum of the community sizes and `members` the nodes laid out
        // under it.
        let mut starts = vec![0u32; k + 1];
        for &c in &membership[..n] {
            starts[c as usize + 1] += 1;
        }
        for c in 0..k {
            starts[c + 1] += starts[c];
        }
        let mut members = vec![0u32; n];
        {
            let mut cursor = starts.clone();
            for (v, &c) in membership[..n].iter().enumerate() {
                let c = c as usize;
                members[cursor[c] as usize] = v as u32;
                cursor[c] += 1;
            }
        }

        let mut self_loops = vec![0.0f64; k];
        let mut offsets = Vec::with_capacity(k + 1);
        let mut targets: Vec<u32> = Vec::new();
        let mut weights: Vec<f64> = Vec::new();
        offsets.push(0);
        let mut row = Neighbourhood::new(k);
        for c in 0..k {
            row.clear();
            for &v in &members[starts[c] as usize..starts[c + 1] as usize] {
                let v = v as usize;
                // Each node's own self-loop carries over whole.
                self_loops[c] += self.self_loops[v];
                for (u, w) in self.neighbours(v) {
                    let cu = membership[u as usize];
                    if cu as usize == c {
                        // Counted once per direction, so half lands here and
                        // half when the other endpoint is visited.
                        self_loops[c] += w / 2.0;
                    } else {
                        row.add(cu, w);
                    }
                }
            }
            // Sorted so the structure is a pure function of the input, not of
            // the order the neighbours happened to arrive in.
            row.sort();
            for &cu in row.communities() {
                targets.push(cu);
                weights.push(row.weight_of(cu));
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

/// The weight from one node into each of its neighbouring communities, as **one
/// allocation reused by every node of every sweep**.
///
/// A `HashMap` built and dropped per node per sweep is what stood here, and at
/// ten million vertices that is a few hundred million transient allocations —
/// [`local_moving`] is 32 sweeps over ten million nodes, because level zero
/// never converges and stops on the sweep cap. It cost **no resident memory at
/// all**: measured per sweep, RSS is flat to three decimal places across all
/// thirty-two. What it cost was time.
///
/// So this is a wall-clock change and it is honest about being one: three arrays
/// of `n`, 90 MB at ten million, for **Louvain 228.2 s → 132.8 s** — 1.72×,
/// measured with this change alone and nothing else in the tree, against a
/// process peak that goes the wrong way by 0.36 GiB (8.54 → 8.90).
///
/// That trade was refused once and the refusal was correct at the time: it spent
/// the one quantity `--memory-gib` bounds to buy wall clock that was not the
/// objective. What removed the objection was [`Weighted::contract`], after which
/// the peak was 5.08 GiB and 90 MB was not a trade; the adjacency remap has
/// taken it to **3.94** since, and 90 MB is less of one still.
///
/// (`examples/enrich_memory 10000000 14`, 2026-08-28, Mac16,8 — 14 cores,
/// 48 GiB, macOS 26.2 / Darwin 25.2.0.)
///
/// # Why the output is bit-identical, and not merely equal
///
/// Two orders decide the answer and both are preserved. **The accumulation
/// order** is the neighbour walk, unchanged, so every `f64` sum is the same
/// sequence of additions and therefore the same bits. **The comparison order**
/// is ascending community id — the `HashMap` version collected its entries into
/// a `Vec` and sorted it for exactly this reason — so [`Self::sort`] sorts
/// `touched` and the strictly-greater tie-break keeps the same winner.
///
/// [`Self::clear`] walks `touched` rather than the whole array, so a sweep is
/// O(E) and not O(V·k): the cost of resetting a node's accumulator is its
/// degree.
struct Neighbourhood {
    /// Accumulated weight per community id. Only the entries `touched` names are
    /// meaningful; every other entry is `0.0` and [`Self::clear`] keeps it so.
    weight: Vec<f64>,
    /// Whether a community id is already in `touched`, so a second edge into it
    /// does not list it twice.
    listed: Vec<bool>,
    /// The communities this node has an edge into. Cleared per node.
    touched: Vec<u32>,
}

impl Neighbourhood {
    /// Sized by the node count rather than by the community count, because a
    /// community id **is** a node id: [`local_moving`] starts every node in its
    /// own community and only ever moves it into one that already exists.
    fn new(n: usize) -> Self {
        Self {
            weight: vec![0.0; n],
            listed: vec![false; n],
            touched: Vec::new(),
        }
    }

    fn clear(&mut self) {
        for &c in &self.touched {
            self.weight[c as usize] = 0.0;
            self.listed[c as usize] = false;
        }
        self.touched.clear();
    }

    fn add(&mut self, community: u32, weight: f64) {
        let i = community as usize;
        if !self.listed[i] {
            self.listed[i] = true;
            self.touched.push(community);
        }
        self.weight[i] += weight;
    }

    /// `0.0` for a community with no edge into it, which is what
    /// `HashMap::get(..).unwrap_or(0.0)` said and what the gain formula wants:
    /// the node's own community is compared whether or not it is a neighbour.
    fn weight_of(&self, community: u32) -> f64 {
        self.weight[community as usize]
    }

    fn sort(&mut self) {
        self.touched.sort_unstable();
    }

    fn communities(&self) -> &[u32] {
        &self.touched
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

    // One allocation for the whole call — see [`Neighbourhood`].
    let mut into = Neighbourhood::new(n);

    let mut moved = true;
    let mut sweeps = 0;
    // Bounded because a pathological tie could otherwise oscillate. It is not a
    // safety net at level zero: on the ten-million fixture the first level runs
    // all thirty-two and is still moving nodes when it stops.
    while moved && sweeps < 32 {
        moved = false;
        sweeps += 1;
        for v in 0..n {
            let own = community[v];
            let k_v = graph.degrees[v];
            // Weight from v into each neighbouring community, into the one
            // accumulator this whole call shares.
            into.clear();
            for (u, w) in graph.neighbours(v) {
                into.add(community[u as usize], w);
            }
            // Remove v from its community before comparing, so staying put is
            // evaluated on the same footing as moving.
            totals[own as usize] -= k_v;

            let mut best = own;
            let mut best_gain = totals[own as usize].mul_add(-k_v / two_m, into.weight_of(own));
            // Ascending community id, which is what decides ties below.
            into.sort();
            for &c in into.communities() {
                if c == own {
                    continue;
                }
                let gain = totals[c as usize].mul_add(-k_v / two_m, into.weight_of(c));
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

    /// The claim the CSR read path rests on: reading the two
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
