//! What it costs to renumber a corpus that has holes in it — read, reassign,
//! write — over a corpus that already exists.
//!
//! The other side of the `dense_id`-by-preassigned-range question. Ranges kill
//! the `collect().await` in `fossil-df`'s `finalize_vertex`; a partition that
//! does not fill its range leaves holes, and
//! `/docs/format/conventions/identity` publishes that `dense_id` is a gapless
//! `0..V−1`. One of the two exits is to keep the convention and **compact at
//! close**: one more pass that squeezes the holes out. This measures that pass.
//!
//! # What a compaction is
//!
//! An order-preserving relabelling. The `k`th surviving id becomes `k`, so
//! every id moves *down* by the number of holes before it and no two ids cross.
//! That is the whole of it, and it costs three things:
//!
//! 1. reading each vertex type's `dense_id` column and ranking it,
//! 2. gathering the vertex rows into the new order and rewriting every tile,
//! 3. sending both adjacency orientations through the same map, and re-cutting
//!    their tiles on the endpoint ranges that just moved.
//!
//! Every one of those is something `fossil_layout::layout::enrich_layout`
//! already does, on every run, for the Morton renumbering — which is why the
//! number this prints is worth having next to that pass's own. Run both.
//!
//! # What it measures
//!
//! Seconds and peak RSS per phase, through `FOSSIL_MEM_PROBE`, the same
//! instrument `enrich_layout` reports through. Plus the holes found, which on a
//! corpus fossil wrote is zero — today's writer numbers `0..V−1` from a
//! `collect()` and there is nothing to compact. That zero is the baseline the
//! measurement needs and not a defect: the *work* a compaction does is a
//! function of V and E, not of how many holes there are. `--holes-per-block`
//! injects a numbering with holes in it so the map is a real piecewise shift
//! rather than the identity.
//!
//! # What it measured, 2026-08-26
//!
//! Over `docs/public/bench/10000000` — ten million vertices, 2,442 tiles,
//! 142,049,380 adjacency rows across the two orientations — on a 14-core
//! machine, twice:
//!
//! ```text
//!   read dense_id                      0.1 s    0.05 G
//!   build the compaction map           0.0 s    0.16 G
//!   read vertices                      0.3 s    0.78 G
//!   gather + write vertex tiles        1.5 s    0.75 G
//!   remap adjacencies + write tiles    7.6 s    3.72 G
//!   total                              9.6 s    peak 3.72 G
//! ```
//!
//! **9.6 seconds and 3.72 GiB.** The whole write of that corpus is 404.4 s and
//! 20.72 GiB (`design/cost.mdx`), so a compaction is **2.4% of the wall clock**
//! and its peak fits inside the 8.39 GiB the layout pass — which does strictly
//! more than this — already reaches. A second run agreed to 0.0 s and 0.00 GiB;
//! injecting 46,059 holes cost 9.7 s and 3.53 G, inside the 0.2 GiB error bar.
//! With `--no-resort`: 8.9 s and 2.91 G.
//!
//! The vertex half is 1.9 s of it. Everything above that is the adjacency, and
//! most of the adjacency is `concat_batches` over 142M rows. **That sentence
//! used to end «— the same term `enrich_layout` pays, in the same place, for the
//! same reason», and it is no longer true of the other side.** The remap in
//! `layout.rs` holds one packed `Vec<u64>` per orientation and sorts it in
//! place; it calls neither `concat_batches` nor `lexsort_to_indices`, and its
//! phase went +1.95 GiB → +0.46 and 11.8 s → 6.5 for it. This example still
//! does, and that is now the largest difference between the two rather than
//! their shared term — the compaction is the one place left in this crate where
//! a whole orientation is materialised as Arrow before it is written.
//!
//! **Beside the pass it would be folded into**, `enrich_layout` over a
//! ten-million fixture at mean degree 14, same machine, same probe
//! (`examples/enrich_memory 10000000 14`, 2026-08-28): **140.2 s, peak 3.94
//! GiB**, of which `community_hierarchy` alone is 126.8 s and +1.93 G. Its
//! renumbering half — ranks, vertex read, gather, adjacency remap — is 12.4 s.
//! So a compaction standing alone is **6.9% of the pass that already runs on
//! every write**, and folded in it is a different value assigned in a loop that
//! already runs. The adjacency remap there costs 6.5 s against 7.6 s here on
//! the same number of rows — it used to cost 16.6, and the packed sort is where
//! the ten seconds went.
//!
//! That comparison has been re-read twice against a pass that keeps shrinking:
//! **289.0 s, peak 8.47 GiB** and 3.3% before the Louvain contraction stopped
//! building one hash table per community, **159.1 s, peak 5.08 GiB** and 6.1%
//! before the remap stopped holding an orientation as Arrow. The pass it is
//! measured against got smaller both times, not the compaction. The
//! compaction's own figures above are unchanged and were not re-run.
//!
//! **Verified rather than asserted.** Compacting a numbering with holes injected
//! into it produces output **byte-identical** to compacting the gapless one, at
//! 2,000 and at ten million; and the identity compaction reproduces the corpus's
//! own vertex tiles byte for byte. A relabelling that is correct is one whose
//! result does not depend on what it was relabelling from.
//!
//! # What it does NOT measure
//!
//! - **Not the community detection.** `enrich_layout` runs Louvain over the
//!   whole graph before it renumbers, and that is the term `layout_memory`
//!   isolates. A compaction needs none of it: the new order is the old order.
//! - **Not the marginal cost inside `enrich_layout`.** This is the pass
//!   standing alone, which is the pessimistic reading — the two renumberings
//!   compose into one, and a map that is already being built and applied costs
//!   nothing to build differently.
//! - **Not the identity index.** `enrich_layout` rewrites `index/` from the
//!   permutation it holds; so would a compaction, and it is the same gather
//!   again over two columns.
//! - **Not a cloud store.** Local files, `std::fs`, same as the pass.
//!
//! # Running it
//!
//! Release only, and it never writes inside the corpus it reads.
//!
//! ```text
//! FOSSIL_MEM_PROBE=1 cargo run --release -p fossil-layout --example compaction_pass -- \
//!   --corpus …/docs/public/bench/10000000
//! ```

// Deliberate numeric code: row and byte counts become `f64` for ratios and for
// a human-readable GiB. Same declension `layout.rs` makes for the same reason.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    // `num_rows()` is `i64` and a Parquet footer does not declare a negative
    // one; the `.max(0)` beside the cast is the whole handling it needs.
    clippy::cast_sign_loss
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, RecordBatch, RecordBatchReader as _, UInt32Array};
use arrow::compute::{
    SortColumn, cast, concat_batches, interleave_record_batch, lexsort_to_indices,
    take_record_batch,
};
use arrow::datatypes::DataType;
use fossil_df::files::batches_to_parquet;
use fossil_mem_probe::Probe;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// Rows per batch out of the Parquet reader. `layout.rs`'s `SCAN_BATCH_ROWS`.
const SCAN_BATCH_ROWS: usize = 8_192;

/// An id no row carries — a hole.
const NO_ROW: u32 = u32::MAX;

fn read_parquet(at: &Path) -> (Vec<RecordBatch>, arrow::datatypes::SchemaRef) {
    let reader = ParquetRecordBatchReaderBuilder::try_new(
        std::fs::File::open(at).unwrap_or_else(|e| panic!("open {}: {e}", at.display())),
    )
    .expect("a parquet footer")
    .with_batch_size(SCAN_BATCH_ROWS)
    .build()
    .expect("a parquet reader");
    let schema = reader.schema();
    let batches: Vec<RecordBatch> = reader
        .collect::<Result<_, _>>()
        .expect("decode every batch");
    (batches, schema)
}

/// One `u32` column, projected so no other column is decoded.
fn read_u32_column(at: &Path, name: &str) -> Vec<u32> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(
        std::fs::File::open(at).unwrap_or_else(|e| panic!("open {}: {e}", at.display())),
    )
    .expect("a parquet footer");
    let rows = builder.metadata().file_metadata().num_rows().max(0) as usize;
    let mask = ProjectionMask::columns(builder.parquet_schema(), [name]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(SCAN_BATCH_ROWS)
        .build()
        .expect("a parquet reader");
    let mut out = Vec::with_capacity(rows);
    for batch in reader {
        let batch = batch.expect("decode a batch");
        out.extend_from_slice(u32_column(&batch, name).values());
    }
    out
}

/// One column of a batch as `UInt32Array`, by name and never by position.
fn u32_column(batch: &RecordBatch, name: &str) -> UInt32Array {
    let index = batch
        .schema()
        .index_of(name)
        .unwrap_or_else(|_| panic!("the file has no `{name}` column"));
    let column = batch.column(index);
    let column = if column.data_type() == &DataType::UInt32 {
        Arc::clone(column)
    } else {
        cast(column, &DataType::UInt32).expect("cast to u32")
    };
    column
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("a cast to UInt32 yields a UInt32Array")
        .clone()
}

/// Replace named columns, keeping the schema and every column nobody named.
fn replace_columns(batch: &RecordBatch, replacements: &[(&str, ArrayRef)]) -> RecordBatch {
    let schema = batch.schema();
    let mut columns = batch.columns().to_vec();
    for (name, array) in replacements {
        let index = schema
            .index_of(name)
            .unwrap_or_else(|_| panic!("the file has no `{name}` column"));
        columns[index] = Arc::clone(array);
    }
    RecordBatch::try_new(schema, columns).expect("the replacement matches the schema")
}

fn write_parquet(at: &Path, batch: &RecordBatch) {
    if let Some(bytes) = batches_to_parquet(std::slice::from_ref(batch)).expect("encode one tile") {
        std::fs::write(at, bytes).unwrap_or_else(|e| panic!("write {}: {e}", at.display()));
    }
}

/// Which batch a global row index falls in, and where inside it. `layout.rs`'s
/// `locate`, and the same silent failure mode: an off-by-one here picks a real
/// row that is the wrong one.
fn locate(starts: &[u32], row: u32) -> (usize, usize) {
    let batch = match starts.binary_search(&row) {
        Ok(exact) => exact,
        Err(after) => after.saturating_sub(1),
    };
    (batch, (row - starts[batch]) as usize)
}

/// `vertex/<Type>/chunk{k}.parquet` in tile order — `k` numerically, because
/// `chunk10` sorts before `chunk2` as a string and the file order IS the
/// `dense_id` order.
fn vertex_tiles(dir: &Path) -> Vec<PathBuf> {
    let mut tiles: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            let k = name.strip_prefix("chunk")?.strip_suffix(".parquet")?;
            Some((k.parse().ok()?, path))
        })
        .collect();
    tiles.sort_unstable();
    tiles.into_iter().map(|(_, p)| p).collect()
}

fn main() {
    let mut corpus: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut chunk_size: u64 = 4_096;
    let mut holes_per_block: u64 = 0;
    let mut blocks: u64 = 14;
    let mut resort = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--corpus" => corpus = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--chunk-size" => {
                chunk_size = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(chunk_size);
            }
            "--holes-per-block" => {
                holes_per_block = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "--blocks" => blocks = args.next().and_then(|v| v.parse().ok()).unwrap_or(blocks),
            // A compaction map is monotone, so the re-sort below is provably a
            // no-op and this is the cost of the pass a compaction would
            // actually be. It is not the default because the pass it would be
            // folded into does sort — Morton is not monotone — and a default
            // that measures the cheaper thing is how a number gets quoted for
            // the wrong pass.
            "--no-resort" => resort = false,
            other => panic!("unknown argument `{other}`"),
        }
    }
    let corpus = corpus.expect("--corpus <dir> is required");
    let shift = chunk_size.trailing_zeros();
    assert!(
        chunk_size.is_power_of_two(),
        "a chunk size that is not a power of two addresses nothing"
    );

    // `<pid>`, for `enrich_memory`'s reason: a fixed path means a second
    // invocation deletes the output the first is mid-measurement over. And it
    // is never inside `--corpus`: this reads a corpus somebody else's
    // measurements depend on.
    let out = out.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("fossil_compaction_{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&out);

    let vertex_root = corpus.join("vertex");
    let types: Vec<PathBuf> = std::fs::read_dir(&vertex_root)
        .unwrap_or_else(|e| panic!("read {}: {e}", vertex_root.display()))
        .filter_map(|e| {
            let p = e.ok()?.path();
            p.is_dir().then_some(p)
        })
        .collect();
    assert_eq!(types.len(), 1, "this harness handles a one-type corpus");
    let vertex_dir = &types[0];
    let type_name = vertex_dir
        .file_name()
        .expect("a directory name")
        .to_string_lossy()
        .into_owned();

    let mut probe = Probe::new(&format!(
        "compaction_pass — {} over {}",
        type_name,
        corpus.display()
    ));
    if std::env::var("FOSSIL_MEM_PROBE").is_err() {
        eprintln!("note: FOSSIL_MEM_PROBE is unset, so this reports no phases.");
    }

    // ── 1. the numbering as it stands ──────────────────────────────────────
    //
    // The `dense_id` column of every tile, in tile order, and nothing else
    // decoded. This is the read a compaction cannot avoid: it has to know which
    // ids exist before it can rank them.
    let tiles = vertex_tiles(vertex_dir);
    let mut dense_of_row: Vec<u32> = Vec::new();
    for tile in &tiles {
        dense_of_row.extend(read_u32_column(tile, "dense_id"));
    }
    let rows = dense_of_row.len();
    probe.mark("read dense_id");

    // A numbering with holes in it, so the map below is a real piecewise shift
    // and not the identity. `blocks` blocks of equal size, each followed by
    // `holes_per_block` ids nothing carries — the shape a range scheme leaves
    // when each of `blocks` partitions underfills its quantum by that much.
    //
    // The endpoints in the adjacency name the ids as the corpus has them, so
    // the same shift has to be applied there before the map is consulted —
    // otherwise the injection is a numbering only half the corpus speaks.
    let block = (rows as u64).div_ceil(blocks);
    let inject = |id: u32| -> u32 {
        if holes_per_block == 0 {
            return id;
        }
        id + ((u64::from(id) / block) * holes_per_block) as u32
    };
    if holes_per_block > 0 {
        for id in &mut dense_of_row {
            *id = inject(*id);
        }
    }

    let numbered = dense_of_row
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m.saturating_add(1)) as usize;
    let holes = numbered - rows;

    // ── 2. the map ─────────────────────────────────────────────────────────
    //
    // `new[old] = rank of old among the ids that exist`. Order-preserving by
    // construction, which is the property that makes it a compaction and not a
    // permutation: an adjacency sorted on an endpoint is STILL sorted after it.
    let mut row_of_dense = vec![NO_ROW; numbered];
    for (row, &dense) in dense_of_row.iter().enumerate() {
        row_of_dense[dense as usize] = row as u32;
    }
    drop(dense_of_row);
    let mut new_of_old = vec![NO_ROW; numbered];
    let mut gather: Vec<u32> = Vec::with_capacity(rows);
    let mut new_dense: Vec<u32> = Vec::with_capacity(rows);
    for old in 0..numbered {
        let row = row_of_dense[old];
        if row == NO_ROW {
            continue;
        }
        new_of_old[old] = gather.len() as u32;
        new_dense.push(gather.len() as u32);
        gather.push(row);
    }
    drop(row_of_dense);
    probe.mark("build the compaction map");

    // ── 3. the vertex rows ─────────────────────────────────────────────────
    let mut batches: Vec<RecordBatch> = Vec::new();
    for tile in &tiles {
        batches.extend(read_parquet(tile).0);
    }
    let batch_refs: Vec<&RecordBatch> = batches.iter().collect();
    let starts: Vec<u32> = batches
        .iter()
        .scan(0u32, |acc, b| {
            let start = *acc;
            *acc += b.num_rows() as u32;
            Some(start)
        })
        .collect();
    probe.mark("read vertices");

    let vertex_out = out.join("vertex").join(&type_name);
    std::fs::create_dir_all(&vertex_out).expect("create the vertex output directory");
    let tile_count = (rows as u64).div_ceil(chunk_size);
    for k in 0..tile_count {
        let lo = (k << shift) as usize;
        let len = (rows - lo).min(chunk_size as usize);
        let picks: Vec<(usize, usize)> = gather[lo..lo + len]
            .iter()
            .map(|&row| locate(&starts, row))
            .collect();
        let tile = interleave_record_batch(&batch_refs, &picks).expect("gather one tile");
        let enriched = replace_columns(
            &tile,
            &[(
                "dense_id",
                Arc::new(UInt32Array::from(new_dense[lo..lo + len].to_vec())) as ArrayRef,
            )],
        );
        write_parquet(&vertex_out.join(format!("chunk{k}.parquet")), &enriched);
    }
    drop(batch_refs);
    drop(batches);
    probe.mark("gather + write vertex tiles");

    // ── 4. the adjacencies ─────────────────────────────────────────────────
    //
    // Both orientations, through the same map, re-sorted and re-cut on the
    // endpoint they are ordered by — `enrich_layout`'s loop, minus the type
    // lookup a one-type corpus does not need.
    //
    // **The sort is the conservative half of this number.** A compaction map is
    // monotone, so a file sorted on `src_dense` before it is sorted on
    // `src_dense` after it, and the `lexsort_to_indices` below is provably a
    // no-op. It is here because the pass it would live inside does it — Morton
    // is not monotone — and because leaving it out would measure the compaction
    // as cheaper than the thing it would actually be folded into.
    let edge_root = corpus.join("edge");
    let mut edge_rows = 0u64;
    if edge_root.is_dir() {
        for entry in std::fs::read_dir(&edge_root).expect("read the edge directory") {
            let rel_dir = entry.expect("an edge directory entry").path();
            if !rel_dir.is_dir() {
                continue;
            }
            let rel = rel_dir
                .file_name()
                .expect("a directory name")
                .to_string_lossy()
                .into_owned();
            for (file, key) in [
                ("by_source.parquet", "src_dense"),
                ("by_target.parquet", "dst_dense"),
            ] {
                let path = rel_dir.join(file);
                if !path.is_file() {
                    continue;
                }
                let (batches, schema) = read_parquet(&path);
                let combined = concat_batches(&schema, &batches).expect("one adjacency relation");
                drop(batches);
                edge_rows += combined.num_rows() as u64;

                let src = u32_column(&combined, "src_dense");
                let dst = u32_column(&combined, "dst_dense");
                let new_src: Vec<u32> = src
                    .values()
                    .iter()
                    .map(|&s| new_of_old[inject(s) as usize])
                    .collect();
                let new_dst: Vec<u32> = dst
                    .values()
                    .iter()
                    .map(|&d| new_of_old[inject(d) as usize])
                    .collect();
                drop(src);
                drop(dst);
                assert!(
                    !new_src.contains(&NO_ROW) && !new_dst.contains(&NO_ROW),
                    "an endpoint pointed at an id no vertex has"
                );

                let src_array: ArrayRef = Arc::new(UInt32Array::from(new_src));
                let dst_array: ArrayRef = Arc::new(UInt32Array::from(new_dst));
                let (first, second) = if key == "src_dense" {
                    (&src_array, &dst_array)
                } else {
                    (&dst_array, &src_array)
                };
                let order = resort.then(|| {
                    lexsort_to_indices(
                        &[
                            SortColumn {
                                values: Arc::clone(first),
                                options: None,
                            },
                            SortColumn {
                                values: Arc::clone(second),
                                options: None,
                            },
                        ],
                        None,
                    )
                    .expect("re-sort the adjacency")
                });
                let remapped = replace_columns(
                    &combined,
                    &[("src_dense", src_array), ("dst_dense", dst_array)],
                );
                drop(combined);
                let sorted = order.as_ref().map_or_else(
                    || remapped.clone(),
                    |order| take_record_batch(&remapped, order).expect("apply the sort"),
                );
                drop(remapped);
                drop(order);

                let stem = file.strip_suffix(".parquet").expect("a .parquet name");
                let prefix = out.join("edge").join(&rel).join(stem);
                std::fs::create_dir_all(&prefix).expect("create the edge tile directory");
                write_parquet(&out.join("edge").join(&rel).join(file), &sorted);

                let keys = u32_column(&sorted, key);
                let addresses = keys.values();
                let mut start = 0usize;
                while start < addresses.len() {
                    let tile = u64::from(addresses[start]) >> shift;
                    let mut end = start + 1;
                    while end < addresses.len() && u64::from(addresses[end]) >> shift == tile {
                        end += 1;
                    }
                    write_parquet(
                        &prefix.join(format!("tile{tile}.parquet")),
                        &sorted.slice(start, end - start),
                    );
                    start = end;
                }
            }
        }
    }
    probe.mark("remap adjacencies + write edge tiles");
    probe.finish();

    println!(
        "\n{type_name}: {rows} vertices numbered 0..{}, {holes} hole(s){}",
        numbered.saturating_sub(1),
        if holes_per_block > 0 {
            format!(" (injected: {blocks} blocks × {holes_per_block})")
        } else {
            String::new()
        }
    );
    println!(
        "{tile_count} vertex tiles rewritten, {edge_rows} adjacency rows remapped{}",
        if resort { "" } else { " (re-sort skipped)" }
    );
    println!("output under {}", out.display());
}
