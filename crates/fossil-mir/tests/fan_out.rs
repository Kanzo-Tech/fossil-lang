//! Guard: the per-mapping invalidation barrier holds THROUGH the MIR lowering.
//!
//! # Why this exists
//!
//! Four places in this crate assert it. `src/lower.rs`'s "CRITICAL barrier
//! rule" says `lower_to_mir` must not add a `parse(db, file)` read in the
//! per-mapping path, "would break `MAX_PER_MAPPING_FAN_OUT = 1`"; two function
//! doc comments repeat the constant as the reason they are written the way they
//! are; `src/schema.rs` opens by saying its queries do not affect it.
//!
//! **The constant exists only in two `fossil-hir` test files**, and both measure
//! `fossil-hir` queries — `body`, `typecheck_mapping`, `expr_types`, `spans`,
//! `check_identities`. Nothing in `crates/fossil-mir/tests/` instrumented Salsa
//! at all, so every one of the four sentences above was unheld: a
//! `parse(db, file)` read added to `lower_to_mir_pg` would leave all of
//! `fossil-hir` green, and the editor would re-lower ten mappings on every
//! keystroke in one of them.
//!
//! # What this proves
//!
//! One character changes in the body of mapping 3 of 10, and `lower_to_mir_pg`
//! re-executes **once**. The number 10 is the corpus, not the claim: what the
//! barrier buys is that the count does not grow with it, so the test asserts
//! against the mapping count it built rather than against a literal.
//!
//! The mechanism is the one the hir tests use — `FossilDb::with_event_callback`
//! and the `WillExecute` key names — because it is the only thing that can see a
//! query re-run. The premises are checked before the claim is: the fixture must
//! differ, the lowering must actually produce ops (a poisoned graph that returns
//! before reading a body would make the fan-out trivially 1), and the warm pass
//! must have executed the query for every mapping.
//!
//! # What this CANNOT prove
//!
//! - **That `lower_to_mir_pg` never reads `parse`.** It measures the
//!   consequence, not the call. A `parse` read on a path this fixture does not
//!   take — a mapping with an edge constructor, a `join`, a source the
//!   catalogue cannot resolve — is invisible here. The barrier rule is still
//!   the thing to read before adding a read; this is what notices the common
//!   case going wrong.
//! - **`MAX_REEXECUTIONS`.** `fossil-hir`'s total-cascade bound is its own
//!   number over its own query set and is not restated here. This is about one
//!   query's fan-out.
//! - **Anything about `apply_output_shape` or `schema_of`.** They are not in the
//!   loop. `src/schema.rs`'s claim that its queries do not affect the fan-out
//!   remains prose.
//! - **That the editor is fast.** A fan-out of 1 over a re-lowering that got
//!   ten times slower per mapping measures the same here.

#![cfg(not(target_arch = "wasm32"))]
// `@subject` below is an interpolated fossil string: `{users.id}` is fossil's
// hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use fossil_base::test_support::{DecodingHost, register_document};
use fossil_base::{FossilDb, SourceFile, System};
use fossil_hir::def_map::def_map;
use fossil_mir::lower_to_mir_pg;
use salsa::Setter as _;

/// The shape all ten mappings target: one un-narrowed `name` predicate.
const DOCUMENT: &str = "\
shape https://example.org/Person
prop https://example.org/name - 1 1
";

const DOCUMENT_PATH: &str = "ten.shex";
const PROGRAM_PATH: &str = "ten.fossil";

/// How many mappings the fixture carries. The barrier's whole content is that
/// the fan-out does not grow with this, so it is a parameter and not a claim —
/// raise it and the assertions still say the same thing.
const MAPPINGS: usize = 10;

/// The mapping whose body is edited. Interior, so a fan-out that leaked would
/// leak in both directions.
const EDITED: usize = 3;

/// Ten mappings of one type, sharing one `@subject` template — two that
/// disagreed would be a compile error — and differing only in the column each
/// body reads. `column` names the one mapping [`EDITED`] reads.
fn program(column: &dyn Fn(usize) -> String) -> String {
    let mut src =
        String::from("type { Person } := io.shex(\"ten.shex\")\n\nusers := io.csv(\"data.csv\")\n");
    for n in 1..=MAPPINGS {
        write!(
            src,
            "\nMapping_{n} : Person from users\n    \
             @subject = \"https://example.org/u/{{users.id}}\"\n    \
             name = users.{}\n",
            column(n)
        )
        .expect("writing to a String cannot fail");
    }
    src
}

/// The `WillExecute` keys captured between a `clear` and a read.
type Keys = Arc<Mutex<Vec<String>>>;

fn db_with_log() -> (FossilDb, Keys) {
    let keys: Keys = Arc::new(Mutex::new(Vec::new()));
    let sink = keys.clone();
    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            // Salsa 0.26 renders `query_name(Id(raw))` while the database is
            // attached, which it is inside the callback. Not a stability
            // guarantee — the premise assertions below fail loudly rather than
            // pass quietly if the rendering stops carrying a name.
            sink.lock()
                .expect("key log")
                .push(format!("{database_key:?}"));
        }
    });
    let system: Arc<dyn System> = Arc::new(DecodingHost::default());
    (FossilDb::with_event_callback(system, callback), keys)
}

/// Lower every mapping in `file`, returning how many produced a non-empty
/// operator list.
fn lower_all(db: &FossilDb, file: SourceFile) -> usize {
    let mappings: Vec<_> = def_map(db, file).mappings(db).clone();
    mappings
        .iter()
        .filter(|m| !lower_to_mir_pg(db, **m).ops(db).is_empty())
        .count()
}

#[test]
fn one_body_edit_re_lowers_one_mapping() {
    let baseline = program(&|n| format!("c{n}"));
    let edited = program(&|n| {
        if n == EDITED {
            "d".to_string()
        } else {
            format!("c{n}")
        }
    });
    assert_ne!(baseline, edited, "the two fixtures must differ");
    assert_eq!(
        baseline.lines().count(),
        edited.lines().count(),
        "the edit must be inside one body, not a change of shape"
    );

    let (mut db, keys) = db_with_log();
    let file = SourceFile::new(&db, baseline, PROGRAM_PATH.to_string());
    register_document(&mut db, DOCUMENT_PATH, DOCUMENT);

    // Warm. A graph that poisons before reading a body would give this test a
    // fan-out of 1 for the wrong reason, so the lowering has to be real.
    let lowered = lower_all(&db, file);
    assert_eq!(
        lowered, MAPPINGS,
        "the warm pass lowered {lowered} of {MAPPINGS} mappings to a non-empty \
         graph. This guard measures re-execution of a query that did something; \
         fix the fixture, not the assertion."
    );

    let warm = keys.lock().expect("key log").clone();
    let warm_lowerings = count(&warm, "lower_to_mir_pg(");
    assert_eq!(
        warm_lowerings, MAPPINGS,
        "the warm pass shows {warm_lowerings} `lower_to_mir_pg` executions for \
         {MAPPINGS} mappings. Either the query is memoised across mappings — in \
         which case this measures nothing — or salsa stopped rendering query \
         names in `DatabaseKeyIndex`'s `Debug`, and this guard must be rewritten \
         rather than relaxed. Keys:\n{warm:#?}"
    );

    keys.lock().expect("key log").clear();
    file.set_text(&mut db).to(edited);
    let relowered = lower_all(&db, file);
    assert_eq!(
        relowered, MAPPINGS,
        "every mapping still lowers after the edit"
    );

    let after = keys.lock().expect("key log").clone();
    let lowerings = count(&after, "lower_to_mir_pg(");
    assert_eq!(
        lowerings, 1,
        "editing one property of mapping {EDITED} of {MAPPINGS} re-lowered \
         {lowerings} mapping(s). The per-mapping barrier is gone: something on \
         the `lower_to_mir_pg` path now reads a whole-file query — \
         `parse(db, file)` is the one `src/lower.rs` names — so every sibling's \
         input changed with the edit. Fix the data layout; do not relax this \
         number. Keys:\n{after:#?}"
    );
}

/// How many captured keys name `query`.
fn count(keys: &[String], query: &str) -> usize {
    keys.iter().filter(|k| k.starts_with(query)).count()
}
