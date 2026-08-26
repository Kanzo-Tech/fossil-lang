//! How many `dense_id`s a preassigned range per partition would waste.
//!
//! `lib.rs`'s `finalize_vertex` calls `collect().await` before it can number a
//! row, because `prepend_dense_id` needs every batch in one place to run a
//! global offset. That is the term that makes the peak proportional to N —
//! 20.72 GiB writing ten million vertices for 1.5 GB of corpus, measured from
//! outside on 2026-08-25. The proposed exit is to give each partition its own
//! range of ids up front so it can number and write without waiting for the
//! others.
//!
//! A range has to be a fixed quantum, because a partition's row count is not
//! known until it has finished — which is the very wait the change exists to
//! remove. A partition that does not fill its quantum leaves the difference as
//! **holes in the numbering**, and
//! `/docs/format/conventions/identity` publishes that `dense_id` is a gapless
//! `0..V−1`. This measures the difference.
//!
//! # What it measures
//!
//! 1. **The real split.** How many partitions the vertex phase runs in, and how
//!    many rows land in each — read off the physical plan DataFusion actually
//!    builds for the same DataFrame `finalize_vertex` builds, not off a model of
//!    it.
//! 2. **The shortfall against a quantum**, for the three quanta a writer could
//!    plausibly pick: the largest partition rounded up to `chunk_size`, the mean
//!    rounded up, and the source row count divided by the partition count (the
//!    only one of the three a writer could choose *before* running, because
//!    counting CSV rows is a scan and deduplicating them is not).
//! 3. **Peak RSS of that plan without the `collect()`**, so the holes have a
//!    price to be weighed against.
//!
//! # What it measured, 2026-08-26
//!
//! Fourteen partitions, and the split is a **hash repartition on `subject`** —
//! `RepartitionExec: partitioning=Hash([subject@0], 14)` in the plan below. So
//! the sizes do not follow the data at all: the synthetic pair and the
//! benchmark's own CSVs give the same fourteen counts to the row at ten
//! million, because the same ten million subject IRIs hash the same way whatever
//! graph joins them.
//!
//! | V | min | max | spread over the mean | holes at the tightest quantum |
//! | --- | --- | --- | --- | --- |
//! | 2,000 | 126 | 163 | 25.90% | 55,344 — **2,767% of V** |
//! | 50,000 | 3,433 | 3,636 | 5.68% | 7,344 — 14.69% |
//! | 1,000,000 | 70,992 | 72,063 | 1.50% | 32,192 — 3.22% |
//! | 5,000,000 | 355,848 | 358,890 | 0.85% | 46,272 — 0.93% |
//! | 10,000,000 | 712,815 | 716,571 | 0.53% | 35,200 — **0.35%** |
//!
//! The imbalance is a hash's, so it shrinks as V grows and the waste with it —
//! and below about fifty thousand the 4,096-row tile is the whole of it:
//! fourteen partitions cannot span fewer than 57,344 ids however few vertices
//! there are.
//!
//! **But the tightest quantum is not a quantum a writer can pick.** It is the
//! largest partition rounded up, and that is known only after the run. The one
//! figure available beforehand is the source row count over the partitions, and
//! the dedup collapses 8.1 rows into one: at ten million that quantum leaves
//! **71,027,072 holes, 710% of V** — seven ids thrown away for every one kept.
//!
//! And the peak, level by level down the plan at ten million, each in its own
//! process: `SortExec` **13.72 GiB**, the `FinalPartitioned` aggregate under it
//! **13.85**, the hash repartition under that **5.91**. Against 20.72 GiB for
//! the whole write. So **deleting the `collect()` leaves ~13.9 GiB**: the term
//! that follows N here is the dedup, which holds every distinct subject, and no
//! range scheme touches it. It agrees with the figure `bounded_context` already
//! carries — 15.7 GiB in the executor for 1.64 GiB of Arrow produced — from the
//! other side.
//!
//! **And that term is already boundable.** The same plan under `--memory-gib`,
//! which is `bounded_context`'s `FairSpillPool` replicated here:
//!
//! | budget | peak RSS | seconds |
//! | --- | --- | --- |
//! | unbounded | 13.72 GiB | 3.1 |
//! | 8 GiB | 4.84 GiB | 9.2 |
//! | 4 GiB | 3.67, 3.70 GiB | 7.4, 7.0 |
//! | 2 GiB | 2.30 GiB | 9.2 |
//!
//! It spills and it obeys. What the pool does **not** reach is the `Vec` the
//! `collect()` builds — `tests/spill.rs` says so in its own header, *"the Arrow
//! the writer holds is outside the pool either way"* — and that `Vec` is 1.64
//! GiB at ten million, not fourteen.
//!
//! # What it does NOT measure
//!
//! - **Not the write.** Nothing is encoded to Parquet here: the question is
//!   where the rows are when the ids are assigned, and the answer is upstream of
//!   any writer.
//! - **Not the edge phase.** Edges join the vertex tables by subject IRI and
//!   never see a `dense_id` until the vertex numbering exists, so the numbering
//!   question is entirely a vertex-phase one.
//! - **Not a decision about the sort.** Today the ids run in `subject` order
//!   because the plan sorts globally before it collects. Whether a range scheme
//!   keeps that order is a separate question from how much it wastes, and this
//!   answers only the second.
//!
//! # Running it
//!
//! Release only. Over the benchmark's own CSVs, which is the run that produced
//! the 20.72 GiB:
//!
//! ```text
//! cargo run --release -p fossil-df --example dense_id_ranges -- \
//!   --nodes …/graph-bench/corpus/nodes.csv --edges …/graph-bench/corpus/edges.csv
//! ```
//!
//! Or over a synthetic pair of the same shape, at any size:
//!
//! ```text
//! cargo run --release -p fossil-df --example dense_id_ranges -- --rows 1000000
//! ```
//!
//! The synthetic pair reproduces what the split depends on: two mappings unioned
//! into one type, ~7.1 edge rows per node so most rows are duplicate subjects,
//! and the same subject IRI shape. It does not reproduce the hyperbolic degree
//! distribution, and `--rows 10000000` beside the real pair is the check that
//! this does not change the answer.

// Deliberate numeric code: row counts and byte counts become `f64` for ratios
// and for a human-readable GiB in a report. Same declension `layout.rs` makes.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use datafusion::arrow::datatypes::DataType;
use datafusion::common::Column;
use datafusion::execution::memory_pool::{FairSpillPool, TrackConsumersPool};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::logical_expr::{Expr as DfExpr, Operator, binary_expr, cast};
use datafusion::physical_expr::expressions::lit as physical_lit;
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::physical_plan::{ExecutionPlan, common, displayable, execute_stream_partitioned};
use datafusion::prelude::{CsvReadOptions, DataFrame, SessionContext, col, lit};

/// The corpus tile size the manifest declares, and the granularity a wasted id
/// is charged in: a quantum that is not a multiple of it puts a tile boundary
/// inside a partition's range.
const CHUNK_SIZE: u64 = 4_096;

/// Peak resident set in bytes, from the OS. Same instrument and same reason as
/// `fossil-layout/examples/layout_memory.rs`: it is what the machine had to
/// find, allocator fragmentation included.
fn rss_bytes() -> u64 {
    let pid = std::process::id();
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        * 1024
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// The vertex projection `lib.rs` builds: `subject`, the props, and the three
/// layout placeholders. Column for column what `vertex_projection` emits, minus
/// the `dense_id` that is the whole question.
fn projection(id: &str, community: &str) -> Vec<DfExpr> {
    vec![
        binary_expr(
            lit("https://kanzo.tech/bench/n/"),
            Operator::StringConcat,
            cast(col(id), DataType::Utf8),
        )
        .alias("subject"),
        col(community).alias("community"),
        lit(0.0_f32).alias("x"),
        lit(0.0_f32).alias("y"),
        lit(0_u32).alias("cluster_id"),
    ]
}

/// The plan `finalize_vertex` builds for a two-mapping type: union the
/// projections, dedup on `subject` keeping the `subject`-sorted order.
///
/// The one thing left out is the `collect()`, which is what is being weighed.
async fn vertex_plan(ctx: &SessionContext, nodes: &Path, edges: &Path) -> DataFrame {
    let opts = CsvReadOptions::new().has_header(true);
    let n = ctx
        .read_csv(nodes.to_string_lossy().as_ref(), opts.clone())
        .await
        .expect("read the node list")
        .select(projection("id", "community"))
        .expect("project the node list");
    let e = ctx
        .read_csv(edges.to_string_lossy().as_ref(), opts)
        .await
        .expect("read the edge list")
        .select(projection("id", "community"))
        .expect("project the edge list");

    let df = n.union(e).expect("UNION ALL, as the writer does");
    let by_subject = vec![col("subject").sort(true, false)];
    let keep: Vec<DfExpr> = df
        .schema()
        .fields()
        .iter()
        .map(|f| DfExpr::Column(Column::new_unqualified(f.name().as_str())))
        .collect();
    df.distinct_on(vec![col("subject")], keep, Some(by_subject))
        .expect("dedup by subject, as the writer does")
}

/// Descend from the plan root to the topmost operator that still has more than
/// one partition — the last point at which rows exist in parallel, and
/// therefore the only place a per-partition range could be assigned — then
/// `level` steps further down its first child.
///
/// Everything above level 0 is the merge that a range scheme deletes. The
/// levels below it are there because the merge is **not the only** term that
/// grows with N: each one is executed in its own process, by `--level`, because
/// an allocator does not give RSS back between two runs in one.
fn parallel_level(plan: &Arc<dyn ExecutionPlan>, level: usize) -> Arc<dyn ExecutionPlan> {
    let mut node = Arc::clone(plan);
    while node.properties().partitioning.partition_count() == 1 {
        let Some(child) = node.children().first().map(|c| Arc::clone(c)) else {
            return node;
        };
        node = child;
    }
    for _ in 0..level {
        let Some(child) = node.children().first().map(|c| Arc::clone(c)) else {
            break;
        };
        node = child;
    }
    node
}

/// What a quantum of `q` ids per partition wastes on this split, charged in ids
/// and in whole tiles.
fn shortfall(name: &str, q: u64, rows: &[u64]) {
    let total: u64 = rows.iter().sum();
    let fits = rows.iter().all(|&r| r <= q);
    let span = q * rows.len() as u64;
    let holes = span.saturating_sub(total);
    let empty_tiles: u64 = rows
        .iter()
        .map(|&r| (q - r.min(q)) / CHUNK_SIZE)
        .sum::<u64>();
    println!(
        "  {name:<34} q = {q:>10}   {}   holes {:>12} ({:>6.2}% of V)   ids span 0..{}   whole empty tiles {empty_tiles}",
        if fits { "fits " } else { "SPILLS" },
        holes,
        100.0 * holes as f64 / total as f64,
        span - 1,
    );
}

/// A node list and an edge list of the benchmark's shape. See the header for
/// what this reproduces and what it does not.
fn synthesise(dir: &Path, rows: u64) -> (PathBuf, PathBuf) {
    let nodes = dir.join("nodes.csv");
    let edges = dir.join("edges.csv");
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    let mut f = std::io::BufWriter::new(std::fs::File::create(&nodes).expect("create nodes.csv"));
    writeln!(f, "id,community").expect("header");
    for id in 0..rows {
        writeln!(f, "{id},{}", id % 64).expect("a node row");
    }
    drop(f);

    // 7.1 edge rows per node, which is the benchmark's ratio at ten million
    // (71,024,690 rows against 10,000,000): the union is mostly duplicate
    // subjects and the dedup is most of the work.
    let mut f = std::io::BufWriter::new(std::fs::File::create(&edges).expect("create edges.csv"));
    writeln!(f, "id,community,target").expect("header");
    for id in 0..rows {
        for _ in 0..7 {
            writeln!(f, "{id},{},{}", id % 64, next() % rows).expect("an edge row");
        }
        if next() % 10 == 0 {
            writeln!(f, "{id},{},{}", id % 64, next() % rows).expect("an edge row");
        }
    }
    drop(f);
    (nodes, edges)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut nodes: Option<PathBuf> = None;
    let mut edges: Option<PathBuf> = None;
    let mut rows: u64 = 1_000_000;
    let mut level: usize = 0;
    let mut budget: Option<f64> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--nodes" => nodes = args.next().map(PathBuf::from),
            "--edges" => edges = args.next().map(PathBuf::from),
            "--rows" => rows = args.next().and_then(|v| v.parse().ok()).unwrap_or(rows),
            "--level" => level = args.next().and_then(|v| v.parse().ok()).unwrap_or(level),
            // `bounded_context`'s pool, replicated rather than called because
            // it is private to the crate. If the two ever disagree this example
            // is the one that is wrong.
            "--memory-gib" => budget = args.next().and_then(|v| v.parse::<f64>().ok()),
            other => panic!("unknown argument `{other}`"),
        }
    }

    // `<pid>` for `enrich_memory`'s reason: the directory is wiped before it is
    // seeded, so a fixed path means a second invocation deletes the fixture the
    // first is mid-measurement over.
    let scratch =
        std::env::temp_dir().join(format!("fossil_dense_id_ranges_{}", std::process::id()));
    let synthetic = nodes.is_none() || edges.is_none();
    let (nodes, edges) = match (nodes, edges) {
        (Some(n), Some(e)) => (n, e),
        _ => {
            let _ = std::fs::remove_dir_all(&scratch);
            std::fs::create_dir_all(&scratch).expect("create the fixture directory");
            eprintln!(
                "synthesising a {rows}-node pair under {}…",
                scratch.display()
            );
            synthesise(&scratch, rows)
        }
    };
    println!(
        "source  {} + {}   ({})",
        nodes.display(),
        edges.display(),
        if synthetic { "synthetic" } else { "as written" }
    );

    let baseline = rss_bytes();
    let ctx = match budget {
        None => SessionContext::new(),
        Some(gib) => {
            let bytes = (gib * 1024.0 * 1024.0 * 1024.0) as usize;
            let pool = TrackConsumersPool::new(
                FairSpillPool::new(bytes),
                std::num::NonZeroUsize::new(5).expect("5 is not zero"),
            );
            let runtime = RuntimeEnvBuilder::new()
                .with_memory_pool(Arc::new(pool))
                .build_arc()
                .expect("a runtime under a pool");
            let config = datafusion::prelude::SessionConfig::new()
                .set_bool("datafusion.optimizer.prefer_hash_join", false);
            SessionContext::new_with_config_rt(config, runtime)
        }
    };
    let partitions = ctx.state().config().target_partitions();
    println!(
        "target_partitions {partitions}   (DataFusion's default: one per core)   budget {}",
        budget.map_or_else(|| "unbounded".to_string(), |g| format!("{g} GiB"))
    );

    let df = vertex_plan(&ctx, &nodes, &edges).await;
    let plan = df.create_physical_plan().await.expect("a physical plan");
    println!("\nthe physical plan the writer runs, and where it stops being parallel:\n");
    print!("{}", displayable(plan.as_ref()).indent(true));

    let parallel = parallel_level(&plan, level);
    let count = parallel.properties().partitioning.partition_count();
    println!(
        "\nmeasuring at level {level}: {}   {count} partition(s)",
        parallel.name()
    );

    // Sampled from a second thread, for `layout_memory`'s reason: the number
    // that matters is the peak *inside* the run, and reading RSS after it
    // returns measures what survived.
    let stop = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(baseline));
    let sampler = {
        let stop = Arc::clone(&stop);
        let peak = Arc::clone(&peak);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(rss_bytes(), Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            peak.fetch_max(rss_bytes(), Ordering::Relaxed);
        })
    };

    // Project the rows down to one `u8` before counting them. The row COUNT is
    // what is being measured and the payload is not, so carrying the subject IRI
    // out of the plan would put a copy of the corpus in this process and make
    // the peak above report this example's arithmetic instead of the writer's.
    let counted: Arc<dyn ExecutionPlan> = Arc::new(
        ProjectionExec::try_new(vec![(physical_lit(1_u8), "one".to_string())], parallel)
            .expect("project to a single constant column"),
    );

    let started = std::time::Instant::now();
    let streams = execute_stream_partitioned(counted, ctx.task_ctx())
        .expect("execute, partition by partition");
    let mut handles = Vec::with_capacity(streams.len());
    for (index, stream) in streams.into_iter().enumerate() {
        handles.push(tokio::spawn(async move {
            let batches = common::collect(stream)
                .await
                .expect("a partition's batches");
            let n: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
            (index, n)
        }));
    }
    let mut per_partition = vec![0u64; count];
    for handle in handles {
        let (index, n) = handle.await.expect("a partition task");
        per_partition[index] = n;
    }
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    sampler.join().expect("sampler");

    let total: u64 = per_partition.iter().sum();
    let max = per_partition.iter().copied().max().unwrap_or(0);
    let min = per_partition.iter().copied().min().unwrap_or(0);
    let mean = total as f64 / count as f64;
    println!(
        "\nV = {total} vertices in {:.1}s   PEAK RSS {:.2} GiB (+{:.2} over baseline, and no collect() in it)",
        elapsed.as_secs_f64(),
        gib(peak.load(Ordering::Relaxed)),
        gib(peak.load(Ordering::Relaxed).saturating_sub(baseline)),
    );
    println!("\nrows per partition:");
    for (index, n) in per_partition.iter().enumerate() {
        println!(
            "  p{index:<3} {n:>12}   {:>7.3}% of V   {:+.2}% off the mean",
            100.0 * *n as f64 / total as f64,
            100.0 * (*n as f64 - mean) / mean,
        );
    }
    println!(
        "  min {min}   max {max}   mean {mean:.0}   spread {:.3}% of the mean",
        100.0 * (max - min) as f64 / mean,
    );

    println!("\nwhat a preassigned quantum wastes on this split:");
    let round_up = |v: u64| v.div_ceil(CHUNK_SIZE) * CHUNK_SIZE;
    shortfall(
        "largest partition, to a tile",
        round_up(max),
        &per_partition,
    );
    shortfall("the mean, to a tile", round_up(mean as u64), &per_partition);
    // The only quantum a writer could choose without running: the source rows
    // are countable by a scan, the deduplicated ones are not.
    let source_rows = ctx
        .read_csv(
            nodes.to_string_lossy().as_ref(),
            CsvReadOptions::new().has_header(true),
        )
        .await
        .expect("re-read the node list")
        .count()
        .await
        .expect("count the node list")
        + ctx
            .read_csv(
                edges.to_string_lossy().as_ref(),
                CsvReadOptions::new().has_header(true),
            )
            .await
            .expect("re-read the edge list")
            .count()
            .await
            .expect("count the edge list");
    println!(
        "  (the source rows before the dedup: {source_rows}, {:.1}× V)",
        source_rows as f64 / total as f64
    );
    shortfall(
        "source rows / partitions, to a tile",
        round_up((source_rows as u64).div_ceil(count as u64)),
        &per_partition,
    );

    if synthetic {
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
