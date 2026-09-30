//! The pass itself: one community partition and one placement over the whole
//! graph, and the global `dense_id` that falls out of it.
//!
//! **It computes and writes nothing.** It takes the rows the executor
//! materialised — one batch set per vertex type, one per relation — and answers
//! a [`Layout`]: for every vertex of every type its global `dense_id`, its
//! position and its community. `fossil-df` writes the corpus from that, which
//! is why this crate links no Parquet writer and no filesystem.
//!
//! # One graph
//!
//! Every vertex type is laid out together: the union of their vertices is one
//! graph, every relation is an edge of it — the ones between types included —
//! and Louvain partitions that graph once. `dense_id` is the rank of each
//! vertex's position on the Hilbert curve over the union's bounding box, so it
//! is global and gapless: `0..V` across all the vertex tables. It used to be
//! one plane per type, each offset from the last, with ids per type; a
//! relation between two types then pulled on nothing, and a reader had to know
//! a vertex's type to resolve an endpoint.

use super::community::{Weighted, flatten_to_budget, hierarchy, order_by_hierarchy};
use super::hilbert::{hilbert_codes, hilbert_ranks};
use super::place::{CLUSTER_BUDGET, cluster_layout};

use std::sync::Arc;

use arrow::array::{Array, RecordBatch, UInt32Array};
use arrow::compute::cast;
use arrow::datatypes::DataType;
use arrow::error::ArrowError;
use fossil_mem_probe::Probe;

/// One vertex type's rows, as the executor materialised them.
///
/// Row `i` is the vertex whose type-local id is `i`: the executor numbers a
/// type's rows `0..n` in the order it produced them and writes that number as
/// `dense_id`, which is what a [`Relation`]'s endpoints hold.
/// [`LayoutError::NotDense`] refuses rows that do not.
#[derive(Debug, Clone, Copy)]
pub struct VertexType<'a> {
    /// The type's label, for errors and reports.
    pub name: &'a str,
    /// The rows. Borrowed: they are the batches the caller is holding.
    pub batches: &'a [RecordBatch],
}

/// One relation's rows, as the executor materialised them: `src_dense` and
/// `dst_dense`, each a type-local id of its endpoint's type.
#[derive(Debug, Clone, Copy)]
pub struct Relation<'a> {
    /// The relation's table name, for errors.
    pub name: &'a str,
    /// Index of the source type in the `vertices` handed to [`layout`].
    pub source: usize,
    /// Index of the destination type in the `vertices` handed to [`layout`].
    pub destination: usize,
    /// The rows, in any order.
    pub batches: &'a [RecordBatch],
}

/// What the pass decided, per vertex type and per type-local id.
#[derive(Debug, Default)]
pub struct Layout {
    /// `ids[t][i]`: the global `dense_id` of vertex `i` of type `t`.
    pub ids: Vec<Vec<u32>>,
    /// `order[t]`: type `t`'s local ids in ascending global `dense_id` — the
    /// row order its table is written in.
    pub order: Vec<Vec<u32>>,
    /// `x[t][i]`, the horizontal coordinate of vertex `i` of type `t`.
    pub x: Vec<Vec<f32>>,
    /// `y[t][i]`, the vertical coordinate.
    pub y: Vec<Vec<f32>>,
    /// `cluster[t][i]`, the community the pass cut the hierarchy at.
    pub cluster: Vec<Vec<u32>>,
}

/// Failure modes of [`layout`] and [`Layout::edges`].
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// An Arrow kernel — the cast to `u32` — refused what it was handed.
    /// Reachable only through a column whose type is not what the writer
    /// declares.
    #[error("layout arrow op on `{target}` failed: {source}")]
    Arrow {
        target: String,
        #[source]
        source: ArrowError,
    },
    /// A batch set missing a column the pass reads (`dense_id` on a vertex,
    /// `src_dense` / `dst_dense` on a relation).
    #[error("`{target}` has no `{column}` column")]
    MissingColumn { target: String, column: String },
    /// A vertex type whose `dense_id` is not its row number.
    #[error("vertex type `{target}` numbers row {row} as {found}; a type's rows are 0..n in order")]
    NotDense {
        target: String,
        row: u64,
        found: u32,
    },
    /// A relation naming a type index no vertex type has.
    #[error("relation `{target}` names vertex type #{index}, and there are {types}")]
    UnknownVertexType {
        target: String,
        index: usize,
        types: usize,
    },
    /// An endpoint naming a type-local id its type does not have.
    ///
    /// An error rather than a skipped row: a relation quietly missing edges
    /// reads downstream as a sparser graph, not as a bug.
    #[error(
        "relation `{target}` has {dropped} of {before} rows naming a vertex that does not exist"
    )]
    DanglingEndpoint {
        target: String,
        before: u64,
        dropped: u64,
    },
    /// More vertices than a `u32` `dense_id` can number.
    #[error("the graph has {vertices} vertices; a `dense_id` is a u32")]
    TooLarge { vertices: u64 },
    /// The caller declared a memory budget smaller than what the pass will hold
    /// for a graph of this shape — see [`estimated_peak_bytes`].
    ///
    /// **A refusal and not a degradation.** The pass has no spill path: it
    /// holds Rust `Vec`s and has nowhere to put them, so the two honest answers
    /// to "it does not fit" are *refuse* and *produce a different corpus*, and a
    /// budget that changed the output would make two runs of one program
    /// disagree exactly when one of them declared a budget. So the budget
    /// decides **whether the pass runs**, never what it computes.
    #[error(
        "the layout pass needs about {} GiB for {vertex_count} vertices and {adjacency_rows} \
         adjacency rows, and the budget is {} GiB",
        .needed_bytes / (1 << 30),
        .declared_bytes / (1 << 30)
    )]
    OverBudget {
        /// Rows across every vertex batch set the pass was handed.
        vertex_count: u64,
        /// Two per edge row: the pass holds both directions.
        adjacency_rows: u64,
        /// What [`estimated_peak_bytes`] says this graph costs.
        needed_bytes: u64,
        /// The declared budget, in bytes.
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
/// | the dense-id check | 4 |
/// | two CSR `offsets` (`usize`) | 16 |
/// | `self_loops`, `degrees` (`f64`) | 16 |
/// | Louvain `community`, `totals`, `levels[0]` | 16 |
/// | [`Neighbourhood`], and [`Weighted::contract`]'s counting sort under it | 21 |
/// | `clusters`, `placement`, `positions`, `codes`, `new_ids`, `order` | 28 |
/// | the per-type split: `ids`, `order`, `x`, `y`, `cluster` | 20 |
///
/// That is 121, and this is 132 — the remainder is the allocator's, and it is a
/// term rather than a rounding; it was fitted when the pass also held a cell
/// pyramid (3 bytes) and a gather through the rows (4 more), and it was not
/// lowered when those left, because it is a bound and a bound may be loose. `vec![0u32; n]` asks for `4n` and the OS hands
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
const VERTEX_ARRAY_BYTES: u64 = 132;

/// Bytes per adjacency row, counting each direction of an edge as a row — the
/// symmetric CSR the pass builds holds both.
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
/// It was **`read CSR + CSC`** when this was fitted, and that step is arithmetic
/// rather than a guess: one `u32` of `targets` per row plus one offset per vertex, and Louvain
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
/// Thirty-two mebibytes rather than five, and the reason is where the number is
/// used rather than where it was measured. At the small end this term **is** the
/// answer, and the run-to-run spread is a larger fraction of it than of anything
/// else here: the same sixty-thousand-vertex corpus holds 25.89 MB in a release
/// build and 30.10 in the debug build `cargo test` produces. A floor fitted to
/// the optimised build is a floor that fails on the guard.
///
/// # It was 16 MiB, and the fixture had been subsidising the measurement
///
/// `crates/fossil-layout/tests/budget_bound.rs` samples the resident set across
/// the pass and asserts `held <= declared`. It went red at sixty thousand, and
/// the cause is the input becoming Arrow rather than anything in the pass: the
/// fixture used to write Parquet and **drop** its arrays, so the baseline was
/// taken with those pages already freed and the pass reused them. The batches
/// are live now — which is what a real run looks like, where `execute_graph`
/// still holds them — so the pass has to ask for pages of its own, and
/// `peak − baseline` stopped being biased low.
///
/// Measured after the change, debug build, seven runs at sixty thousand
/// vertices and mean degree fourteen — `held` minus the three linear terms:
///
/// | | MB |
/// | --- | --- |
/// | six runs alone | 12.48 · 14.53 · 14.69 · 15.28 · 15.81 · 16.46 |
/// | one run **in a loaded batch**, which is how CI runs it | **19.9** |
///
/// So the old floor sat *below* the worst observation, which is why it flaked
/// rather than failed. This clears 19.9 MB by 68%, and the loaded run is the one
/// it is fitted to: `cargo test` runs test binaries in parallel and a bound that
/// only holds on an idle machine is a bound that goes red on somebody else's.
///
/// Nothing at the sizes the rest of this file is calibrated on notices: it is
/// 0.9% of the ten-million estimate, where 16 MiB was 0.4%.
const LAYOUT_BASE_BYTES: u64 = 32 * 1024 * 1024;

/// What one byte of the vertex payload costs while the pass runs, in
/// thousandths.
///
/// The payload is the one input whose cost is **not** a function of the row
/// count: a row is a subject IRI and every property the mapping emitted, and a
/// corpus of four integer columns and a corpus of a dozen strings have the same
/// V. So this term measures the payload rather than pretending a row has a
/// width.
///
/// # It is 1.00 now, and that is the same measurement
///
/// It was 1.30 per **uncompressed Parquet byte**, the footer's
/// `total_byte_size`, and the 1.30 was the decode: at ten million the fixture's
/// vertex Parquet declared 1,006 MiB and `read vertices` billed +1.28 GiB
/// decoded. There is no footer to read any more and no decode to pay — the
/// batches arrive as Arrow — so the input to this term is
/// `get_array_memory_size`, which is the 1.28 GiB directly. The conversion
/// factor and the thing it converted between both went away in one step; the
/// number of bytes being bounded did not change.
///
/// # And it is an over-count on purpose
///
/// These batches are the **caller's**, alive whether the pass runs or not, and
/// the pass borrows rather than copying them. Charging them to the pass is
/// therefore deliberately pessimistic, which is the direction
/// [`estimated_peak_bytes`] is calibrated in: the budget bounds each stage of
/// the write path rather than their sum, and on the browser path the tab has no
/// second stage to charge them to.
const VERTEX_PAYLOAD_PERMILLE: u64 = 1_000;

/// What [`layout`] will hold, in bytes, for a corpus of this
/// shape — the terms above, summed: a floor the corpus does not change, one per
/// vertex, one per adjacency row, and one per uncompressed byte of the vertex
/// file.
///
/// **Every input is already in hand.** `vertex_count` and `adjacency_rows` are
/// sums of `RecordBatch::num_rows`, `vertex_payload_bytes` a sum of
/// `get_array_memory_size`, and all three are properties of the batches the
/// caller passed in — no file is opened to learn any of them. That is what makes
/// a budget check something the pass can afford to do *first*, at second zero,
/// rather than discovering at second two hundred that the machine cannot finish.
/// It used to be three Parquet footer reads, which was already cheap; it is now
/// arithmetic over metadata that is in memory.
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

/// Row count and resident byte size of a batch set — the budget's inputs,
/// both already in memory, so the check costs no I/O.
fn footprint(batches: &[RecordBatch]) -> (u64, u64) {
    let rows = batches.iter().map(|b| b.num_rows() as u64).sum();
    let bytes = batches
        .iter()
        .map(|b| b.get_array_memory_size() as u64)
        .sum();
    (rows, bytes)
}

/// Lay the whole graph out: one Louvain partition over the union of every
/// vertex type with every relation as an edge, one placement, and the global
/// `dense_id` as the Hilbert rank of each position.
///
/// `memory_bytes` is a declared budget in bytes, or `None` for none. It is
/// checked once, first, against [`estimated_peak_bytes`]; over it is
/// [`LayoutError::OverBudget`] and nothing is computed. Under it, the answer is
/// the one an unbounded run gives.
///
/// Deterministic: the same rows give the same layout on every run, ties in the
/// Hilbert order broken by type and then by type-local id.
///
/// # Errors
///
/// [`LayoutError`] on a vertex type whose rows are not numbered `0..n`, a
/// relation naming a type or a vertex that does not exist, a graph larger than
/// a `u32` numbers, or a declared budget the graph does not fit in.
pub fn layout(
    vertices: &[VertexType<'_>],
    relations: &[Relation<'_>],
    memory_bytes: Option<u64>,
) -> Result<Layout, LayoutError> {
    if let Some(declared) = memory_bytes {
        let (mut vertex_count, mut payload) = (0u64, 0u64);
        for v in vertices {
            let (rows, bytes) = footprint(v.batches);
            vertex_count = vertex_count.saturating_add(rows);
            payload = payload.saturating_add(bytes);
        }
        let adjacency_rows = relations
            .iter()
            .map(|r| footprint(r.batches).0)
            .sum::<u64>()
            .saturating_mul(2);
        let needed_bytes = estimated_peak_bytes(vertex_count, adjacency_rows, payload);
        if needed_bytes > declared {
            return Err(LayoutError::OverBudget {
                vertex_count,
                adjacency_rows,
                needed_bytes,
                declared_bytes: declared,
            });
        }
    }

    let mut probe = Probe::new(&format!(
        "layout — {} vertex type(s), {} relation(s)",
        vertices.len(),
        relations.len()
    ));

    let counts: Vec<u32> = vertices.iter().map(dense_count).collect::<Result<_, _>>()?;
    let mut offsets = Vec::with_capacity(counts.len() + 1);
    let mut total = 0u64;
    for &c in &counts {
        offsets.push(u32::try_from(total).map_err(|_| LayoutError::TooLarge { vertices: total })?);
        total += u64::from(c);
    }
    let n = u32::try_from(total).map_err(|_| LayoutError::TooLarge { vertices: total })?;

    let mut columns: Vec<Vec<(UInt32Array, UInt32Array)>> = Vec::with_capacity(relations.len());
    for r in relations {
        for index in [r.source, r.destination] {
            if index >= counts.len() {
                return Err(LayoutError::UnknownVertexType {
                    target: r.name.to_string(),
                    index,
                    types: counts.len(),
                });
            }
        }
        let pairs = endpoint_columns(r)?;
        let (src_n, dst_n) = (counts[r.source], counts[r.destination]);
        let (mut before, mut dropped) = (0u64, 0u64);
        for (s, d) in &pairs {
            before += s.len() as u64;
            dropped += s
                .values()
                .iter()
                .zip(d.values())
                .filter(|&(&s, &d)| s >= src_n || d >= dst_n)
                .count() as u64;
        }
        if dropped > 0 {
            return Err(LayoutError::DanglingEndpoint {
                target: r.name.to_string(),
                before,
                dropped,
            });
        }
        columns.push(pairs);
    }
    probe.mark("read relations");

    let graph = Weighted::from_pairs(n as usize, || {
        relations.iter().zip(&columns).flat_map(|(r, pairs)| {
            let (os, od) = (offsets[r.source], offsets[r.destination]);
            pairs.iter().flat_map(move |(s, d)| {
                s.values()
                    .iter()
                    .zip(d.values())
                    .map(move |(&s, &d)| (s + os, d + od))
            })
        })
    });
    drop(columns);
    let levels = if n == 0 { Vec::new() } else { hierarchy(graph) };
    probe.mark("community_hierarchy");

    let (clusters, _) = flatten_to_budget(&levels, n, CLUSTER_BUDGET);
    let mut placement = levels.first().cloned().unwrap_or_else(|| (0..n).collect());
    if !levels.is_empty() {
        order_by_hierarchy(&levels, 0, &mut placement);
    }
    drop(levels);
    let positions = cluster_layout(&placement);
    drop(placement);
    probe.mark("flatten + order + place");

    let codes = hilbert_codes(&positions);
    let (rank, order) = hilbert_ranks(&codes);
    drop(codes);
    probe.mark("hilbert codes + ranks");

    let mut out = Layout::default();
    for (t, &count) in counts.iter().enumerate() {
        let range = offsets[t] as usize..(offsets[t] + count) as usize;
        out.ids.push(rank[range.clone()].to_vec());
        out.x
            .push(positions[range.clone()].iter().map(|p| p.0).collect());
        out.y
            .push(positions[range.clone()].iter().map(|p| p.1).collect());
        out.cluster.push(clusters[range].to_vec());
        out.order.push(Vec::with_capacity(count as usize));
    }
    for &union in &order {
        let t = offsets.partition_point(|&o| o <= union) - 1;
        // A type with no vertices shares its offset with the next; the last one
        // at or below `union` is the type that holds it.
        out.order[t].push(union - offsets[t]);
    }
    probe.mark("split by type");
    probe.finish();
    Ok(out)
}

impl Layout {
    /// A relation's rows in global ids, sorted by `(src, dst)`: the two columns
    /// of its edge table, `src` first.
    ///
    /// # Errors
    ///
    /// [`LayoutError`] on a relation naming a type or a vertex this layout does
    /// not hold, or a batch without the two endpoint columns.
    pub fn edges(&self, relation: &Relation<'_>) -> Result<(Vec<u32>, Vec<u32>), LayoutError> {
        let types = self.ids.len();
        let (Some(src_ids), Some(dst_ids)) = (
            self.ids.get(relation.source),
            self.ids.get(relation.destination),
        ) else {
            return Err(LayoutError::UnknownVertexType {
                target: relation.name.to_string(),
                index: relation.source.max(relation.destination),
                types,
            });
        };
        let rows = relation.batches.iter().map(RecordBatch::num_rows).sum();
        let mut keys: Vec<u64> = Vec::with_capacity(rows);
        let mut dropped = 0u64;
        for (s, d) in endpoint_columns(relation)? {
            for (&s, &d) in s.values().iter().zip(d.values()) {
                match (src_ids.get(s as usize), dst_ids.get(d as usize)) {
                    (Some(&s), Some(&d)) => keys.push((u64::from(s) << 32) | u64::from(d)),
                    _ => dropped += 1,
                }
            }
        }
        if dropped > 0 {
            return Err(LayoutError::DanglingEndpoint {
                target: relation.name.to_string(),
                before: rows as u64,
                dropped,
            });
        }
        keys.sort_unstable();
        Ok(keys
            .into_iter()
            .map(|k| ((k >> 32) as u32, k as u32))
            .unzip())
    }
}

/// A vertex type's row count, having checked its `dense_id` column is its row
/// number. A type with no rows has no batches to check.
fn dense_count(v: &VertexType<'_>) -> Result<u32, LayoutError> {
    let mut row = 0u32;
    for batch in v.batches {
        let ids = u32_column(batch, v.name, "dense_id")?;
        for &found in ids.values() {
            if found != row {
                return Err(LayoutError::NotDense {
                    target: v.name.to_string(),
                    row: u64::from(row),
                    found,
                });
            }
            row = row.checked_add(1).ok_or(LayoutError::TooLarge {
                vertices: u64::from(u32::MAX) + 1,
            })?;
        }
    }
    Ok(row)
}

/// A relation's two endpoint columns, batch by batch.
fn endpoint_columns(r: &Relation<'_>) -> Result<Vec<(UInt32Array, UInt32Array)>, LayoutError> {
    r.batches
        .iter()
        .map(|b| {
            Ok((
                u32_column(b, r.name, "src_dense")?,
                u32_column(b, r.name, "dst_dense")?,
            ))
        })
        .collect()
}

/// A named column as `u32`, cast if the writer handed a wider integer.
fn u32_column(batch: &RecordBatch, target: &str, name: &str) -> Result<UInt32Array, LayoutError> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|_| LayoutError::MissingColumn {
            target: target.to_string(),
            column: name.to_string(),
        })?;
    let column = batch.column(index);
    let column = if column.data_type() == &DataType::UInt32 {
        Arc::clone(column)
    } else {
        cast(column, &DataType::UInt32).map_err(|source| LayoutError::Arrow {
            target: target.to_string(),
            source,
        })?
    };
    Ok(column
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("a cast to UInt32 yields a UInt32Array")
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::ArrayRef;

    fn vertices(n: u32) -> Vec<RecordBatch> {
        vec![
            RecordBatch::try_from_iter(vec![(
                "dense_id",
                Arc::new(UInt32Array::from_iter_values(0..n)) as ArrayRef,
            )])
            .unwrap(),
        ]
    }

    fn edges(pairs: &[(u32, u32)]) -> Vec<RecordBatch> {
        let (s, d): (Vec<u32>, Vec<u32>) = pairs.iter().copied().unzip();
        vec![
            RecordBatch::try_from_iter(vec![
                ("src_dense", Arc::new(UInt32Array::from(s)) as ArrayRef),
                ("dst_dense", Arc::new(UInt32Array::from(d)) as ArrayRef),
            ])
            .unwrap(),
        ]
    }

    #[test]
    fn ids_are_global_gapless_and_a_permutation() {
        let (a, b) = (vertices(5), vertices(3));
        let ab = edges(&[(0, 0), (1, 2), (4, 1)]);
        let aa = edges(&[(0, 1), (1, 2), (2, 3), (3, 4)]);
        let types = [
            VertexType {
                name: "A",
                batches: &a,
            },
            VertexType {
                name: "B",
                batches: &b,
            },
        ];
        let rels = [
            Relation {
                name: "A_r_B",
                source: 0,
                destination: 1,
                batches: &ab,
            },
            Relation {
                name: "A_s_A",
                source: 0,
                destination: 0,
                batches: &aa,
            },
        ];
        let l = layout(&types, &rels, None).unwrap();
        let mut all: Vec<u32> = l.ids.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..8).collect::<Vec<_>>());
        for t in 0..2 {
            let ordered: Vec<u32> = l.order[t].iter().map(|&i| l.ids[t][i as usize]).collect();
            assert!(
                ordered.windows(2).all(|w| w[0] < w[1]),
                "type {t} rows ascend"
            );
            assert_eq!(l.order[t].len(), l.ids[t].len());
        }
        let (src, dst) = l.edges(&rels[0]).unwrap();
        assert_eq!(src.len(), 3);
        let mut pairs: Vec<(u32, u32)> = src.iter().copied().zip(dst.iter().copied()).collect();
        let sorted = {
            let mut p = pairs.clone();
            p.sort_unstable();
            p
        };
        assert_eq!(pairs, sorted, "edges come sorted by (src, dst)");
        pairs.sort_unstable();
        let mut want: Vec<(u32, u32)> = [(0u32, 0u32), (1, 2), (4, 1)]
            .iter()
            .map(|&(s, d)| (l.ids[0][s as usize], l.ids[1][d as usize]))
            .collect();
        want.sort_unstable();
        assert_eq!(pairs, want);
    }

    #[test]
    fn the_same_rows_lay_out_the_same_way_twice() {
        let a = vertices(40);
        let pairs: Vec<(u32, u32)> = (0..39).map(|i| (i, i + 1)).collect();
        let aa = edges(&pairs);
        let types = [VertexType {
            name: "A",
            batches: &a,
        }];
        let rels = [Relation {
            name: "A_n_A",
            source: 0,
            destination: 0,
            batches: &aa,
        }];
        let one = layout(&types, &rels, None).unwrap();
        let two = layout(&types, &rels, None).unwrap();
        assert_eq!(one.ids, two.ids);
        assert_eq!(one.x, two.x);
    }

    #[test]
    fn an_empty_type_and_an_empty_graph_are_not_errors() {
        let none: Vec<RecordBatch> = Vec::new();
        let a = vertices(2);
        let types = [
            VertexType {
                name: "Nobody",
                batches: &none,
            },
            VertexType {
                name: "A",
                batches: &a,
            },
        ];
        let l = layout(&types, &[], None).unwrap();
        assert!(l.ids[0].is_empty());
        assert_eq!(l.order[1].len(), 2);
        assert!(layout(&[], &[], None).unwrap().ids.is_empty());
    }

    #[test]
    fn a_dangling_endpoint_is_an_error_and_not_a_missing_row() {
        let a = vertices(2);
        let aa = edges(&[(0, 1), (1, 7)]);
        let types = [VertexType {
            name: "A",
            batches: &a,
        }];
        let rels = [Relation {
            name: "A_n_A",
            source: 0,
            destination: 0,
            batches: &aa,
        }];
        assert!(matches!(
            layout(&types, &rels, None),
            Err(LayoutError::DanglingEndpoint {
                dropped: 1,
                before: 2,
                ..
            })
        ));
    }

    #[test]
    fn rows_not_numbered_in_order_are_refused() {
        let a = vec![
            RecordBatch::try_from_iter(vec![(
                "dense_id",
                Arc::new(UInt32Array::from(vec![0u32, 2, 1])) as ArrayRef,
            )])
            .unwrap(),
        ];
        let types = [VertexType {
            name: "A",
            batches: &a,
        }];
        assert!(matches!(
            layout(&types, &[], None),
            Err(LayoutError::NotDense {
                row: 1,
                found: 2,
                ..
            })
        ));
    }

    #[test]
    fn a_relation_between_types_pulls_them_together() {
        // Eight groups of eight per type, no edge inside a type, and every
        // vertex of group k of A related to every vertex of group k of B. The
        // only structure is across the types, so a pass that laid the types out
        // apart would find none; laid out as one graph, each group of A shares
        // its community with its group of B.
        let (groups, size) = (8u32, 8u32);
        let n = groups * size;
        let (a, b) = (vertices(n), vertices(n));
        let mut pairs = Vec::new();
        for k in 0..groups {
            for i in 0..size {
                for j in 0..size {
                    pairs.push((k * size + i, k * size + j));
                }
            }
        }
        let ab = edges(&pairs);
        let types = [
            VertexType {
                name: "A",
                batches: &a,
            },
            VertexType {
                name: "B",
                batches: &b,
            },
        ];
        let rels = [Relation {
            name: "A_p_B",
            source: 0,
            destination: 1,
            batches: &ab,
        }];
        let l = layout(&types, &rels, None).unwrap();
        for v in 0..n as usize {
            assert_eq!(
                l.cluster[0][v], l.cluster[1][v],
                "A{v} and B{v} are in one group and not one community"
            );
        }
        // And no community reaches across two groups: the partition is the
        // cross-type structure, cut at the finest level the budget allows.
        let mut group_of = std::collections::HashMap::new();
        for v in 0..n {
            let seen = *group_of.entry(l.cluster[0][v as usize]).or_insert(v / size);
            assert_eq!(
                seen,
                v / size,
                "a community spans groups {seen} and {}",
                v / size
            );
        }
    }
}
