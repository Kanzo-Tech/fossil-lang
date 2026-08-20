//! The ADVISORY Criterion benchmark for the `didChange` round-trip budget.
//!
//! Measures the SAME per-keystroke analysis round-trip as the hard-gate
//! correctness test (`tests/didchange_budget.rs`): `set_text` (Salsa Setter,
//! revision bump) → `def_map` → `typecheck_mapping` over every mapping → drain
//! the `Diagnostic` accumulator, on the canonical 200-line fixture.
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
use fossil_base::{Diagnostic, FossilDb, FsError, Provider, SourceFile, System};
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
/// `fossil_base::NativeSystem`, which reads no types; see the hard gate.
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

/// One `didChange` round-trip — identical to the budget test's, kept in sync so
/// the advisory benchmark and the hard gate measure the same work.
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
