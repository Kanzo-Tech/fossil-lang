//! The HARD CI gate for the `didChange` round-trip budget.
//!
//! Measures the full per-keystroke analysis round-trip the LSP runs on every
//! `didChange` over the canonical 200-line fixture:
//!
//!   `set_text` (Salsa Setter, revision bump) → [`fossil_mir::program_diagnostics`]
//!
//! That is the SAME work `fossil-lsp`'s `didChange` handler performs, and now it
//! is the same *call* — the handler's body is one line and this is that line.
//! The claim used to be made about a hand-rolled loop that did strictly less;
//! see [`round_trip`]. Minus the JSON-RPC framing, which is negligible. It runs
//! as a plain `#[test]` in `cargo test`, so it is the CI hard gate.
//!
//! # Why a MARGINED budget, not a naked `< 100ms`
//!
//! The design GOAL is `< 100ms` on dev hardware, but a naked
//! `< 100ms` assertion FLAKES on shared CI runners (cold caches, neighbour
//! noise, slow debug builds). The hard gate therefore asserts a GENEROUS
//! margin ([`BUDGET_MS`]) that still catches algorithmic blow-up (an O(n²)
//! regression on a 200-line file would blow well past it) without flaking. The
//! tight `< 100ms` goal + the 20%-regression check live in the ADVISORY
//! Criterion benchmark (`benches/lsp_didchange.rs`), reliable only on a pinned
//! runner.
//!
//! # The host has to be the EDITOR's, and it was not
//!
//! This built its db on `fossil_base::test_support::NativeSystem`, whose provider table is the
//! trait default: **no row reads types**. Since ruling 3 of 2026-08-11 a program
//! must name a shape document, and the fixture does; under `NativeSystem` that
//! document decodes to nothing, `resolve_target_shape` fails per mapping, and
//! the checker leaves by the shortest path it has. The number that came out was
//! real and measured the wrong program — every property's constraint lookup, the
//! backward check and the short-name resolution were all skipped, and those are
//! the work a keystroke actually costs now.
//!
//! [`EditorSystem`] installs the same rows `fossil-lsp`'s own `LspSystem` does,
//! and [`fixture`] opens the program under its REAL path so the document beside
//! it resolves. What that buys is stated as an assertion rather than a comment:
//! [`RESOLVED_PREDICATES`] is checked before the clock starts, so a future edit
//! that quietly stops resolving the contract fails here instead of producing a
//! flattering millisecond count.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use fossil_base::{FossilDb, FsError, Provider, SourceFile, System};
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

/// What the fixture's 15 mappings resolve between them — 60 properties, plus
/// the 15 `@subject` lines, which are not predicates.
///
/// This is the guard on the paragraph above: it is the count that goes to zero
/// the moment the host stops reading types or the document stops resolving, and
/// it is checked before anything is timed.
const RESOLVED_PREDICATES: usize = 60;

/// The path the program is opened under. It has to be the REAL one: the shape
/// document is resolved relative to the program, so a synthetic path resolves
/// nothing and silently measures a program with no output contract.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/canonical_200.fossil")
}

fn fixture() -> String {
    std::fs::read_to_string(fixture_path()).expect("read canonical_200.fossil")
}

/// The editor's `System`: a filesystem plus the rows that read shape documents,
/// exactly as `fossil-lsp`'s `LspSystem` installs them.
#[derive(Debug, Default)]
struct EditorSystem;

impl System for EditorSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| FsError::Io(e.to_string()))
    }
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// One `didChange` round-trip: bump the revision via `set_text` (the real
/// cancellation trigger), then run exactly what the handler runs. Returns the
/// diagnostic count (kept so the optimiser cannot elide the work).
///
/// # This measured a cheaper program than the handler ran, and the docblock
/// said otherwise
///
/// It was a hand-rolled loop over `typecheck_mapping`, and the header above
/// called it «the SAME work `fossil-lsp`'s `didChange` handler performs». It
/// was not, in two directions at once. The handler drains **`lower_to_mir_pg`**
/// — deliberately, because draining the typechecker alone shows the editor a
/// clean file that `run` refuses — so every lowering the keystroke pays for was
/// outside the clock. And the handler runs three FILE-level drains the loop had
/// no equivalent of.
///
/// A budget is only a gate on the thing it executes. Calling
/// [`fossil_mir::program_diagnostics`] is what makes the number a statement
/// about `didChange` rather than about a loop that resembles it; there is now
/// one function, so the two cannot drift again.
fn round_trip(db: &mut FossilDb, file: SourceFile, new_text: String) -> usize {
    file.set_text(db).to(new_text);
    fossil_mir::program_diagnostics(db, file).len()
}

#[test]
fn didchange_round_trip_under_margined_budget() {
    let base = fixture();
    let system: Arc<dyn System> = Arc::new(EditorSystem);
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(
        &db,
        base.clone(),
        fixture_path().to_string_lossy().into_owned(),
    );
    // The host's other half: the document the program names is a Salsa INPUT, so
    // it has to be registered before any query looks for it. `fossil-lsp` does
    // this on `didOpen`; skipping it here is what made the old number cheap.
    fossil_ide::register_missing_documents(&mut db, file, &|key| std::fs::read_to_string(key).ok());

    // The contract actually resolved — assert it BEFORE timing, so a budget that
    // stops measuring the checking path fails loudly instead of getting faster.
    let mapping_count = fossil_hir::def_map::def_map(&db, file).mappings(&db).len();
    let resolved: usize = fossil_hir::def_map::def_map(&db, file)
        .mappings(&db)
        .iter()
        .filter_map(|m| fossil_hir::check::typecheck_mapping(&db, *m).ok())
        .map(|out| out.predicates(&db).len())
        .sum();
    assert_eq!(
        resolved, RESOLVED_PREDICATES,
        "the fixture must resolve its output contract or this budget measures a \
         program that checks nothing; {mapping_count} mappings resolved {resolved} \
         predicates"
    );

    // Each "keystroke" appends a comment char to a comment line — a VALID-syntax
    // edit. This changes the text (so `set_text` genuinely bumps the revision)
    // without introducing a parse error or an unlexable byte, which is what
    // keeps the budget measuring the resolving path rather than the error one.
    // (A bare `#` used to hang the parser. It does not any more — see
    // `fossil-syntax/tests/recovery.rs` — but an unlexable byte still measures
    // the wrong path.)
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
