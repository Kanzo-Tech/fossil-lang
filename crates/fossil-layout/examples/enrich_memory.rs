//! What `enrich_layout`'s VERTEX half costs in memory, on a corpus this file
//! makes.
//!
//! `layout_memory` next door measures `community_hierarchy` — the partition,
//! which is integers. This measures the other half: reading a wide vertex file
//! and permuting it into Morton order. The 2026-08-22 run of the real thing put
//! `read vertices` at **+0.95 G** and `gather + replace` at **+0.51 G** of the
//! layout's +2.00 G, against `remap adjacencies`'s +0.17 G, so the vertex half
//! is where the pass's memory is and there was no way to isolate it short of
//! building a million-node corpus through `fossil run`.
//!
//! It is an example and not a benchmark for `layout_memory`'s reason: the
//! question is "how much, and against what", not "how fast".
//!
//! ```text
//! FOSSIL_MEM_PROBE=1 cargo run --release -p fossil-layout --example enrich_memory -- 1000000
//! ```
//!
//! # What makes the number mean anything
//!
//! **The rows are WIDE.** A vertex row in a real corpus is a subject IRI and
//! every property the mapping emitted; an adjacency row is two `u32`. That
//! asymmetry is the whole finding the real measurement made, so a fixture with
//! four integer columns would measure the easy case and report it as the cost.
//! Each row here carries a ~46-byte IRI and two string properties, which puts
//! the vertex file in the same order of bytes-per-row as the corpus that was
//! measured.
//!
//! **The permutation is real.** `community_hierarchy` runs on a planted
//! partition, so the gather is a genuine scatter across the file and not a near
//! identity that a sequential reader would get for free.
//!
//! # What it does NOT measure
//!
//! - **Not the executor.** `execute_graph` is the other 2.21 G under a 2 GiB
//!   cap and is not in this process at all.
//! - **Not larger-than-RAM.** The fixture IS the pass's input now — the batches
//!   are live throughout, because that is what a real run hands over — so the
//!   peak reported for `start` is the fixture's, not the pass's. Read the DELTAS.
//! - **Not throughput.** Release build, but the seconds column is there to say a
//!   phase ran, not what it costs.

use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use fossil_layout::layout::{AdjacencyTarget, Endpoint, VertexLayoutTarget};

/// Rows per `RecordBatch`. The executor's own batch size, so the fixture arrives
/// in the shape a real run's does — and [`locate`](fossil_layout) is exercised
/// across boundaries rather than inside one batch.
const WRITE_BATCH: u32 = 65_536;

// The report prints human-readable MB and bytes-per-row; the 53rd significant
// bit of a byte count is noise in a memory report.
#[allow(clippy::cast_precision_loss)]
fn main() {
    let mut args = std::env::args().skip(1);
    let rows: u32 = args
        .next()
        .as_deref()
        .unwrap_or("1000000")
        .parse()
        .expect("rows must be a u32");
    let degree: u32 = args
        .next()
        .as_deref()
        .unwrap_or("10")
        .parse()
        .expect("mean degree must be a u32");

    if std::env::var("FOSSIL_MEM_PROBE").is_err() {
        eprintln!(
            "note: FOSSIL_MEM_PROBE is unset, so `enrich_layout` will report nothing.\n\
             re-run as: FOSSIL_MEM_PROBE=1 cargo run --release -p fossil-layout \\\n\
             \x20   --example enrich_memory -- {rows} {degree}"
        );
    }

    // `<pid>` for the reason `tests/layout_renumber.rs` and
    // `fossil-cli/tests/common/mod.rs` carry one: the directory is WIPED before
    // it is seeded, so a fixed path means a second invocation deletes the
    // fixture the first is mid-measurement over. That file was measured red at
    // 30 concurrent processes, 30 failures; this is an example rather than a
    // test, so nothing runs two of it — but two `cargo run` in two terminals is
    // exactly the case, and the failure would look like a memory number.
    let root = std::env::temp_dir().join(format!("fossil_enrich_memory_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("chunks")).expect("create the fixture directory");

    eprintln!("building a {rows}-vertex fixture at mean degree {degree}…");
    let vertices = vertex_batches(rows);
    let edges = planted(rows, degree);
    eprintln!("  {} edges", edges.len());
    let by_source = adjacency_batches(&edges, Endpoint::Src);
    let by_target = adjacency_batches(&edges, Endpoint::Dst);
    drop(edges);

    // **The number `tests/budget.rs` asks for.** Its calibration table records a
    // Parquet size scaled by the decode factor the payload term used to carry,
    // because these five runs measured a file; this prints the thing the term is
    // now per byte of, so a re-run replaces the scaling with a measurement.
    let payload = arrow_bytes(&vertices);
    eprintln!(
        "  vertex payload {:.2} MB of Arrow ({:.0} bytes/row), adjacency {:.2} MB",
        mb(payload),
        payload as f64 / f64::from(rows),
        mb(arrow_bytes(&by_source))
    );

    let chunk_prefix = format!(
        "{}{}",
        path(&root.join("chunks")),
        std::path::MAIN_SEPARATOR
    );
    let target = VertexLayoutTarget {
        type_name: "Node".to_string(),
        batches: &vertices,
        chunk_prefix: chunk_prefix.clone(),
        chunk_size: 4_096,
    };
    let adjacencies = [
        ("by_source", Endpoint::Src, &by_source),
        ("by_target", Endpoint::Dst, &by_target),
    ]
    .map(|(dir, ordered_by, batches)| AdjacencyTarget {
        src_type: "Node".to_string(),
        label: "edge".to_string(),
        dst_type: "Node".to_string(),
        ordered_by,
        batches,
        tile_prefix: format!("{}{}", path(&root.join(dir)), std::path::MAIN_SEPARATOR),
        levels_prefix: format!("{}{}", path(&root), std::path::MAIN_SEPARATOR),
    });

    fossil_layout::layout::enrich_layout(std::slice::from_ref(&target), &adjacencies)
        .expect("enrich_layout runs on its own fixture");

    let chunks = std::fs::read_dir(root.join("chunks"))
        .expect("read the chunk directory")
        .count();
    eprintln!("wrote {chunks} vertex tiles under {chunk_prefix}");
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// What a batch set occupies, which is the payload term's own unit.
fn arrow_bytes(batches: &[RecordBatch]) -> u64 {
    batches
        .iter()
        .map(|b| b.get_array_memory_size() as u64)
        .sum()
}

// A human-readable megabyte figure for a report; the 53rd bit is noise here.
#[allow(clippy::cast_precision_loss)]
fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The vertex schema a W0b corpus has: the four columns the pass replaces, plus
/// the subject IRI and two properties that make a row wide. See the header.
fn vertex_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("note", DataType::Utf8, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]))
}

/// The vertex fixture in [`WRITE_BATCH`]-row batches — the shape the executor
/// hands the pass.
fn vertex_batches(rows: u32) -> Vec<RecordBatch> {
    let schema = vertex_schema();
    let mut batches = Vec::new();

    let mut start = 0u32;
    while start < rows {
        let len = WRITE_BATCH.min(rows - start);
        let ids: Vec<u32> = (start..start + len).collect();
        let subjects: Vec<String> = ids
            .iter()
            .map(|i| format!("https://example.org/corpus/node/{i:012}"))
            .collect();
        let names: Vec<String> = ids.iter().map(|i| format!("node number {i}")).collect();
        let notes: Vec<String> = ids
            .iter()
            .map(|i| format!("a property wide enough to matter, row {i}"))
            .collect();
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(UInt32Array::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(subjects)),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(notes)),
                Arc::new(Float32Array::from(vec![0.0f32; len as usize])),
                Arc::new(Float32Array::from(vec![0.0f32; len as usize])),
                Arc::new(UInt32Array::from(vec![0u32; len as usize])),
            ],
        )
        .expect("the fixture batch matches its schema");
        batches.push(batch);
        start += len;
    }
    batches
}

/// One adjacency orientation, sorted by the endpoint it is aligned by — which is
/// what the manifest's `ordered: true` claims and what the pass re-establishes
/// after renumbering.
fn adjacency_batches(edges: &[(u32, u32)], ordered_by: Endpoint) -> Vec<RecordBatch> {
    let mut sorted = edges.to_vec();
    match ordered_by {
        Endpoint::Src => sorted.sort_unstable(),
        Endpoint::Dst => sorted.sort_unstable_by_key(|&(s, d)| (d, s)),
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
    ]));
    sorted
        .chunks(WRITE_BATCH as usize)
        .map(|chunk| {
            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(UInt32Array::from(
                        chunk.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
                    )) as ArrayRef,
                    Arc::new(UInt32Array::from(
                        chunk.iter().map(|&(_, d)| d).collect::<Vec<_>>(),
                    )),
                ],
            )
            .expect("the adjacency batch matches its schema")
        })
        .collect()
}

/// A graph with communities in it, so the Morton renumbering is a real
/// permutation. Same planted partition and same LCG as `layout_memory`, for the
/// reason given there: Louvain on a uniform random graph merges nothing.
// Every `as u32` is a modulus by a `u32`-derived bound, so it is in range by
// construction — `try_from` here would be an unwrap wearing a longer name.
#[allow(clippy::cast_possible_truncation)]
fn planted(n: u32, mean_degree: u32) -> Vec<(u32, u32)> {
    let block = 1_000.min(n.max(1));
    let mut edges = Vec::with_capacity((n as usize * mean_degree as usize) / 2);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for v in 0..n {
        let home = v / block;
        for _ in 0..(mean_degree / 2) {
            let u = if next() % 10 == 0 {
                (next() % u64::from(n)) as u32
            } else {
                let base = home * block;
                let span = block.min(n - base);
                base + (next() % u64::from(span)) as u32
            };
            if u != v {
                edges.push((v, u));
            }
        }
    }
    edges
}
