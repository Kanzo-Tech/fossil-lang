// The embedded `.fossil` fixture interpolates (`"…/{users.id}"`), which clippy
// mistakes for format args in a plain string literal — it is not.
#![allow(clippy::literal_string_with_formatting_args)]

//! A declared budget is honoured by spilling, not by fitting.
//!
//! A budget the operators cannot honour looks exactly like a budget nobody
//! tested: both end in a run that finished. So this test asserts the mechanism —
//! under the budget the executor wrote spill files, without it the same corpus
//! never touched disk, and the two runs wrote byte-identical trees.
//!
//! **What it cannot prove.** That ten million vertices stay inside a 4 GiB
//! budget: peak RSS is not what is measured here, and the Arrow the writer holds
//! is outside the pool either way. That a plan free to pick a hash join would
//! survive: it would not, which is why `bounded_context` clears
//! `prefer_hash_join`, and this test only ever sees the plan that leaves. And
//! that any *given* budget is viable — the floor is a property of the machine,
//! not of the corpus (see `budget` below).

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

mod support;

/// 5k people, 400k orders: the orders are what make the corpus too big for the
/// budget (each is an `Order` vertex *and* an edge), while the people stay small
/// enough that generating and parsing the corpus is a second, not a minute.
const PEOPLE: u32 = 5_000;
const ORDERS: u32 = 400_000;

/// `{dir}` is the ONLY hole `run` substitutes: `{users.id}` and its siblings are
/// Fossil interpolation and are left alone by a literal `replace("{dir}", …)`.
const PROGRAM: &str = "\
type { Person, Order } := io.shex(\"spill.shex\")

users := io.csv(\"{dir}/users.csv\")
orders := io.csv(\"{dir}/orders.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Order : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = \"https://example.org/person/{orders.user_id}\"
";

/// The document the program names, and the descriptor the executor is given.
/// `ex:placedBy @ex:Person` is what makes `placedBy` an edge — under
/// `ACCEPT_ALL_DEFAULT` it degrades to a string column, the edge table
/// disappears, and this test's premise ("one edge table to join and sort
/// twice") goes with it silently.
const SPILL_SHEX: &str = include_str!("fixtures/spill.shex");

/// The budget, and why it is per-core rather than a round number.
///
/// A budget below the floor does not spill, it dies — and the floor is set by
/// the machine, not the corpus: each sort partition reserves
/// `sort_spill_reservation_bytes` (10 MB, and it declares `can spill: false`),
/// so the smallest viable pool grows with `target_partitions`. Measured on a
/// ten-core machine: 384 MiB dies with five `ExternalSorterMerge` holding 10 MB
/// each, 480 MiB runs and spills, and it still spills at 2 GiB. 64 MiB per core
/// sits inside that window and moves with the machine the test runs on.
///
/// Ridiculous is therefore relative: this is a fraction of what the same corpus
/// takes unbounded, which is the sense that matters.
fn budget() -> u64 {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    64 * 1024 * 1024 * cores as u64
}

#[test]
fn a_ridiculous_budget_spills_and_writes_the_same_corpus() {
    let spills = install_spill_recorder();
    let corpus = tempfile::tempdir().expect("tempdir");
    write_corpus(corpus.path());

    let program = PROGRAM.replace("{dir}", &corpus.path().to_string_lossy());
    let unbounded_dir = tempfile::tempdir().expect("tempdir");
    let bounded_dir = tempfile::tempdir().expect("tempdir");

    // Control: the same corpus with no budget. The default pool is unlimited and
    // has no disk manager, so this one cannot spill — without it, a spill below
    // would not be evidence that the budget caused it.
    run(&program, unbounded_dir.path(), None);
    let unbounded_spills = spills.take();
    assert!(
        unbounded_spills.is_empty(),
        "the unbounded run spilled, so a spill no longer means the budget did it: {unbounded_spills:?}",
    );

    run(&program, bounded_dir.path(), Some(budget()));
    let bounded_spills = spills.take();
    assert!(
        !bounded_spills.is_empty(),
        "the run finished under a {}-byte budget without spilling once — either the corpus now \
         fits (raise ORDERS) or the operators are not honouring the pool, which is the failure \
         this test exists to catch",
        budget(),
    );

    // Spilling changes how the executor gets there, not what it produces.
    assert_eq!(
        tree(unbounded_dir.path()),
        tree(bounded_dir.path()),
        "the spilled run wrote a different corpus",
    );
}

/// One `run_to_dir` over `program` — local CSV sources, no RDF seam.
fn run(program: &str, dest: &Path, memory_bytes: Option<u64>) {
    let (db, file) =
        support::db_with_shapes(program, "spill.fossil", &[("spill.shex", SPILL_SHEX)]);
    let descriptor = fossil_df::OutputDescriptorKind::ShEx(
        fossil_shex::ShExDescriptor::from_shex_source(SPILL_SHEX).expect("parse spill.shex"),
    );
    fossil_df::run_to_dir(
        &db,
        file,
        &descriptor,
        dest,
        &std::collections::HashMap::new(),
        |uri| Err(format!("no RDF source expected: {uri}")),
        memory_bytes,
        // No policy: this test is about the memory budget, and a run with no
        // policy seals `privacy: undeclared` rather than refusing.
        None,
    )
    .unwrap_or_else(|e| panic!("run_to_dir: {e}; {:#?}", support::diagnostics(&db, file)));
}

/// `PEOPLE` people and `ORDERS` orders, each order pointing at a person — two
/// vertex types to dedup and sort, one edge table to join and sort twice.
fn write_corpus(dir: &Path) {
    // The ids are walked by a coprime stride so neither file arrives sorted:
    // an already-ordered input is the one case a sort never has to spill for.
    let mut users = String::from("id,name\n");
    for i in 0..PEOPLE {
        let key = (u64::from(i) * 7919) % u64::from(PEOPLE);
        users.push_str(&format!("{key},person-{key}\n"));
    }
    let mut orders = String::from("order_id,user_id\n");
    for i in 0..ORDERS {
        let key = (u64::from(i) * 7919) % u64::from(PEOPLE);
        orders.push_str(&format!("{i},{key}\n"));
    }
    std::fs::write(dir.join("users.csv"), users).expect("write users.csv");
    std::fs::write(dir.join("orders.csv"), orders).expect("write orders.csv");
}

/// Every file under `root`, by dataset-relative path → bytes.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).expect("read output file"));
            }
        }
    }
    assert!(
        !out.is_empty(),
        "no output written under {}",
        root.display()
    );
    out
}

/// What a spilling operator says, and nothing a merely bounded one does: the
/// first marker is the `DiskManager` opening the directory it writes spill files
/// into (the request description names the operator — `HashAggSpill`), the
/// second is `ExternalSorter` announcing the same. Deliberately not a bare
/// "spill": `Created new FairSpillPool(…)` carries the word and means only that
/// a budget was declared, which is the very thing under test.
const SPILL_MARKERS: [&str; 2] = ["as DataFusion tempfile directory for", "Spilling"];

/// Spill evidence, taken from DataFusion itself.
///
/// A spill leaves nothing behind to assert on: `DiskManager::used_disk_space` is
/// back to zero once the temp files drop, the files themselves are deleted with
/// them, and the plan's metrics never leave `execute_graph`. What does survive
/// is DataFusion's own `log` record, so the test installs a logger and keeps the
/// records that name a spill. This couples to another crate's log wording — a
/// rewording fails this test loudly, which is the right failure mode for the
/// only evidence available.
#[derive(Debug, Default)]
struct SpillRecorder(Mutex<Vec<String>>);

impl SpillRecorder {
    /// The spill records since the last call, clearing them.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("spill log not poisoned"))
    }
}

impl log::Log for SpillRecorder {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let message = record.args().to_string();
        if SPILL_MARKERS.iter().any(|m| message.contains(m)) {
            self.0.lock().expect("spill log not poisoned").push(message);
        }
    }

    fn flush(&self) {}
}

fn install_spill_recorder() -> &'static SpillRecorder {
    static RECORDER: OnceLock<&'static SpillRecorder> = OnceLock::new();
    RECORDER.get_or_init(|| {
        let recorder: &'static SpillRecorder = Box::leak(Box::new(SpillRecorder::default()));
        log::set_logger(recorder).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Debug);
        recorder
    })
}
