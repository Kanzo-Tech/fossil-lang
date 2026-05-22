//! SC#2 — the HARD CI gate for the `didChange` round-trip budget.
//!
//! Measures the full per-keystroke analysis round-trip the LSP runs on every
//! `didChange` over the canonical 200-line fixture:
//!
//!   `set_text` (Salsa Setter, revision bump) → `def_map` → `typecheck_mapping`
//!   over every mapping → drain the `Diagnostic` accumulator
//!
//! This is the SAME work `fossil-lsp`'s `didChange` handler performs (minus the
//! JSON-RPC framing, which is negligible). It runs as a plain `#[test]` in
//! `cargo test`, so it is the CI hard gate.
//!
//! # Why a MARGINED budget, not a naked `< 100ms`
//!
//! Per ADR-0021 the design GOAL is `< 100ms` on dev hardware, but a naked
//! `< 100ms` assertion FLAKES on shared CI runners (cold caches, neighbour
//! noise, slow debug builds). The hard gate therefore asserts a GENEROUS
//! margin ([`BUDGET_MS`]) that still catches algorithmic blow-up (an O(n²)
//! regression on a 200-line file would blow well past it) without flaking. The
//! tight `< 100ms` goal + the 20%-regression check live in the ADVISORY
//! Criterion benchmark (`benches/lsp_didchange.rs`), reliable only on a pinned
//! runner. See ADR-0021.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use salsa::Setter as _;

/// The margined hard-gate budget. The design goal is < 100ms on dev hardware;
/// this 400ms ceiling absorbs CI noise + debug-build overhead while still
/// catching an algorithmic regression (which would be seconds, not ms) on a
/// 200-line file. Tightening this is the Criterion benchmark's job (advisory).
const BUDGET_MS: u128 = 400;

/// Warm-up + measured iterations. The first analysis primes Salsa's caches; we
/// then measure steady-state per-`didChange` cost (the realistic editor case is
/// an already-warm db being re-analysed after an edit).
const WARMUP: usize = 2;
const ITERS: usize = 10;

fn fixture() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/canonical_200.fossil"
    ))
    .expect("read canonical_200.fossil")
}

/// One `didChange` round-trip: bump the revision via `set_text` (the real
/// cancellation trigger), then re-run the analysis pipeline + drain the
/// accumulator across every mapping. Returns the diagnostic count (kept so the
/// optimiser cannot elide the work).
fn round_trip(db: &mut FossilDb, file: SourceFile, new_text: String) -> usize {
    file.set_text(db).to(new_text);
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mappings = def_map.mappings(db).clone();
    let mut count = 0;
    for mapping in &mappings {
        let _ = fossil_hir::check::typecheck_mapping(db, *mapping);
        let diags = fossil_hir::check::typecheck_mapping::accumulated::<Diagnostic>(db, *mapping);
        count += diags.len();
    }
    count
}

#[test]
fn didchange_round_trip_under_margined_budget() {
    let base = fixture();
    let system: Arc<dyn System> = Arc::new(NativeSystem);
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(&db, base.clone(), "canonical_200.fossil".to_string());

    // Each "keystroke" appends a comment char to a comment line — a VALID-syntax
    // edit (never a bare `#`, which would hang the parser per 06-07). This
    // changes the text (so `set_text` genuinely bumps the revision) without
    // introducing a parse error or an unlexable byte.
    let edit = |i: usize| format!("{base}\n// keystroke {i}\n");

    // Warm-up: prime Salsa's caches.
    for i in 0..WARMUP {
        let _ = round_trip(&mut db, file, edit(i));
    }

    // Measured: steady-state per-didChange cost.
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    for i in 0..ITERS {
        let text = edit(WARMUP + i);
        let start = Instant::now();
        let _ = round_trip(&mut db, file, text);
        let elapsed = start.elapsed();
        worst = worst.max(elapsed);
        total += elapsed;
    }

    let avg_ms = total.as_millis() / ITERS as u128;
    let worst_ms = worst.as_millis();
    eprintln!(
        "didChange round-trip over canonical_200.fossil: avg={avg_ms}ms worst={worst_ms}ms \
         (design goal <100ms; hard-gate budget <{BUDGET_MS}ms)"
    );

    assert!(
        worst_ms < BUDGET_MS,
        "didChange round-trip blew the margined budget: worst={worst_ms}ms >= {BUDGET_MS}ms \
         (avg={avg_ms}ms). This margin tolerates CI noise but catches algorithmic blow-up — \
         a real regression here means the analysis pipeline got asymptotically slower."
    );
}
