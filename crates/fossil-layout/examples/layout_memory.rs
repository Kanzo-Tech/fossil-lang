//! What the layout core costs in memory, per level, with nothing else in the process.
//!
//! Writing a ten-million-vertex corpus peaks at 16.4 GiB for 713 MB of output
//! (measured 2026-08-04). That number is the whole build — the generator, `DuckDB`, the Parquet
//! writer and this — so it says where to look and nothing more. Larger-than-RAM is the
//! architecture's central untested claim, and the risk is here, in a Louvain that runs in memory
//! over the whole graph. This isolates it: no I/O, no database, one synthetic graph, and
//! the peak resident set after each phase.
//!
//! It is an example rather than a benchmark because the question is not "how fast" — it is "how
//! much, and does it follow N". Criterion would time it and tell us nothing.
//!
//!     cargo run --release --example layout_memory -- 1000000 [mean_degree]
//!
//! Reports, for each level of the hierarchy, the node and edge count going in and the peak RSS
//! reached. A level that costs more than the level below it is the finding: contraction is supposed
//! to shrink the graph.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use fossil_layout::layout::community_hierarchy;

/// Peak resident set in bytes, from the OS rather than from an allocator hook.
///
/// `ps -o rss=` is the number that matters here — it is what the machine had to find, including
/// the allocator's own fragmentation, which is precisely what a million small `HashMap`s would
/// cost and what a counting allocator would hide.
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

// Precision loss is the point: this prints a human-readable GiB figure for a
// benchmark, where the 53rd significant bit of a byte count is noise.
#[allow(clippy::cast_precision_loss)]
fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// A graph with communities in it, because Louvain on a uniform random graph merges nothing and
/// the hierarchy is one level deep — which would measure the easy case and report it as the cost.
///
/// Planted partition: `n / size` blocks, most edges inside a block and a few across. Deterministic
/// from a linear congruential generator so a rerun measures the same graph.
fn planted(n: u32, mean_degree: u32, block: u32) -> Vec<(u32, u32)> {
    let mut edges = Vec::with_capacity((n as usize * mean_degree as usize) / 2);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // Every `as u32` below is a modulus by a `u32`-derived bound, so the value
    // is in range by construction — `try_from` here would be an unwrap wearing a
    // longer name.
    #[allow(clippy::cast_possible_truncation)]
    for v in 0..n {
        let home = v / block;
        for _ in 0..(mean_degree / 2) {
            // Nine in ten edges stay inside the block; the tenth is what makes it one graph.
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

fn main() {
    let mut args = std::env::args().skip(1);
    let n: u32 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let mean_degree: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(14);
    let block: u32 = 128;

    let baseline = rss_bytes();
    println!("n = {n}, mean degree = {mean_degree}, blocks of {block}");
    println!("baseline RSS      {:.2} GiB", gib(baseline));

    let edges = planted(n, mean_degree, block);
    let after_edges = rss_bytes();
    println!(
        "edges built       {:>12} edges   {:.2} GiB  (+{:.2})",
        edges.len(),
        gib(after_edges),
        gib(after_edges - baseline)
    );
    println!("  the Vec alone   {:.2} GiB", gib(edges.len() as u64 * 8));

    // Sampled from a second thread while the call runs, because the number that matters is the
    // peak *inside* it. Reading RSS after it returns measures what survived, and a contraction
    // that allocates a million maps and frees them would be invisible — which is exactly the
    // shape this was written to look for.
    let stop = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(after_edges));
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

    let started = std::time::Instant::now();
    let levels = community_hierarchy(n, &edges);
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    sampler.join().expect("sampler");
    let after = peak.load(Ordering::Relaxed);

    println!(
        "community_hierarchy  {:.1}s   PEAK RSS {:.2} GiB  (+{:.2} over the edge list)",
        elapsed.as_secs_f64(),
        gib(after),
        gib(after.saturating_sub(after_edges))
    );
    println!("levels            {}", levels.len());
    for (i, level) in levels.iter().enumerate() {
        let communities = level.iter().copied().max().map_or(0, |m| m + 1);
        println!(
            "  level {i:>2}        {:>12} in -> {:>10} communities",
            level.len(),
            communities
        );
    }

    // Held to the end so the reported RSS includes them; without this the optimiser is free to
    // drop the hierarchy before the last `ps` runs and the number would be a lie.
    std::hint::black_box(&levels);
    std::hint::black_box(&edges);
}
