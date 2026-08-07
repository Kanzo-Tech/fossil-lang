//! What would removing salsa from the batch path actually buy, in milliseconds?
//!
//! `query_executions.rs` counts executions and says plainly that it cannot
//! answer this: it knows there is nothing to *reuse* per mapping, not what the
//! bookkeeping *costs*. ADR-0046 §6 fills that hole with Apollo's −52.3%, and
//! marks it as borrowed. This example takes the number back.
//!
//! # The three quantities
//!
//! 1. **The batch compile.** `def_map` + `lower_to_mir_pg` per mapping, the
//!    exact shape `fossil_engine::check` runs, timed at four sizes.
//! 2. **Salsa's per-execution cost.** A tracked function over an interned key
//!    that computes nothing, on cold keys — so what is timed is intern + memo
//!    insert + dependency edge, and nothing else. Multiplied by the execution
//!    count from `query_executions`, it is an **upper bound** on what leaving
//!    salsa can return: the compiler's own work does not change, and a
//!    salsa-free core still has to intern types and hold results somewhere.
//! 3. **What is not the compiler.** `fossil check` also starts a process and
//!    opens `DuckDB` to pre-introspect every source. Salsa cannot be a large
//!    share of a command those two dominate, and the shell measures it:
//!    `fossil refs` runs the same parse with neither `DuckDB` nor a typecheck.
//!
//! ```text
//! cargo run --release -p fossil-mir --example query_time
//! ```
//!
//! Release only. A debug build measures the absence of inlining.

use std::fmt::Write as _;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fossil_base::{Db, FossilDb, NativeSystem, SourceFile, System};

/// A program with `n` mappings over one source, in the shape `hello.fossil`
/// has — the same generator `query_executions` uses, so the two examples are
/// counting and timing the same programs.
// The `${ex:}` / `${.id}` in the template are fossil's own interpolation, which
// clippy reads as a Rust format argument that escaped its macro. It did not.
#[allow(clippy::literal_string_with_formatting_args)]
fn program(n: usize) -> String {
    let mut s = String::from(
        "prefix ex: <https://example.org/>\n\nusers := io.csv(\"examples/users.csv\")\n\n",
    );
    for i in 0..n {
        let _ = write!(
            s,
            "M{i} : ex:Person from users\n    iri = `${{ex:}}m{i}/${{.id}}`\n    ex:name = .name\n\n"
        );
    }
    s
}

/// The batch path, verbatim in shape, on a database built for this call and
/// thrown away after it — which is what the CLI does on every invocation.
/// Returns (database construction, compile).
fn batch(n: usize) -> (Duration, Duration) {
    let text = program(n);

    let t0 = Instant::now();
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, text, "bench.fossil".to_string());
    let open = t0.elapsed();

    let t1 = Instant::now();
    let dm = fossil_hir::def_map::def_map(&db, file);
    let mappings = dm.mappings(&db).clone();
    for m in &mappings {
        let _ = black_box(fossil_mir::lower_to_mir_pg(&db, *m));
        let _ = black_box(fossil_mir::lower_to_mir_pg::accumulated::<
            fossil_base::Diagnostic,
        >(&db, *m));
    }
    let compile = t1.elapsed();

    assert_eq!(mappings.len(), n, "the generator and the def map disagree");
    (open, compile)
}

// ------------------------------------------------------------------ the control

/// A cold key for the control query. Interned, because every real query in the
/// batch path is keyed by an interned or tracked struct, and the intern is part
/// of what a tracked call costs.
#[salsa::interned(debug)]
struct ControlKey<'db> {
    index: usize,
}

/// Computes nothing. What its call costs is salsa's bookkeeping, entire.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)]
fn control<'db>(db: &'db dyn Db, key: ControlKey<'db>) -> usize {
    key.index(db)
}

/// Time `n` executions of the control on `n` distinct (cold) keys.
fn control_cost(n: usize) -> Duration {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let t = Instant::now();
    for i in 0..n {
        let key = ControlKey::new(&db, i);
        let _ = black_box(control(&db, key));
    }
    t.elapsed()
}

// ------------------------------------------------------------------- the phases

/// Where the batch compile's time goes, query by query. Each phase is timed
/// after the ones above it have run, so salsa has already memoised them and
/// what is timed is that phase's own cold work — the decomposition only reads
/// this way in the order written.
fn phases(n: usize) {
    use fossil_hir::body::{body, mapping_cst_node};

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, program(n), "bench.fossil".to_string());

    let t = Instant::now();
    let _ = black_box(fossil_syntax::parse(&db, file));
    let parse = t.elapsed();

    let t = Instant::now();
    let dm = fossil_hir::def_map::def_map(&db, file);
    let def_map = t.elapsed();
    let mappings = dm.mappings(&db).clone();

    let t = Instant::now();
    for m in &mappings {
        let _ = black_box(mapping_cst_node(&db, *m));
    }
    let cst_node = t.elapsed();

    let t = Instant::now();
    for m in &mappings {
        let _ = black_box(body(&db, *m));
    }
    let bodies = t.elapsed();

    let t = Instant::now();
    for m in &mappings {
        let _ = black_box(fossil_hir::check::typecheck_mapping(&db, *m));
    }
    let typeck = t.elapsed();

    let t = Instant::now();
    for m in &mappings {
        let _ = black_box(fossil_mir::lower_to_mir_pg(&db, *m));
        let _ = black_box(fossil_mir::lower_to_mir_pg::accumulated::<
            fossil_base::Diagnostic,
        >(&db, *m));
    }
    let lower = t.elapsed();

    let total = parse + def_map + cst_node + bodies + typeck + lower;
    println!(
        "\nwhere the compile goes at {n} mappings — each phase cold, the ones above it warm\n"
    );
    for (name, d) in [
        ("parse", parse),
        ("def_map", def_map),
        ("mapping_cst_node", cst_node),
        ("body", bodies),
        ("typecheck_mapping", typeck),
        ("lower_to_mir_pg", lower),
    ] {
        let share = d.as_secs_f64() / total.as_secs_f64() * 100.0;
        println!("  {name:>18}  {d:>11.3?}  {share:>5.1}%");
    }
}

// ------------------------------------------------------------------------- main

/// Executions per invocation at each size, from `query_executions` — three
/// file-pinned queries plus five per mapping. Recomputed here rather than
/// imported because that example is a binary, not a library; the two are kept
/// honest by both deriving from the same `program()`.
const fn executions(n: usize) -> usize {
    3 + 5 * n
}

fn median(mut xs: Vec<Duration>) -> Duration {
    xs.sort_unstable();
    xs[xs.len() / 2]
}

fn main() {
    // One untimed pass: the first call in the process pays for lazily
    // initialised statics (the stdlib catalog, the source-kind table) that no
    // later call pays again, and charging them to n = 1 would read as salsa.
    let _ = batch(1);

    println!("batch compile — median of 9, a fresh database per run\n");
    println!(
        "{:>8}  {:>12}  {:>12}  {:>12}  {:>10}  {:>8}",
        "mappings", "open db", "compile", "total", "execs", "per exec"
    );

    let sizes = [1usize, 10, 100, 1000];
    let mut totals = Vec::new();
    for n in sizes {
        let mut opens = Vec::new();
        let mut compiles = Vec::new();
        for _ in 0..9 {
            let (open, compile) = batch(n);
            opens.push(open);
            compiles.push(compile);
        }
        let open = median(opens);
        let compile = median(compiles);
        let total = open + compile;
        let per = total / u32::try_from(executions(n)).expect("execution count fits u32");
        println!(
            "{n:>8}  {:>11.3?}  {:>11.3?}  {:>11.3?}  {:>10}  {:>7.3?}",
            open,
            compile,
            total,
            executions(n),
            per
        );
        totals.push((n, total));
    }

    // The control, at the same execution counts the batch path produces.
    println!("\nsalsa's own cost — a tracked query that computes nothing, on cold keys\n");
    let _ = control_cost(1000); // warm the allocator, not the memo table
    println!("{:>10}  {:>12}  {:>10}", "executions", "total", "per exec");
    let mut per_exec = Duration::ZERO;
    for n in [100usize, 1_000, 10_000] {
        let d = median((0..9).map(|_| control_cost(n)).collect());
        let per = d / u32::try_from(n).expect("n fits u32");
        println!("{n:>10}  {d:>11.3?}  {per:>9.3?}");
        per_exec = per;
    }

    println!("\nthe bound: salsa's share of the batch compile, at the largest control rate\n");
    for (n, total) in &totals {
        let overhead = per_exec * u32::try_from(executions(*n)).expect("execution count fits u32");
        #[allow(clippy::cast_precision_loss)]
        let share = overhead.as_secs_f64() / total.as_secs_f64() * 100.0;
        println!("{n:>8} mappings  {overhead:>11.3?}  of {total:>11.3?}  = {share:>5.1}%");
    }

    // Two sizes and not one: a phase that grows 10× with the program is linear
    // in it, and a phase that grows 100× is not — which is the only way to read
    // a share table without mistaking a scaling defect for a fixed cost.
    phases(100);
    phases(1000);

    println!(
        "\nRead the ceiling table as a CEILING and not a forecast. It is what salsa's\n\
         bookkeeping costs if the batch path's queries are as cheap to book as one\n\
         that computes nothing — and it is charged in full, though a salsa-free core\n\
         still has to intern its types and keep its results somewhere. The floor of\n\
         the win is lower than this; it is not higher.\n\n\
         And this whole table is a share of the COMPILE, not of the command. What\n\
         `fossil check` spends outside it — process start, and a DuckDB connection\n\
         per run to pre-introspect every source — the shell measures:\n\
         `fossil refs` is the same parse with neither."
    );
}
