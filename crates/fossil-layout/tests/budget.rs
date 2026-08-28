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
/// The measurements, all `FOSSIL_MEM_PROBE=1 cargo run --release -p
/// fossil-layout --example enrich_memory -- <N> <degree>` on a Mac16,8 — 14
/// cores, 48 GiB, macOS 26.2 / Darwin 25.2.0 — on 2026-08-28:
///
/// | N | degree | vertex Parquet | process peak | at `start` | **the pass** |
/// | --- | --- | --- | --- | --- | --- |
/// | 2,000,000 | 14 | 235.87 MB | 1.02 GiB | 0.42 GiB | 0.60 GiB |
/// | 4,000,000 | 6 | 473.84 MB | 1.43 GiB | 0.36 GiB | 1.07 GiB |
/// | 4,000,000 | 14 | 473.84 MB | 1.95 GiB | 0.72 GiB | 1.23 GiB |
/// | 4,000,000 | 28 | 473.84 MB | 2.18 GiB | 1.01 GiB | 1.17 GiB |
/// | 10,000,000 | 14 | 1,187.75 MB | 3.94 GiB | 1.23 GiB | 2.71 GiB |
///
/// **The three middle rows are the ones that were owed.** Every point behind the
/// original calibration shared mean degree fourteen, where V and E are
/// proportional and a per-row term and a per-vertex term fit the same line;
/// these three hold the vertex file identical and move only the edges.
///
/// The ten-million row is the **larger** of two runs of the same build — 3.94
/// and 3.61 GiB — because a bound fitted to the luckier of two runs is a bound
/// that fails on the unluckier.
///
/// **The fourth column is no longer what `ADJACENCY_ROW_BYTES` is fitted on, and
/// this table is why.** At four million vertices the pass goes 1.07 → 1.23 →
/// **1.17** GiB as the degree goes 6 → 14 → 28: it falls at the densest point,
/// and a slope through the three is 1.2 B/row, below what `read CSR + CSC` is
/// measured to hold in the same runs. The fifth column is the confound — this
/// example builds its fixture in the process that then measures the pass, so the
/// baseline already holds the generator's retained heap (0.36, 0.72, 1.01 GiB
/// across those three) and the pass reuses those pages rather than asking for
/// more. `peak − start` is biased low and increasingly so with degree. The
/// constant is fitted on a per-phase delta instead; see `ADJACENCY_ROW_BYTES`.
/// These rows remain the right thing to assert against, because a bound that
/// clears a biased-low measurement clears the true one too.
///
/// **What is bounded is the pass, not the process**, which is why the fourth
/// column is the one asserted against. `enrich_memory` builds its fixture in the
/// same process and the generator's pages are still resident when the pass
/// starts; in a real `fossil run` that resident set belongs to `execute_graph`,
/// which carries a `FairSpillPool` and a budget of its own.
///
/// **That composition is decided rather than open**: `--memory-gib` bounds each
/// stage of the write path, not their sum, which is why this asserts the pass
/// against the pass's own footprint and not against the process's. Splitting the
/// number and sequencing it are both refused on `/docs/design/streaming`, with
/// the measurements that refuse them.
#[test]
fn the_estimate_over_estimates_the_runs_it_is_calibrated_on() {
    // (vertices, adjacency rows over both orientations, vertex Parquet bytes,
    //  what the pass itself added)
    const MEASURED: [(u64, u64, u64, u64); 5] = [
        (2_000_000, 27_974_508, 235_870_000, 644_245_094), // 0.60 GiB
        (4_000_000, 23_978_362, 473_840_000, 1_148_903_751), // 1.07 GiB, degree 6
        (4_000_000, 55_949_862, 473_840_000, 1_320_702_443), // 1.23 GiB, degree 14
        (4_000_000, 111_899_830, 473_840_000, 1_256_277_606), // 1.17 GiB, degree 28
        (10_000_000, 139_874_560, 1_187_750_000, 2_909_844_700), // 2.71 GiB
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
    // And the floor is a term and not a rounding: a corpus of nothing still
    // opens a Parquet reader and a writer, and at the small end that is the
    // whole of the answer rather than a correction to it.
    assert!(
        estimated_peak_bytes(0, 0, 0) >= 8 * 1024 * 1024,
        "an empty corpus estimates as free, which is the shape that let a \
         sixty-thousand-vertex run be admitted on a budget it then exceeded"
    );
}
