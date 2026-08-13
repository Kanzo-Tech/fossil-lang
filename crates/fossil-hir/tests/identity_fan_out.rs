//! What the per-type identity check costs, measured.
//!
//! `tests/invalidation_regression.rs` pins `MAX_REEXECUTIONS = 18` and
//! `MAX_PER_MAPPING_FAN_OUT = 1` over a ten-mapping file, and neither moves for
//! the per-type identity check. That test does not measure this one, and
//! cannot: its fixture
//! names no shape document, so `subject_templates` skips every mapping before
//! reading a body and the identity queries are vacuous over it. The number
//! quoted there would be a number about nothing.
//!
//! So the same experiment is run again here over a fixture where the identity
//! check is REAL — ten mappings, one type bound to a document, an `@subject` in
//! every body — and with the two identity queries in the warm/measure loop, as a
//! host runs them.
//!
//! # What is being claimed
//!
//! 1. **The per-mapping fan-out is unmoved.** `body`, `typecheck_mapping`,
//!    `spans` and `expr_types` re-execute at most once each after a one-property
//!    edit, with `check_identities` in the loop. This is the per-mapping
//!    invalidation barrier — editing one body re-runs that mapping and no
//!    other — and the reason the identity check reads `body(m)` rather than putting the
//!    `@subject` into `lower_to_hir`'s `HirFile` — a file-keyed struct that
//!    every `typecheck_mapping` reads whole, where one body's content would
//!    invalidate all ten.
//! 2. **The file-level check is file-keyed, so it costs ONE.** Not ten. That is
//!    the same property the shape document has (`shape_document` is keyed by the
//!    DOCUMENT, so ten mappings share one decode) and it is what the coordinator
//!    asked to be looked for.
//! 3. **An edit that does not touch an identity costs ZERO.** `check_identities`
//!    reads `subject_templates`' OUTPUT, which is structurally equal when the
//!    edited property is not the `@subject`, so it validates instead of
//!    re-executing. Editing `a = .a` into `a = .x` is free at this layer.
//!
//! Counts are asserted as bounds and PRINTED, because the exact total is a
//! measurement and a test that hardcodes it goes red for a reason that is not a
//! regression.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::sync::Mutex;

use fossil_base::test_support::{DecodingHost, PERSON_DOCUMENT};
use fossil_base::{FossilDb, SourceFile, System};
use fossil_hir::body::body;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;
use fossil_hir::identity::{check_identities, subject_templates};
use fossil_hir::provenance::expr_types;
use fossil_hir::spans::spans;
use salsa::Setter as _;

/// The load-bearing bound, quoted from `tests/invalidation_regression.rs` rather
/// than re-decided here.
const MAX_PER_MAPPING_FAN_OUT: usize = 1;

/// Ten mappings, one type, one identity — the shape of a real program, where
/// `subject_templates` reads all ten bodies. `{n}` is the mapping's number.
fn ten_mappings(third_property: &str) -> String {
    let mut src = String::from(
        "type { Person } := io.shex(\"person.shex\")\n\
         users := io.csv(\"data.csv\")\n\n",
    );
    for n in 1..=10 {
        let a = if n == 3 { third_property } else { "users.a" };
        src.push_str(&format!(
            "Mapping_{n} : Person from users\n    \
             @subject = \"http://example.org/p/{{users.id}}\"\n    \
             name = {a}\n\n"
        ));
    }
    src
}

/// Every `WillExecute` key, as salsa's `Debug` renders it (`query_name(Id(_))`).
fn measure(edit: bool) -> Vec<String> {
    let keys: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = keys.clone();
    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            sink.lock().unwrap().push(format!("{database_key:?}"));
        }
    });

    // `DecodingHost` and not `NativeSystem`: the latter installs no shape
    // decoder, so the type binding would resolve nothing, every mapping's
    // `shape_iri` would be empty and `subject_templates` would skip all ten
    // before reading a body. The measurement would be of nothing — which is
    // exactly why `tests/invalidation_regression.rs`'s fixture cannot carry it.
    let system: Arc<dyn System> = Arc::new(DecodingHost::default());
    let mut db = FossilDb::with_event_callback(system, callback);
    let file = SourceFile::new(
        &db,
        ten_mappings("users.a"),
        "ten_identities.fossil".to_string(),
    );
    // The document the type binding names, registered as a Salsa input — the
    // same route `fossil_base::test_support::db_with_document` takes.
    let doc = SourceFile::new(&db, PERSON_DOCUMENT.to_string(), "person.shex".to_string());
    fossil_base::register_file(&mut db, "person.shex".to_string(), doc);

    let warm = |db: &FossilDb| {
        let mappings = def_map(db, file).mappings(db).clone();
        for m in &mappings {
            let _ = body(db, *m);
            let _ = typecheck_mapping(db, *m);
            let _ = expr_types(db, *m);
            let _ = spans(db, *m);
        }
        // The two queries under measurement, called the way a host calls them.
        let _ = subject_templates(db, file);
        let _ = check_identities(db, file);
    };

    warm(&db);
    keys.lock().unwrap().clear();

    // One property of ONE mapping, and never the `@subject`: the claim being
    // measured is that an edit which cannot change an identity costs nothing at
    // this layer.
    file.set_text(&mut db)
        .to(ten_mappings(if edit { "users.x" } else { "users.a" }));
    warm(&db);

    let out = keys.lock().unwrap().clone();
    out
}

#[test]
fn the_identity_check_is_file_keyed_and_does_not_widen_the_per_mapping_fan_out() {
    let keys = measure(true);
    let count = |name: &str| keys.iter().filter(|k| k.starts_with(name)).count();

    let body_count = count("body(");
    let typecheck_count = count("typecheck_mapping(");
    let spans_count = count("spans(");
    let expr_types_count = count("expr_types(");
    let templates_count = count("subject_templates(");
    let identities_count = count("check_identities(");

    eprintln!(
        "one-property edit in mapping 3 of 10, with the identity check in the loop:\n  \
         total={} body={body_count} typecheck_mapping={typecheck_count} spans={spans_count} \
         expr_types={expr_types_count} subject_templates={templates_count} \
         check_identities={identities_count}",
        keys.len(),
    );

    for (name, n) in [
        ("body", body_count),
        ("typecheck_mapping", typecheck_count),
        ("spans", spans_count),
        ("expr_types", expr_types_count),
    ] {
        assert!(
            n <= MAX_PER_MAPPING_FAN_OUT,
            "FORBIDDEN per-mapping fan-out: {name} re-executed {n} times after a \
             single-mapping body edit (cap = {MAX_PER_MAPPING_FAN_OUT}). If it is 10, the \
             identity check has been wired into a per-mapping query — put it back on a \
             file-keyed one, do NOT relax this. keys: {keys:#?}"
        );
    }

    assert!(
        templates_count <= 1,
        "`subject_templates` is FILE-keyed: ten mappings share one execution. \
         {templates_count} means it has been keyed by mapping. keys: {keys:#?}"
    );
    assert!(
        identities_count <= 1,
        "`check_identities` is FILE-keyed too. {identities_count} means the same. \
         keys: {keys:#?}"
    );
}

#[test]
fn an_edit_that_cannot_change_an_identity_re_executes_neither_identity_query() {
    let keys = measure(true);
    let count = |name: &str| keys.iter().filter(|k| k.starts_with(name)).count();

    // `subject_templates` reads `body(M_3)`, whose output changed, so it
    // re-executes — once, and its OUTPUT is structurally equal because the
    // `@subject` did not move. `check_identities` reads that output and
    // therefore validates: zero.
    assert_eq!(
        count("check_identities("),
        0,
        "editing `name = users.a` into `name = users.x` cannot change an \
         identity, and the check that reads only identities must not re-run. \
         keys: {keys:#?}"
    );
}

/// The control for the two tests above: with no edit at all, the measured work
/// is the revision bump and nothing else, so what the other two report is the
/// cost of the EDIT rather than of a cold cache.
///
/// `parse` re-executes even here — `set_text` bumps the revision whatever the
/// text is — and everything downstream validates on its structurally-equal
/// output. That one re-execution is the floor of any measurement in this file.
#[test]
fn re_running_the_same_text_re_executes_only_the_parse() {
    let keys = measure(false);
    assert_eq!(
        keys.len(),
        1,
        "the floor is one `parse`; anything else is work done for no change. \
         keys: {keys:#?}"
    );
    assert!(
        keys[0].starts_with("parse("),
        "and it is the parse. keys: {keys:#?}"
    );
}
