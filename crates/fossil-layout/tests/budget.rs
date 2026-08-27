//! Integration: what a declared memory budget does to the layout pass, and what
//! it must never do.
//!
//! `--memory-gib` used to reach `execute_graph` and stop there — `fossil-cli`'s
//! `enrich_written_layout` took the run's `memory_bytes` and wrote `let _ =
//! memory_bytes;`. At ten million vertices a declared 4 GiB run peaked at 8.39
//! GiB, and a bound overshot by more than double is worse than no bound at all,
//! because the person who declared it stopped worrying.
//!
//! The pass has no spill path — it holds Rust `Vec`s, and there is no disk
//! manager under it to hand them to — so a budget can only be honoured here by
//! *refusing*. These three assertions are that refusal's contract:
//!
//! 1. over budget is an error, and it happens **before anything is written**;
//! 2. under budget the corpus is byte-for-byte the corpus an unbounded run
//!    writes, because a budget that changed the output would make `fossil run`
//!    and the browser tab — which has no flag to declare one with — disagree
//!    exactly when a budget was declared;
//! 3. the estimate the refusal is made on **over**-estimates the one measurement
//!    it is calibrated against, rather than under-estimating it.
//!
//! The fourth — that a run which is allowed through then stays inside the number
//! — is a resident-set measurement and cannot share a process with anything
//! else, so it lives alone in `budget_bound.rs`.

mod common;

use std::fs;
use std::path::Path;

use common::{dir, fixture};
use fossil_layout::layout::{LayoutError, enrich_layout_within, estimated_peak_bytes};

/// Every file under `root`, as `(relative path, bytes)`, sorted — so two corpora
/// compare as one value and a difference names the file it is in.
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(at: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(at)
            .expect("read the corpus directory")
            .map(|e| e.expect("a directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .expect("a path under the root")
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, fs::read(&path).expect("read a corpus file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// **The refusal, and that it happens first.**
///
/// One byte is a budget no corpus fits in, so what this pins is not the
/// arithmetic — that is the third test — but the *order*: the pass reads three
/// Parquet footers, decides, and returns without having decoded a column chunk
/// or created a tile. A refusal at second zero is a different thing to offer a
/// caller than an out-of-memory at second two hundred, and the corpus it leaves
/// behind is the one the writer staged rather than half of a rewritten one.
#[test]
fn a_budget_the_corpus_cannot_fit_is_refused_before_anything_is_written() {
    let f = fixture(dir("refused"), 20_000, 14);
    let before = tree(&f.root);

    let err = enrich_layout_within(
        &fossil_layout::io::LocalFs,
        &f.targets,
        &f.adjacencies,
        Some(1),
    )
    .expect_err("one byte is not a budget any corpus fits in");

    match err {
        LayoutError::OverBudget {
            vertex_count,
            adjacency_rows,
            needed_bytes,
            declared_bytes,
        } => {
            assert_eq!(vertex_count, 20_000, "the footers were read");
            assert!(adjacency_rows > 0, "both orientations were counted");
            assert!(
                needed_bytes > declared_bytes,
                "the error is only reachable when it does not fit"
            );
        }
        other => panic!("expected OverBudget, got {other:?}"),
    }

    assert_eq!(
        tree(&f.root),
        before,
        "a refusal must leave the staged corpus exactly as it found it"
    );
}

/// **The budget decides whether the pass runs, never what it writes.**
///
/// This is the invariant that rules out the other design. Degrading under
/// pressure — fewer levels, coarser tiles, a second pass — is the obvious way to
/// honour a budget, and it is unavailable here: `fossil run` and the playground
/// tab write the same tree byte for byte through the same `LayoutIo`, and only
/// one of them has a `--memory-gib`. A budget that changed the output would
/// break that identity precisely when someone declared one.
///
/// So: two identical staged corpora, one run unbounded and one under a budget it
/// fits in, and every byte of every file has to match.
#[test]
fn a_budget_it_fits_in_writes_the_corpus_an_unbounded_run_writes() {
    let unbounded = fixture(dir("identity_unbounded"), 20_000, 14);
    let bounded = fixture(dir("identity_bounded"), 20_000, 14);

    enrich_layout_within(
        &fossil_layout::io::LocalFs,
        &unbounded.targets,
        &unbounded.adjacencies,
        None,
    )
    .expect("the unbounded control");

    // Generous on purpose: what is under test is that passing a budget changes
    // nothing, not where the threshold is.
    enrich_layout_within(
        &fossil_layout::io::LocalFs,
        &bounded.targets,
        &bounded.adjacencies,
        Some(64 << 30),
    )
    .expect("a budget this corpus fits inside");

    let a = tree(&unbounded.root);
    let b = tree(&bounded.root);
    assert_eq!(
        a.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        b.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        "the two runs wrote different files"
    );
    for ((path, left), (_, right)) in a.iter().zip(&b) {
        assert_eq!(
            left, right,
            "`{path}` differs between the bounded and the unbounded run"
        );
    }
}

/// **The estimate over-estimates both runs it is calibrated on.**
///
/// The direction is the whole assertion. Too large refuses a run that would have
/// fitted, and whoever declared the budget raises it and tries again; too small
/// accepts a run and lets it exceed the number they were promised, which is the
/// defect the budget exists to remove. So this is `>=` and never `abs() < eps`.
///
/// The measurements, both `FOSSIL_MEM_PROBE=1 cargo run --release -p
/// fossil-layout --example enrich_memory -- <N> 14` on a Mac16,8 — 14 cores, 48
/// GiB, macOS 25.2.0 — on 2026-08-27:
///
/// | N | vertex Parquet | process peak | at `start` | **the pass** |
/// | --- | --- | --- | --- | --- |
/// | 2,000,000 | 235.87 MB | 2.09 GiB | 0.50 GiB | 1.59 GiB |
/// | 10,000,000 | 1,187.75 MB | 8.82 GiB | 1.33 GiB | 7.49 GiB |
///
/// **What is bounded is the pass, not the process**, which is why the fourth
/// column is the one asserted against. `enrich_memory` builds its fixture in the
/// same process and the generator's pages are still resident when the pass
/// starts; in a real `fossil run` that resident set belongs to `execute_graph`,
/// which carries a `FairSpillPool` and a budget of its own. That composition is
/// the gap this does not close and `/docs/design/streaming` names: one declared
/// number is spent twice, once by each half of the write path, and neither half
/// knows what the other took.
#[test]
fn the_estimate_over_estimates_the_runs_it_is_calibrated_on() {
    // (vertices, adjacency rows over both orientations, vertex Parquet bytes,
    //  what the pass itself added)
    const MEASURED: [(u64, u64, u64, u64); 2] = [
        (2_000_000, 27_974_508, 235_870_000, 1_707_296_522), // 1.59 GiB
        (10_000_000, 139_874_560, 1_187_750_000, 8_042_847_109), // 7.49 GiB
    ];

    for (vertices, rows, payload, measured) in MEASURED {
        let needed = estimated_peak_bytes(vertices, rows, payload);
        assert!(
            needed >= measured,
            "at {vertices} vertices the estimate ({needed} bytes) is BELOW the {measured} bytes \
             measured — a budget check built on it would wave through a run that then exceeds \
             the declaration"
        );
        // And not by so much that the budget becomes theatre: a check almost
        // nothing satisfies is refused as often as it is honoured, and stops
        // being read.
        assert!(
            needed < measured * 2,
            "at {vertices} vertices the estimate ({needed} bytes) is more than twice the \
             {measured} measured, which would refuse corpora that fit comfortably"
        );
    }
}

/// The three terms are all monotone, which is what makes the check safe to run
/// on the sums across a multi-type corpus rather than per type.
#[test]
fn the_estimate_grows_with_every_term() {
    let base = estimated_peak_bytes(1_000, 10_000, 100_000);
    assert!(estimated_peak_bytes(2_000, 10_000, 100_000) > base);
    assert!(estimated_peak_bytes(1_000, 20_000, 100_000) > base);
    assert!(estimated_peak_bytes(1_000, 10_000, 200_000) > base);
    // Saturating rather than wrapping: an absurd corpus must estimate as
    // enormous, not as small.
    assert!(estimated_peak_bytes(u64::MAX, u64::MAX, u64::MAX) > base);
}
