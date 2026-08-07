//! How much does salsa save inside **one** batch invocation?
//!
//! `decisions/0046-un-nucleo-y-carcasas-finas.md` §6 keeps salsa but says its
//! scope is wrong, and it refuses to settle that on an argument. The open
//! question is narrow: the CLI builds a `FossilDb` with empty storage on every
//! call (`crates/fossil-engine/src/system.rs:60`), so there is no incrementality
//! across invocations by construction — the only thing salsa can be buying the
//! batch path is **memoization within a single run**, which a `HashMap` also
//! buys, at none of salsa's cost.
//!
//! Apollo removed salsa from `apollo-compiler` in 2024 and measured 1.2×–2×:
//! −19.4% on the large input and **−52.3% on the small one**. Per-query
//! bookkeeping is a fixed cost, so it dominates exactly when the work per query
//! is small and the process is short — which is fossil's stated profile, small
//! programs over large sources.
//!
//! # What this measures
//!
//! It reproduces the shape of `fossil_engine::check` — `def_map`, then
//! `lower_to_mir_pg` per mapping — against a database with an event callback,
//! and counts `EventKind::WillExecute` **per query**. An execution is a cache
//! miss; a call that does not appear is a hit.
//!
//! Run it against programs of several sizes: if executions grow one-for-one with
//! mappings and nothing repeats, salsa is memoizing nothing that a single pass
//! would not already avoid.
//!
//! # What it does NOT measure
//!
//! **Not time.** It counts executions, not milliseconds, so it cannot say what
//! removing salsa would win — only whether there is anything to win. That half
//! is `query_time.rs`, and it has since been measured: the ceiling is 3.8% of
//! the batch compile, and the batch compile is 0.20% of `fossil check`. Apollo's
//! −52.3% was never ours and is no longer cited as if it were (ADR-0050).
//!
//! **Not the editor path.** The LSP keeps one database alive across edits, and
//! `crates/fossil-hir/tests/invalidation_regression.rs` already pins what that
//! saves. This example is about the path that throws the database away.
//!
//! ```text
//! cargo run -p fossil-mir --example query_executions
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};

/// A program with `n` mappings over one source, in the shape `hello.fossil` has.
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

fn measure(n: usize) -> (usize, BTreeMap<String, usize>) {
    let tally: Arc<Mutex<BTreeMap<String, usize>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let sink = Arc::clone(&tally);

    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            // The debug form is `query_name(Key)`; keep the name, drop the key,
            // because what is being counted is executions per query, not per key.
            let full = format!("{database_key:?}");
            let name = full.split('(').next().unwrap_or(&full).to_string();
            *sink.lock().expect("tally").entry(name).or_insert(0) += 1;
        }
    });

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::with_event_callback(system, callback);
    let file = SourceFile::new(&db, program(n), "bench.fossil".to_string());

    // The batch path, verbatim in shape: one `def_map`, then one
    // `lower_to_mir_pg` per mapping plus its accumulator drain.
    let dm = fossil_hir::def_map::def_map(&db, file);
    let mappings = dm.mappings(&db).clone();
    for m in &mappings {
        let _ = fossil_mir::lower_to_mir_pg(&db, *m);
        let _ = fossil_mir::lower_to_mir_pg::accumulated::<fossil_base::Diagnostic>(&db, *m);
    }

    let counts = tally.lock().expect("tally").clone();
    (counts.values().sum(), counts)
}

fn main() {
    println!("executions per one batch invocation — a miss is an execution, a hit is silent\n");
    println!(
        "{:>8}  {:>12}  {:>14}",
        "mappings", "executions", "per mapping"
    );

    let sizes = [1usize, 10, 100];
    let mut last = BTreeMap::new();
    for n in sizes {
        let (total, counts) = measure(n);
        #[allow(clippy::cast_precision_loss)]
        let per = total as f64 / n as f64;
        println!("{n:>8}  {total:>12}  {per:>14.2}");
        last = counts;
    }

    println!("\nby query, at {} mappings:", sizes[sizes.len() - 1]);
    for (name, count) in &last {
        #[allow(clippy::cast_precision_loss)]
        let per = *count as f64 / sizes[sizes.len() - 1] as f64;
        println!("  {count:>7}  {per:>7.2}/mapping  {name}");
    }

    println!(
        "\nRead it this way: a query whose count is ~1 regardless of size ran once and was reused —\n\
         that is the memoization a HashMap also gives. A query at ~1 per mapping ran once per\n\
         mapping, so nothing was reused and salsa's bookkeeping bought nothing on that one."
    );
}
