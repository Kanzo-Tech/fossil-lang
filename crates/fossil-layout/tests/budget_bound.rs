//! The declared bound, measured: a run the budget lets through must stay inside
//! the number it was let through on.
//!
//! Everything else about the budget is arithmetic and can be asserted in
//! `budget.rs`. This one cannot: the claim is about a **resident set**, and the
//! only honest way to check it is to ask the OS what the machine had to find
//! while the pass ran. That is why this file exists at all — `cargo test` runs
//! the tests inside one binary in parallel, so a process peak sampled here would
//! also be counting whatever the tests beside it happened to allocate. A
//! separate integration target is a separate process, and this is the only test
//! in it.
//!
//! # Sampled, not marked
//!
//! `fossil_mem_probe::Probe` reports a peak, and it is a peak **over the phase
//! boundaries** — it calls `ps` at each `mark` and at `finish`, so a phase that
//! doubles its footprint in the middle and gives it back before the next mark is
//! invisible to it. That is the right instrument for attributing cost to phases
//! and the wrong one for asserting a ceiling was never crossed. So the peak here
//! comes from a sampler thread, the same shape `examples/layout_memory` uses.
//!
//! # What would make this red
//!
//! Any change that raises what the pass holds without raising
//! `estimated_peak_bytes` with it. That is the failure the budget exists to
//! prevent and the one nothing caught before: `--memory-gib 4` reached
//! `execute_graph` and stopped, `enrich_written_layout` wrote `let _ =
//! memory_bytes;`, and ten million vertices peaked at 8.39 GiB against the four
//! that were declared. Three parity tests stayed green throughout, because they
//! checked the corpus was correct when the cost was the entire objective.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use common::{dir, fixture};
use fossil_layout::layout::{enrich_layout_within, estimated_peak_bytes};

/// Vertices in the fixture. Large enough that Louvain builds a real hierarchy
/// over a planted partition — under a block of a thousand there is nothing to
/// merge — and small enough that the whole test is a few seconds of a CI run.
const VERTICES: u32 = 60_000;
/// Mean degree, and it is fourteen because that is the degree every calibration
/// point behind `estimated_peak_bytes` was measured at. A test at a degree the
/// constant was never fitted to would be measuring the extrapolation rather than
/// the bound.
const MEAN_DEGREE: u32 = 14;

/// Resident set from the OS, in bytes. `ps` rather than an allocator hook, for
/// the reason `fossil-mem-probe` gives: it is what the machine had to find,
/// fragmentation included, which is precisely the term a counting allocator
/// hides and precisely the term that made Louvain's number a surprise.
fn rss_bytes() -> u64 {
    let pid = std::process::id();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0)
        * 1024
}

#[allow(clippy::cast_precision_loss)] // a human-readable GiB in a failure message
fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// **The bound is respected, and this is the measurement that says so.**
///
/// The shape of the assertion matters as much as the number. `enrich_layout_within`
/// is handed exactly the estimate for this corpus as its budget, so it is let
/// through by the narrowest margin the check allows — and then what it actually
/// costs is sampled while it runs and compared against that same number. Handing
/// it a generous budget and watching it fit would assert nothing: it would pass
/// for a pass that held ten times as much.
#[test]
fn a_run_the_budget_admits_stays_inside_the_budget() {
    let f = fixture(dir("bound"), VERTICES, MEAN_DEGREE);

    // The corpus's own shape, from the same footers the check reads. Computing
    // the budget rather than writing a constant is what keeps this a statement
    // about the pass instead of about this fixture's size.
    let payload = std::fs::metadata(&f.targets[0].vertex_parquet)
        .expect("the staged vertex parquet")
        .len();
    let adjacency_rows = u64::from(VERTICES) * u64::from(MEAN_DEGREE / 2) * 2;
    let declared = estimated_peak_bytes(u64::from(VERTICES), adjacency_rows, payload);

    // Everything the fixture allocated is resident and none of it is the pass's,
    // so the baseline is taken here — after the corpus is on disk and before the
    // first footer is read.
    let baseline = rss_bytes();

    let stop = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(baseline));
    let sampler = {
        let (stop, peak) = (Arc::clone(&stop), Arc::clone(&peak));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(rss_bytes(), Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            peak.fetch_max(rss_bytes(), Ordering::Relaxed);
        })
    };

    enrich_layout_within(
        &fossil_layout::io::LocalFs,
        &f.targets,
        &f.adjacencies,
        Some(declared),
    )
    .expect("the pass is admitted by a budget computed from its own corpus");

    stop.store(true, Ordering::Relaxed);
    sampler.join().expect("the sampler thread");

    let held = peak.load(Ordering::Relaxed).saturating_sub(baseline);
    assert!(
        held <= declared,
        "the pass declared {:.3} GiB and held {:.3} GiB — the bound was admitted and then \
         exceeded, which is the defect `--memory-gib` exists to remove. \
         Either the pass grew or `estimated_peak_bytes` did not grow with it.",
        gib(declared),
        gib(held)
    );
}
