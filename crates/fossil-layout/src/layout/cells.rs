//! The pyramid: **one summary row per region of the plane, per rung.**
//!
//! A rung is a mipmap level over the corpus, and the corpus was already a mipmap
//! without its interior rows. `dense_id` is a vertex's rank in the Morton order
//! of its position, so a quaternary cell at any depth is a **contiguous interval
//! of `dense_id`** — exactly, not approximately, and with no tree on disk to
//! walk. Cell `r` of rung `k` is `[r·4^k, (r+1)·4^k)`, a cell id is
//! `dense_id >> shift`, and a cell's parent is `r >> 2`.
//!
//! `/docs/design/holons` is the argument. Three sentences of it are load-bearing
//! here:
//!
//! - **The partition is arbitrary; the aggregation is exact.** Nothing below
//!   asks how the cells were chosen. Counts sum, weights sum, and the two
//!   obligations that follow from that are checkable by symmetric difference
//!   against the level underneath.
//! - **A cell is not an entity.** No IRI, no shape, no subject — nothing but the
//!   camera reads a rung, and no verb returns one.
//! - **Where a cell goes is the one thing that is chosen rather than aggregated.**
//!   The extent of a cell is known from its id, so the candidates are its centre
//!   and its members' centroid; this writes the centroid and *declares* it, the
//!   way a vertex's position is declared.
//!
//! # Why the mode does not aggregate and everything else does
//!
//! A texel averages colour. A category does not: the mean of two `cluster_id`s
//! is not a cluster. So a categorical is summarised as **the majority value and
//! what fraction of the cell holds it** — the answer indexed textures already
//! use — and the purity is not optional, because a mode without one is a lie at
//! the coarse end. Measured on com-DBLP, purity of a cell with respect to
//! `cluster_id` is 100% at and below the saturation scale and 7.7% at the root.
//!
//! **Which categorical is not in the column.** `mode` is a bare `u32` and a
//! `u32` carries no referent, so the tree names the channel its rungs summarise
//! and a reader resolves that name against the type's own `channels:` — the
//! column and the domain in one hop, from the entry that measured them. The
//! name reaches this module beside the array it is the name of; see
//! [`Pyramid::summarise`].
//!
//! And a mode cannot be computed from its children's modes, which is why every
//! rung is summarised from the payload in its own pass rather than folded up
//! from the rung below. The rows arrive in write order and `dense_id` ascends,
//! so a cell is a **contiguous run** and one linear scan answers a whole rung.
//!
//! # What it costs, measured
//!
//! **Nothing on the peak, and a little clock.** The two phases this module marks
//! bill `summarise cells` 0.1 s and `write cells` 0.7 s at one million vertices
//! and mean degree fourteen, release — stable across runs whose *totals* were
//! 12.7 s and 74.5 s on the same machine, which is why the attributable figure
//! is the phase and not the total.
//!
//! On memory the marginal cost was measured at four sizes, three runs each, and
//! came back **unresolvable**: negative at three of the four, `|Δ| ≤ 4.2 MB`
//! against a spread of up to 11.9 MB. `estimated_peak_bytes`'s
//! `VERTEX_ARRAY_BYTES` carries the pyramid's 3 bytes per vertex anyway, derived
//! rather than fitted, and says there why.
//!
//! **In a debug build the clock is not little**, and it lands on the test suite:
//! `tests/budget_bound.rs` went from about five seconds to sixty-seven when the
//! pyramid was switched on in its fixture, over sixty thousand vertices and
//! thirteen rungs. That is the per-rung re-walk, which is the price of not
//! holding a map — see [`Pyramid::write`]. It is the trade this module made on
//! purpose and the number is here so the next person weighing it has both
//! halves.
//!
//! (`examples/enrich_memory 1000000 14 16`, 2026-09-15, Mac16,8 — 14 cores,
//! 48 GiB, macOS 26.2 / Darwin 25.2.0.)

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, UInt32Array, UInt64Array};
use arrow::datatypes::{Field, Schema, SchemaRef};
use fossil_sinks::generated::{CELL_COLUMNS, QUOTIENT_COLUMNS, WriterColumn};
use fossil_sinks::manifest::{
    CoordinateSystem, HolonRung, HolonTree, QUOTIENT_PREFIX, TILES_FILE, arrow_type,
    declared_properties,
};

use super::pass::LayoutError;
use crate::io::{LayoutIo, Sink};
use fossil_df::files::TileWriter;

/// **How a cell's position was arrived at**, as the manifest declares it.
///
/// Everything else on a cell row is verified; this one is declared, because
/// where a summary goes on the plane is a choice and no amount of checking makes
/// a choice correct. A cell's extent is known from its id, so the two candidates
/// are its own centre and its members' centroid — different pictures, the first
/// regular and the second following the data. This is the second, and naming it
/// is what lets a reader re-derive it and find out that a cell has not moved.
const DERIVED_BY: &str = "cell-member-centroid";

/// One vertex type's pyramid, accumulated across the two halves of the pass.
///
/// **It is filled in two places because its two halves come from two places.**
/// Counts, positions and the categorical summary are a function of the rows,
/// which the vertex phase has; the internal weight and the quotient are a
/// function of the edges, which only exist remapped after the adjacency phase
/// has read them. So this is summarised there, absorbed here, and written last.
pub(crate) struct Pyramid {
    /// The declared base, carried so the tree this reports is the tree it built.
    vertices_per_cell: u64,
    /// **The name of the channel `mode` is the mode of**, carried from the
    /// array it was summarised over.
    ///
    /// It arrives beside `clusters` in [`Pyramid::summarise`] rather than as a
    /// constant here, and that is the point: this module never learns which
    /// column it is tallying, so the only thing it could invent is a second
    /// spelling of a name the pass has already declared. See
    /// `fossil_sinks::manifest::HolonTree::mode_channel`.
    mode_channel: String,
    /// Finest first, which is the order a manifest lists rungs in.
    rungs: Vec<Rung>,
}

/// One rung's cells, as the columns a row of it carries.
///
/// Struct-of-arrays and not a `Vec<Cell>`, because what is written is columns:
/// a tile is a slice of each of these and no row is ever a Rust value.
struct Rung {
    /// How many bits of `dense_id` this rung drops. A cell id is
    /// `dense_id >> shift`, and the rung above shifts two further.
    shift: u32,
    count: Vec<u32>,
    /// Summed in `f64` and divided once. Summing coordinates in `f32` loses the
    /// low bits of the last additions at the coarse end, where a cell holds
    /// hundreds of thousands of members — and the centroid is the one number
    /// here a reader re-derives to check that a cell has not moved.
    sum_x: Vec<f64>,
    sum_y: Vec<f64>,
    mode: Vec<u32>,
    purity: Vec<f32>,
    /// The edges absorbed: both ends in this cell. **Not a self-loop on the
    /// quotient** — an edge whose two ends share a cell is not an edge of that
    /// cell — which on a graph with any structure is the overwhelming majority.
    ///
    /// The only edge state a rung keeps. The quotient is not held: see
    /// [`Pyramid::write`].
    internal: Vec<u64>,
}

impl Rung {
    fn new(shift: u32, holons: usize) -> Self {
        Self {
            shift,
            count: vec![0; holons],
            sum_x: vec![0.0; holons],
            sum_y: vec![0.0; holons],
            mode: vec![0; holons],
            purity: vec![0.0; holons],
            internal: vec![0; holons],
        }
    }

    /// Close one cell's categorical tally into a mode and a purity.
    ///
    /// Ties go to the **lowest value**, and that is a decision and not an
    /// accident: a cell split evenly between two clusters has to summarise the
    /// same way in two processes, and `BTreeMap` walks ascending so a strict `>`
    /// keeps the first. It is the same tie-break `local_moving` takes, for the
    /// same reason.
    fn close(&mut self, cell: usize, tally: &BTreeMap<u32, u32>) {
        let mut best = (0u32, 0u32);
        for (&value, &seen) in tally {
            if seen > best.1 {
                best = (value, seen);
            }
        }
        self.mode[cell] = best.0;
        let members = self.count[cell];
        self.purity[cell] = if members == 0 {
            0.0
        } else {
            // The ratio a reader desaturates a mark by. Computed in `f32`
            // because that is what is written and a wider intermediate would
            // round differently in the two languages that read it.
            best.1 as f32 / members as f32
        };
    }
}

impl Pyramid {
    /// **Summarise the rows into every rung**, or `None` where the type earns no
    /// pyramid.
    ///
    /// `new_dense`, `xs`, `ys` and `clusters` are the four arrays the vertex
    /// phase already holds, in write order — so this costs one pass per rung
    /// over them and allocates the cells, which is `4/3` of the base.
    ///
    /// **`mode_channel` travels with `clusters` and is not a parameter beside
    /// it by accident**: the mode this writes is the mode of *that array*, and
    /// the name is what the pass declared the array as. A constant here would
    /// be a second spelling of it, which is the failure the declaration exists
    /// to remove — see [`HolonTree::mode_channel`].
    ///
    /// The plan comes from [`HolonTree`] and not from arithmetic here, which is
    /// the same rule the level pyramid is written under: one function chooses
    /// the rungs and both the writer and the manifest call it.
    pub(crate) fn summarise(
        vertex_count: u64,
        vertices_per_cell: u64,
        new_dense: &[u32],
        xs: &[f32],
        ys: &[f32],
        clusters: &[u32],
        mode_channel: &str,
    ) -> Option<Self> {
        HolonTree::base_bits(vertices_per_cell)?;
        if vertex_count <= vertices_per_cell {
            return None;
        }

        let mut rungs = Vec::new();
        for rung in 1u32.. {
            let holons = HolonTree::holons_at(vertex_count, vertices_per_cell, rung)?;
            let shift = HolonTree::shift_at(vertices_per_cell, rung)?;
            let mut here = Rung::new(shift, usize::try_from(holons).unwrap_or(usize::MAX));

            // One linear scan. A cell is a contiguous run of rows because
            // `new_dense` ascends — it is the write order and the address at
            // once — so the tally is one cell's and is closed when the run ends.
            // A gap in the numbering shortens a run and cannot split one.
            let mut tally: BTreeMap<u32, u32> = BTreeMap::new();
            let mut open: Option<usize> = None;
            for i in 0..new_dense.len() {
                let cell = (new_dense[i] >> shift) as usize;
                if open != Some(cell) {
                    if let Some(previous) = open {
                        here.close(previous, &tally);
                    }
                    tally.clear();
                    open = Some(cell);
                }
                here.count[cell] += 1;
                here.sum_x[cell] += f64::from(xs[i]);
                here.sum_y[cell] += f64::from(ys[i]);
                *tally.entry(clusters[i]).or_default() += 1;
            }
            if let Some(previous) = open {
                here.close(previous, &tally);
            }

            rungs.push(here);
            if holons <= 1 {
                break;
            }
        }

        Some(Self {
            vertices_per_cell,
            mode_channel: mode_channel.to_string(),
            rungs,
        })
    }

    /// **Write every rung and every quotient, and hand back the declaration.**
    ///
    /// `prefix` is the tree's own — the vertex type's prefix plus
    /// `HOLON_PREFIX` — and each rung lands under `<prefix>r{k}/` with the
    /// quotient, where there is one, under `<prefix>r{k}/quotient/`. Tiled at
    /// the type's own `chunk_size`, because a rung's ids are a shift of the
    /// payload's and there is no second number to declare.
    ///
    /// **One call returns the bytes and the document**, and that is the phase
    /// order made a signature. A rung's `holon_count` is arithmetic and a
    /// manifest could state it in front of the pass; a rung's *quotient edge
    /// count* is a measurement, and nothing can plan it. So the writer declares
    /// what it wrote, and the plan is the part it checks itself against rather
    /// than the part it reports.
    ///
    /// # Errors
    ///
    /// [`LayoutError`] on the first failing write, or on a prefix naming a
    /// scheme the pass cannot dereference.
    pub(crate) fn write(
        &mut self,
        io: &dyn LayoutIo,
        prefix: &str,
        chunk_size: u64,
        vertex_count: u64,
        edges: &[Edges<'_>],
    ) -> Result<HolonTree, LayoutError> {
        let cell_schema = schema_of(CELL_COLUMNS, "a cell row");
        let quotient_schema = schema_of(QUOTIENT_COLUMNS, "a quotient edge");
        let tile = usize::try_from(chunk_size).unwrap_or(usize::MAX).max(1);

        // **Write first, declare after.** A rung's quotient edge count is the
        // one number in the document that is a measurement, so the declaration
        // is built out of what the loop found rather than patched into a plan.
        let mut declared = Vec::with_capacity(self.rungs.len());
        for index in 0..self.rungs.len() {
            let at = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let here = format!("{prefix}{}", HolonTree::rung_prefix(at));
            io.ensure_prefix(&here)?;

            // **The edges, one rung at a time.** The quotient is a run-length of
            // the sorted cell pairs and the internal weight is what the runs
            // with `a == b` count, so both come out of one `Vec<u64>` that is
            // built, sorted and dropped inside this iteration. This is where a
            // `BTreeMap<(u32, u32), u64>` per rung stood: correct, and about
            // forty-eight bytes per surviving quotient edge held across every
            // rung at once, which is memory `estimated_peak_bytes` has no term
            // for. One packed `u64` per edge, transient, is eight — and it is
            // not live at the same time as the remap's own `Vec<u64>`, which is
            // exactly the argument `ADJACENCY_ROW_BYTES` already makes about
            // which of two terms the peak holds.
            //
            // It costs a second walk of the caller's Arrow per rung, and that
            // walk is possible at all because the pass takes batches rather than
            // files: re-reading a Parquet nine times to save a map would not
            // have been a trade worth making.
            let pairs = self.rungs[index].pair_edges(edges);
            let quotient = Self::write_quotient(io, &here, tile, &pairs, &quotient_schema)?;
            self.rungs[index].absorb_runs(&pairs);

            let rung = &self.rungs[index];
            let rows = rung.count.len();
            let url = format!("{here}{TILES_FILE}");
            let mut writer = open_tiles(io, &url, Arc::clone(&cell_schema))?;
            let mut lo = 0usize;
            while lo < rows {
                let hi = (lo + tile).min(rows);
                writer
                    .tile(&rung.rows(&cell_schema, lo, hi))
                    .map_err(write_err(&url))?;
                lo = hi;
            }
            writer.finish().map_err(write_err(&url))?;

            // The rung, as the manifest states it: the count it holds, and a
            // quotient only where one was written.
            let mut entry = HolonRung::at(at, rows as u64, declared_properties(CELL_COLUMNS));
            if let Some(count) = quotient {
                entry = entry.with_quotient(count, declared_properties(QUOTIENT_COLUMNS));
            }
            declared.push(entry);
        }

        // And the plan, from the same function a reader derives the tree with.
        // It is not what is reported — the quotient counts above are not in it —
        // it is what the writer holds itself against: the rungs it wrote and the
        // rungs `ceil(V / 4^k)` names have to be the same list, and the writer
        // is the only place that can notice they are not.
        debug_assert_eq!(
            HolonTree::planned(
                vertex_count,
                self.vertices_per_cell,
                Vec::new(),
                &declared_properties(CELL_COLUMNS),
            )
            .map(|plan| plan.rungs.iter().map(|r| r.holon_count).collect::<Vec<_>>()),
            Some(declared.iter().map(|r| r.holon_count).collect::<Vec<_>>()),
            "the pyramid this wrote is not the pyramid the arithmetic names",
        );

        // The relations the tree summarises, by label. Named in the manifest
        // because mass conservation has no population without them: every edge
        // of these is somewhere in every rung — as a quotient edge, or absorbed
        // into the internal weight of the cell holding both its ends — and a
        // tree over a different relation is a different tree that opens,
        // addresses and draws exactly the same.
        let mut relations: Vec<String> = edges.iter().map(|e| e.label.to_string()).collect();
        relations.sort_unstable();
        relations.dedup();

        // And the two things about a rung that the rung itself cannot say. The
        // position is a choice, so it is declared; the `mode` is a `u32` whose
        // referent is nowhere in the column, so the channel it summarises is
        // NAMED — the name the pass declared the partition under, carried here
        // since `summarise` rather than spelled a second time.
        Ok(HolonTree::new(self.vertices_per_cell, relations, declared)
            .with_coordinates(vec![CoordinateSystem::derived(
                "holon", "x", "y", DERIVED_BY,
            )])
            .with_mode_channel(self.mode_channel.clone()))
    }

    /// One rung's quotient, or `None` where the rung has no cross edges.
    ///
    /// **Tiled on the source cell's own tile**, which is the adjacency's rule
    /// and not a new one: tile `j` holds every quotient edge whose `src_cell`
    /// falls in cell tile `j`, so a reader that can address a rung can address
    /// its quotient with the arithmetic it already has. `pairs` is sorted, so it
    /// is source-ascending and every tile is a run.
    ///
    /// A **weight** per distinct pair and not a row per edge: the run length IS
    /// the weight, which is what makes the quotient smaller than the relation
    /// rather than a copy of it in other names.
    fn write_quotient(
        io: &dyn LayoutIo,
        rung_prefix: &str,
        tile: usize,
        pairs: &[u64],
        schema: &SchemaRef,
    ) -> Result<Option<u64>, LayoutError> {
        // Skip the runs that are absorbed; what is left is the quotient.
        let crossing = |&key: &u64| (key >> 32) as u32 != key as u32;
        if !pairs.iter().any(crossing) {
            // No file, and the manifest says `None` rather than zero. A rung
            // whose cells have no edges between them and a rung whose writer
            // published none are different claims, and a 404 cannot tell them
            // apart on its own.
            return Ok(None);
        }
        let here = format!("{rung_prefix}{QUOTIENT_PREFIX}");
        io.ensure_prefix(&here)?;
        let url = format!("{here}{TILES_FILE}");
        let mut writer = open_tiles(io, &url, Arc::clone(schema))?;
        let width = u32::try_from(tile).unwrap_or(u32::MAX).max(1);

        let mut edges = 0u64;
        let mut src = Vec::new();
        let mut dst = Vec::new();
        let mut weight = Vec::new();
        let mut open: Option<u32> = None;
        let mut start = 0usize;
        while start < pairs.len() {
            let mut end = start + 1;
            while end < pairs.len() && pairs[end] == pairs[start] {
                end += 1;
            }
            let (a, b) = ((pairs[start] >> 32) as u32, pairs[start] as u32);
            if a != b {
                let at = a / width;
                if open.is_some_and(|previous| previous != at) {
                    writer
                        .tile(&quotient_rows(schema, &mut src, &mut dst, &mut weight))
                        .map_err(write_err(&url))?;
                }
                open = Some(at);
                src.push(a);
                dst.push(b);
                weight.push((end - start) as u64);
                edges += 1;
            }
            start = end;
        }
        if !src.is_empty() {
            writer
                .tile(&quotient_rows(schema, &mut src, &mut dst, &mut weight))
                .map_err(write_err(&url))?;
        }
        writer.finish().map_err(write_err(&url))?;
        Ok(Some(edges))
    }
}

/// **One self-relation of a type, as the pyramid re-walks it.**
///
/// The remapped orientation is not kept — the pass writes its tiles and drops
/// the `Vec<u64>` — so the pyramid walks the caller's Arrow again and remaps
/// through the same array the pass did. `batches` is the SOURCE-ordered half
/// only: the two orientations are one relation stored twice, and counting both
/// would double every weight in the tree.
pub(crate) struct Edges<'a> {
    /// The relation's label, which the tree declares so its conserved mass has a
    /// named population.
    pub(crate) label: &'a str,
    /// The source-ordered rows, as the executor produced them.
    pub(crate) batches: &'a [RecordBatch],
    /// `old dense_id → new dense_id`, this type's own. Both endpoints go through
    /// it because both are this type: a cell is an interval of ONE numbering.
    pub(crate) map: &'a [u32],
}

impl Rung {
    /// **Every edge as a packed, sorted, cell-space pair** — low cell in the
    /// high half, so a run is one distinct pair.
    ///
    /// Ordered `(min, max)` rather than `(src, dst)` because the tree is
    /// undirected: an edge between two cells is one quotient edge whichever end
    /// the relation wrote first, and normalising here is what makes the two ends
    /// unable to disagree about it.
    ///
    /// A row whose endpoint is outside the mapping is **skipped**. The pass
    /// refuses a dangling endpoint before this runs — see
    /// `LayoutError::DanglingEndpoint` — so this is unreachable rather than
    /// lenient, and a summary is the wrong place to discover it.
    fn pair_edges(&self, relations: &[Edges<'_>]) -> Vec<u64> {
        let rows: usize = relations
            .iter()
            .flat_map(|e| e.batches.iter())
            .map(RecordBatch::num_rows)
            .sum();
        let mut pairs = Vec::with_capacity(rows);
        for relation in relations {
            for batch in relation.batches {
                let (Some(src), Some(dst)) = (
                    u32_values(batch, "src_dense"),
                    u32_values(batch, "dst_dense"),
                ) else {
                    continue;
                };
                for (&u, &v) in src.iter().zip(dst) {
                    let (Some(&u), Some(&v)) =
                        (relation.map.get(u as usize), relation.map.get(v as usize))
                    else {
                        continue;
                    };
                    let (a, b) = (u >> self.shift, v >> self.shift);
                    pairs.push((u64::from(a.min(b)) << 32) | u64::from(a.max(b)));
                }
            }
        }
        pairs.sort_unstable();
        pairs
    }

    /// Count the absorbed runs into [`Self::internal`].
    ///
    /// The other half of what [`Self::pair_edges`] produced: a run whose two
    /// cells are the same cell is mass that stops being an edge. **Not a
    /// self-loop on the quotient** — an edge whose ends share a cell is not an
    /// edge of that cell — and on the planted fixture
    /// `crates/fossil-layout/tests/holons.rs` evaluates, 1,088 of 1,095 edges
    /// are absorbed, so a writer that kept them would be wrong about the
    /// overwhelming majority rather than in an edge case.
    fn absorb_runs(&mut self, pairs: &[u64]) {
        for &key in pairs {
            let a = (key >> 32) as u32;
            if a == key as u32
                && let Some(slot) = self.internal.get_mut(a as usize)
            {
                *slot += 1;
            }
        }
    }

    /// One tile of cell rows — the slice `lo..hi` of every column.
    fn rows(&self, schema: &SchemaRef, lo: usize, hi: usize) -> RecordBatch {
        let ids: Vec<u32> = (lo..hi)
            .map(|r| u32::try_from(r).unwrap_or(u32::MAX))
            .collect();
        let xs: Vec<f32> = (lo..hi)
            .map(|r| centre(self.sum_x[r], self.count[r]))
            .collect();
        let ys: Vec<f32> = (lo..hi)
            .map(|r| centre(self.sum_y[r], self.count[r]))
            .collect();
        columns(
            schema,
            &[
                ("cell_id", Arc::new(UInt32Array::from(ids))),
                ("x", Arc::new(Float32Array::from(xs))),
                ("y", Arc::new(Float32Array::from(ys))),
                (
                    "count",
                    Arc::new(UInt32Array::from(self.count[lo..hi].to_vec())),
                ),
                (
                    "internal",
                    Arc::new(UInt64Array::from(self.internal[lo..hi].to_vec())),
                ),
                (
                    "mode",
                    Arc::new(UInt32Array::from(self.mode[lo..hi].to_vec())),
                ),
                (
                    "purity",
                    Arc::new(Float32Array::from(self.purity[lo..hi].to_vec())),
                ),
            ],
        )
    }
}

/// One `u32` column of a batch, or `None` where the batch does not declare it.
///
/// Total rather than fallible: the pass has already refused an adjacency that is
/// not two `u32` columns — see `LayoutError::AdjacencyShape` — so a batch
/// reaching here without them cannot happen, and inventing an error variant for
/// it would be one nobody can reach.
fn u32_values<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a [u32]> {
    let index = batch.schema().index_of(name).ok()?;
    Some(
        batch
            .column(index)
            .as_any()
            .downcast_ref::<UInt32Array>()?
            .values(),
    )
}

/// A cell's centroid on one axis — the members' mean, and the origin for a cell
/// with no members.
///
/// An empty cell is a real row and not a hole: the rung is a dense `0..n` so
/// that a cell id is a shift, and a region of the plane the corpus put nothing
/// in still has an address. `count` is what tells a reader it is empty, which is
/// why the count is a column.
fn centre(sum: f64, members: u32) -> f32 {
    if members == 0 {
        0.0
    } else {
        (sum / f64::from(members)) as f32
    }
}

/// One tile of quotient rows, draining the three column buffers.
fn quotient_rows(
    schema: &SchemaRef,
    src: &mut Vec<u32>,
    dst: &mut Vec<u32>,
    weight: &mut Vec<u64>,
) -> RecordBatch {
    columns(
        schema,
        &[
            ("src_cell", Arc::new(UInt32Array::from(std::mem::take(src)))),
            ("dst_cell", Arc::new(UInt32Array::from(std::mem::take(dst)))),
            (
                "weight",
                Arc::new(UInt64Array::from(std::mem::take(weight))),
            ),
        ],
    )
}

/// Order named arrays into the schema's own order.
///
/// **By name, never by position.** The schema comes out of `corpus.bnf`'s
/// generated table, so the file's column order is the data file's; a writer that
/// listed its arrays positionally would emit the right bytes under the wrong
/// names the day a column is inserted rather than appended.
///
/// # Panics
///
/// If a declared column was not supplied, naming it. That is a defect in this
/// module rather than in a corpus — a column added to `corpus.bnf` with no
/// writer — and the panic is what says which one.
fn columns(schema: &SchemaRef, supplied: &[(&str, ArrayRef)]) -> RecordBatch {
    let ordered: Vec<ArrayRef> = schema
        .fields()
        .iter()
        .map(|field| {
            let found = supplied.iter().find(|(name, _)| *name == field.name());
            let Some((_, array)) = found else {
                panic!(
                    "`{}` is declared in corpus.bnf and this writer supplies no array for it",
                    field.name()
                )
            };
            Arc::clone(array)
        })
        .collect();
    RecordBatch::try_new(Arc::clone(schema), ordered)
        .expect("the arrays are built to the schema's own types and one length")
}

/// The Arrow schema of one declared column set.
///
/// **Names and types both, off `corpus.bnf`.** Taking the names from the data
/// file and spelling the types here would leave half the set stated twice, which
/// is the thing that file exists to remove;
/// `crates/fossil-sinks/src/manifest.rs, arrow_type` is the other half of the
/// mapping the manifest already writes through.
///
/// # Panics
///
/// If a declared `data_type` has no Arrow type. Unreachable from a corpus:
/// `every_declared_column_type_has_an_arrow_type` in `fossil-sinks` fails first,
/// which is where a spelling with no mapping is a build failure rather than a
/// write failure.
fn schema_of(declared: &[WriterColumn], what: &str) -> SchemaRef {
    let fields: Vec<Field> = declared
        .iter()
        .map(|column| {
            let dt = arrow_type(column.data_type).unwrap_or_else(|| {
                panic!(
                    "{what}: `{}` declares type `{}`, which has no arrow type",
                    column.name, column.data_type
                )
            });
            Field::new(column.name, dt, false)
        })
        .collect();
    Arc::new(Schema::new(fields))
}

/// Open one tile set for writing. The pass's own helper, duplicated here rather
/// than made `pub(crate)` across the module boundary — see `pass.rs`.
fn open_tiles(
    io: &dyn LayoutIo,
    url: &str,
    schema: SchemaRef,
) -> Result<TileWriter<Sink>, LayoutError> {
    TileWriter::new(io.create(url)?, schema).map_err(write_err(url))
}

/// Parquet errors from a write, as a [`LayoutError::Write`].
fn write_err(url: &str) -> impl Fn(parquet::errors::ParquetError) -> LayoutError + '_ {
    move |source| LayoutError::Write {
        target: url.to_string(),
        source,
    }
}
