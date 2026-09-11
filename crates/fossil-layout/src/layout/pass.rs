//! The pass itself: read the staged Parquet, apply the partition and the
//! placement, renumber into Morton order, and write the payload, the tiles and
//! the level pyramids back.
//!
//! Split out of `layout.rs` unchanged. This is the half that touches files;
//! [`super::community`], [`super::place`] and [`super::morton`] are the halves
//! that do not, which is what lets them be unit-tested without a filesystem.

use super::community::{
    Csr, CsrBuilder, Weighted, flatten_to_budget, hierarchy, order_by_hierarchy,
};
use super::morton::{morton_codes, morton_ranks};
use super::place::{CLUSTER_BUDGET, cluster_layout, place_after};
// ──────────────────────────────────────────────────────────────────────────
// W3.1b — integration: apply the pure layout to the written GraphAr vertices.
// ──────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Float32Array, RecordBatch, RecordBatchReader, StringArray, UInt32Array,
};
use arrow::compute::{cast, interleave_record_batch};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::error::ArrowError;
use fossil_df::files::TileWriter;
use fossil_mem_probe::Probe;

use crate::io::{LayoutIo, LocalFs, Sink};
use fossil_sinks::manifest::{TILES_FILE, VertexLevels};
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
/// self-edges, run [`super::community::community_hierarchy`] + [`cluster_layout`], derive the
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

    // `new_dense_id → (x, y)` per vertex type, index-aligned with `targets`.
    //
    // Held past the vertex phase for one reason: an EDGE level carries both of
    // its endpoints' coordinates, and the far end of an edge is an arbitrary
    // vertex rather than one of the level's own. Without this the adjacency
    // phase would have to read the payload back to place a line — which is the
    // read the whole pyramid exists to avoid, moved from the reader to the
    // writer. Two `f32` per vertex: 8 MB at a million, against the batches this
    // pass already holds.
    let mut placed: Vec<Vec<(f32, f32)>> = vec![Vec::new(); targets.len()];

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

        let morton = morton_codes(&positions);
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

        // The pyramid, when the type is big enough to have earned one.
        //
        // Level `k` is the rows whose NEW `dense_id` is a multiple of 4^k, and
        // it is selected by that predicate over `new_dense` rather than by
        // striding the write order — the two are the same list only where the
        // numbering is gapless, and a gap would silently make the file and the
        // predicate disagree. Which is the one property this must not break:
        // the file is an optimisation of the predicate, so a corpus without it
        // draws the identical picture and only reads more.
        //
        // `VertexLevels::planned` is the only place the levels are chosen, and
        // whoever declares them in the manifest calls the same function.
        if let Some(plan) = VertexLevels::planned(rows as u64, target.chunk_size) {
            write_levels(
                io,
                target,
                &plan,
                &batch_refs,
                &starts,
                &Enriched {
                    gather: &gather,
                    new_dense: &new_dense,
                    xs: &xs,
                    ys: &ys,
                    cluster_ids: &cluster_ids,
                },
            )?;
            probe.mark("write levels");
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

        // Where every vertex of this type ended up, keyed by the address it
        // ended up with. `xs`/`ys` are in WRITE order and `new_dense` is that
        // row's address, so this is a permutation and not a second computation.
        // The adjacency phase draws its level sets out of it — see `placed`.
        let mut by_address = vec![(0f32, 0f32); rows];
        for i in 0..rows {
            let at = new_dense[i] as usize;
            if at < by_address.len() {
                by_address[at] = (xs[i], ys[i]);
            }
        }
        placed[index] = by_address;

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

        // The pyramid of EDGES, written once per relation and from the
        // source-ordered half — the two orientations are one relation stored
        // twice, and `keys` here is already every edge of it, sorted by source.
        if adjacency.ordered_by == Endpoint::Src {
            let src = index_of(&adjacency.src_type, aurl)?;
            let dst = index_of(&adjacency.dst_type, aurl)?;
            let source_count = placed[src].len() as u64;
            // The SOURCE type's own plan and not a second one: a level of a
            // relation is *which vertices are in it*. One plan, two artefacts.
            if let Some(plan) = VertexLevels::planned(source_count, endpoint.chunk_size) {
                write_edge_levels(io, aurl, &plan, &keys, &placed[src], &placed[dst])?;
                probe.sample(&format!("{step}: write edge levels"));
            }
        }
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

/// The four columns the layout pass replaces, plus the permutation that says
/// which file row each written row came from — borrowed as one argument so that
/// [`write_levels`] takes six and not ten.
struct Enriched<'a> {
    /// File row of each written row, in write order.
    gather: &'a [u32],
    /// The new `dense_id` of each written row. **Not** its index: a `dense_id`
    /// that no row carries is skipped, so these are the values a level's
    /// predicate is evaluated against.
    new_dense: &'a [u32],
    xs: &'a [f32],
    ys: &'a [f32],
    cluster_ids: &'a [u32],
}

/// Write one vertex type's **level sets** — the decimated pyramid a zoomed-out
/// camera reads instead of striding the whole type.
///
/// # What a level is
///
/// Level `k` is the rows whose `dense_id` is a multiple of `4^k`, which over a
/// Morton-ordered `dense_id` is one vertex per quadtree cell of depth `k`. Every
/// row is a real vertex at its real position — there is no synthetic centroid
/// here, because a centroid cannot nest: replace it with its children and every
/// point on screen moves. A decimation nests by construction, so zooming in only
/// ever ADDS.
///
/// # The property this must not break
///
/// **The file and the predicate select the same rows.** A level set is an
/// optimisation of `dense_id % 4^k == 0` over the payload and nothing else, so a
/// corpus without one draws the same picture and only reads more. That is what
/// keeps the pyramid from becoming a second contract, and it is why the
/// selection here is a predicate over [`Enriched::new_dense`] rather than a
/// stride over the write order: the two coincide only while the numbering is
/// gapless, and a hole would make the file quietly disagree with the predicate
/// a reader without one evaluates.
/// `crates/fossil-layout/tests/levels.rs` is that test.
///
/// # Each level is a payload set, addressed by the same rule
///
/// Level `k` goes under `<chunk_prefix><stem>{k}/`, tiled at the plan's
/// `chunk_size` into one Parquet whose row groups are its tiles — the same
/// shape, the same filename and the same container as the payload beside it. So
/// a reader that can address a type can address a level of it with no new
/// arithmetic, and tile `j` of level `k` is the `dense_id` range
/// `[j·chunk·4^k, (j+1)·chunk·4^k)`.
fn write_levels(
    io: &dyn LayoutIo,
    target: &VertexLayoutTarget,
    plan: &VertexLevels,
    batch_refs: &[&RecordBatch],
    starts: &[u32],
    rows: &Enriched<'_>,
) -> Result<(), LayoutError> {
    let Some(first) = batch_refs.first() else {
        return Ok(());
    };
    for &level in &plan.levels {
        // `VertexLevels::stride` is the only place the pyramid's base is
        // written down, and it saturates rather than panicking past `u64`.
        let stride = VertexLevels::stride(level);
        let picks: Vec<usize> = rows
            .new_dense
            .iter()
            .enumerate()
            .filter(|&(_, &dense)| u64::from(dense) % stride == 0)
            .map(|(i, _)| i)
            .collect();
        let prefix = format!("{}{}", target.chunk_prefix, plan.level_prefix(level));
        io.ensure_prefix(&prefix)?;
        let url = format!("{prefix}{TILES_FILE}");
        let mut writer = open_tiles(io, &url, first.schema())?;
        for tile in picks.chunks(plan.chunk_size as usize) {
            let gathered: Vec<(usize, usize)> = tile
                .iter()
                .map(|&i| locate(starts, rows.gather[i]))
                .collect();
            let batch = interleave_record_batch(batch_refs, &gathered).map_err(arrow_err(&url))?;
            let take = |pick: &dyn Fn(usize) -> u32| -> Vec<u32> {
                tile.iter().map(|&i| pick(i)).collect()
            };
            let enriched = replace_columns(
                &batch,
                &url,
                &[
                    (
                        "dense_id",
                        Arc::new(UInt32Array::from(take(&|i| rows.new_dense[i]))) as ArrayRef,
                    ),
                    (
                        "x",
                        Arc::new(Float32Array::from(
                            tile.iter().map(|&i| rows.xs[i]).collect::<Vec<f32>>(),
                        )),
                    ),
                    (
                        "y",
                        Arc::new(Float32Array::from(
                            tile.iter().map(|&i| rows.ys[i]).collect::<Vec<f32>>(),
                        )),
                    ),
                    (
                        "cluster_id",
                        Arc::new(UInt32Array::from(take(&|i| rows.cluster_ids[i]))),
                    ),
                ],
            )?;
            writer.tile(&enriched).map_err(write_err(&url))?;
        }
        writer.finish().map_err(write_err(&url))?;
    }
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

/// Write one relation's **level sets** — the edges a zoomed-out camera draws
/// without opening the vertex payload.
///
/// # What a level of a relation is
///
/// Level `k` is the edges **incident to a level-`k` vertex** in either
/// orientation — `src % 4^k == 0 || dst % 4^k == 0` — and every row carries
/// BOTH endpoints' coordinates.
///
/// The coordinates are the reason this file exists. A camera keeps an edge with
/// ONE end drawn, so the far end has to be positioned to draw the line, and a
/// vertex level holds one row in `4^k`: on the bench corpus at the app's own
/// three-pixel floor, levels 1 to 4 position 14.3%, 3.2%, 0.79% and 0.12% of the
/// edges the same view draws. Carrying `src_x`/`src_y`/`dst_x`/`dst_y` makes the set
/// **self-drawing** — the lines and their ends come out of this file and no
/// vertex tile is opened for them.
///
/// # It decimates and does not aggregate
///
/// Every row is a real edge between two real vertices at their real positions.
/// Nothing is contracted, which is what keeps the standing refusal intact: an
/// edge between two survivors standing in for a path through vertices that are
/// not drawn is synthetic, and replacing it with the path when the camera zooms
/// moves every line on screen. And it nests, because level `k+1`'s vertices are
/// a subset of level `k`'s.
///
/// # Tiled by the rule the vertex levels already have
///
/// Tile `j` holds the rows whose `src_dense` is in
/// `[j · chunk_size · 4^k, (j+1) · chunk_size · 4^k)` — the source level's own
/// tile range, so a reader addresses these with the shift it already has.
/// `keys` arrives sorted by source, so each tile is a run and one pass finds
/// every one of them.
fn write_edge_levels(
    io: &dyn LayoutIo,
    adjacency: &str,
    plan: &VertexLevels,
    keys: &[u64],
    src_placed: &[(f32, f32)],
    dst_placed: &[(f32, f32)],
) -> Result<(), LayoutError> {
    // The relation's own directory, which is the parent of the orientation's:
    // `…/edge/A_b_A/by_source.parquet` → `…/edge/A_b_A/`. The level sets belong
    // to the relation and not to one half of it.
    let orientation = tile_prefix(adjacency);
    let relation = orientation
        .trim_end_matches('/')
        .rsplit_once('/')
        .map_or_else(|| orientation.clone(), |(head, _)| format!("{head}/"));

    let schema = Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
        Field::new("src_x", DataType::Float32, false),
        Field::new("src_y", DataType::Float32, false),
        Field::new("dst_x", DataType::Float32, false),
        Field::new("dst_y", DataType::Float32, false),
    ]));
    let shift = shift_for(plan.chunk_size).ok_or_else(|| LayoutError::TileSize {
        vertex_type: String::new(),
        rows: plan.chunk_size,
    })?;

    for &level in &plan.levels {
        // A level tile's address is the payload's own shift plus the bits the
        // level drops, which is `VertexLevels::stride_bits` and nowhere else.
        // `checked_shr` rather than a mask: past the width of a `dense_id` the
        // whole level is one tile, which is the answer and not an overflow.
        let stride = VertexLevels::stride(level);
        let span = shift + VertexLevels::stride_bits(level);
        let tile_of = |key: u64| (key >> 32).checked_shr(span).unwrap_or(0);
        let prefix = format!("{relation}{}", plan.level_prefix(level));
        io.ensure_prefix(&prefix)?;
        let url = format!("{prefix}{TILES_FILE}");
        let mut writer = open_tiles(io, &url, Arc::clone(&schema))?;

        let mut start = 0usize;
        while start < keys.len() {
            let tile = tile_of(keys[start]);
            let mut end = start + 1;
            while end < keys.len() && tile_of(keys[end]) == tile {
                end += 1;
            }
            let (mut src, mut dst) = (Vec::new(), Vec::new());
            let (mut sx, mut sy, mut dx, mut dy) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for &key in &keys[start..end] {
                let s = (key >> 32) as u32;
                let d = key as u32;
                if u64::from(s) % stride != 0 && u64::from(d) % stride != 0 {
                    continue;
                }
                // An endpoint outside its type's table is a dangling one, which
                // this pass reports elsewhere and does not draw: skipped rather
                // than placed at the origin, because a line to (0, 0) is a lie
                // about where a vertex is.
                let (Some(&(x0, y0)), Some(&(x1, y1))) =
                    (src_placed.get(s as usize), dst_placed.get(d as usize))
                else {
                    continue;
                };
                src.push(s);
                dst.push(d);
                sx.push(x0);
                sy.push(y0);
                dx.push(x1);
                dy.push(y1);
            }
            if !src.is_empty() {
                let batch = RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![
                        Arc::new(UInt32Array::from(src)) as ArrayRef,
                        Arc::new(UInt32Array::from(dst)),
                        Arc::new(Float32Array::from(sx)),
                        Arc::new(Float32Array::from(sy)),
                        Arc::new(Float32Array::from(dx)),
                        Arc::new(Float32Array::from(dy)),
                    ],
                )
                .map_err(arrow_err(&url))?;
                writer.tile(&batch).map_err(write_err(&url))?;
            }
            start = end;
        }
        writer.finish().map_err(write_err(&url))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
