//! The ADVISORY Criterion benchmark for the `didChange` round-trip budget.
//!
//! Measures the SAME per-keystroke analysis round-trip as the hard-gate
//! correctness test (`tests/didchange_budget.rs`): `set_text` (Salsa Setter,
//! revision bump) → [`fossil_mir::program_diagnostics`], on the canonical
//! 200-line fixture. It is the same *call*, not a resemblance — see
//! [`round_trip`], which described a loop the budget test had already retired.
//!
//! # Status: ADVISORY
//!
//! Criterion's committed baseline + 20%-regression detection is reliable only
//! on a pinned / self-hosted runner (stable CPU, no neighbour noise). On shared
//! CI it is INFORMATIONAL — run it, record the numbers, but do not BLOCK on the
//! regression check (that would flake). The blocking gate is the margined
//! wall-clock budget test. The design GOAL this benchmark tracks is `< 100ms`
//! on dev hardware; the benchmark reports the true steady-state cost so a real
//! perf regression is visible on a controlled runner.
//!
//! Run: `cargo bench -p fossil-lsp`. The first run writes the baseline under
//! `target/criterion/`; subsequent runs compare against it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use criterion::{Criterion, criterion_group, criterion_main};
use fossil_base::{FossilDb, FsError, Provider, SourceFile, System};
use salsa::Setter as _;

/// The path the program is opened under — the REAL one, because the shape
/// document is resolved relative to the program. See the hard gate's module
/// docs: a synthetic path resolves no contract, and the benchmark then measures
/// a program that checks nothing.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/canonical_200.fossil")
}

fn fixture() -> String {
    std::fs::read_to_string(fixture_path()).expect("read canonical_200.fossil")
}

/// The editor's `System` — the rows that read shape documents, as
/// `fossil-lsp`'s own `LspSystem` installs them. It was
/// `fossil_base::test_support::NativeSystem`, which reads no types; see the hard gate.
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

/// One `didChange` round-trip: bump the revision via `set_text`, then run
/// exactly what the handler runs. Returns the diagnostic count so the optimiser
/// cannot elide the work.
///
/// # This said it was the budget test's function and had not been for a while
///
/// The line above read «identical to the budget test's, kept in sync». It was
/// not. The budget test collapsed onto [`fossil_mir::program_diagnostics`] —
/// the one function `fossil-lsp`'s `didChange` handler calls — and its own
/// docblock closes with «there is now one function, so the two cannot drift
/// again». This bench stayed on the hand-rolled `def_map` +
/// `typecheck_mapping` loop that collapse replaced, so the advisory number and
/// the hard gate measured different programs: the loop never drained
/// `lower_to_mir_pg`, and had no equivalent of the three FILE-level drains.
/// Every lowering a keystroke pays for was outside this clock.
///
/// «Kept in sync» is not a mechanism, and this is the third place in these two
/// crates where a comment claimed an agreement nothing checked. Calling the
/// same function is the mechanism.
fn round_trip(db: &mut FossilDb, file: SourceFile, new_text: String) -> usize {
    file.set_text(db).to(new_text);
    fossil_mir::program_diagnostics(db, file).len()
}

fn bench_didchange(c: &mut Criterion) {
    let base = fixture();

    let mut group = c.benchmark_group("lsp_didchange");
    // A handful of samples is enough for a 200-line file; keep wall time low.
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("canonical_200_round_trip", |b| {
        // Build a warm db once; each iteration is a distinct edit so `set_text`
        // genuinely bumps the revision (the realistic editor case).
        let system: Arc<dyn System> = Arc::new(EditorSystem);
        let mut db = FossilDb::new(system);
        let file = SourceFile::new(
            &db,
            base.clone(),
            fixture_path().to_string_lossy().into_owned(),
        );
        // The document the program names is a Salsa INPUT: register it, or the
        // contract never resolves and this measures the error path.
        fossil_ide::register_missing_documents(&mut db, file, &|key| {
            std::fs::read_to_string(key).ok()
        });
        let _ = round_trip(&mut db, file, format!("{base}\n// warm\n"));
        let mut i = 0usize;
        b.iter(|| {
            i += 1;
            // VALID-syntax edit (a `//` comment line — never a bare `#`).
            std::hint::black_box(round_trip(&mut db, file, format!("{base}\n// edit {i}\n")))
        });
    });

    group.finish();
}

criterion_group!(benches, bench_didchange);
criterion_main!(benches);
