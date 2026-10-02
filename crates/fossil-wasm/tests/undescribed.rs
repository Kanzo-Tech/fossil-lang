//! A source the host could not describe is a row of `check`, at the call that
//! reads it, under the code its introspection answered.
//!
//! `introspect` in `@fossil-lang/introspect` returns `{ descriptors, undescribed }`
//! and `FossilProgram.registerIntrospection` hands both halves over; this is the
//! Rust half the second one drives.

#![cfg(not(target_arch = "wasm32"))]

use fossil_wasm::{CheckRow, FossilWorkspace};
use lsp_types::{DiagnosticSeverity, Position};

const PROGRAM: &str = "\
users := io.csv(\"@lake/users.csv\")
again := io.csv(\"@lake/users.csv\")
orders := io.csv(\"@lake/orders.csv\")
";

/// What `introspect` answers for a source whose file is not there — the
/// TypeScript `Problem`, `title` and `detail` included, as a host passes it.
const NOT_FOUND: &str = r#"{
  "code": "source/not-found",
  "data": { "locator": "s3://lake/users.csv" },
  "title": "A source names no file",
  "detail": "the source `s3://lake/users.csv` names no file",
  "severity": "error",
  "help": "check the path under the connection"
}"#;

const ORDERS_DESCRIPTOR: &str = r#"{
  "uri": "@lake/orders.csv",
  "columns": [{ "name": "id", "primitive": "string" }],
  "freshness_token": ""
}"#;

const USERS_DESCRIPTOR: &str = r#"{
  "uri": "@lake/users.csv",
  "columns": [{ "name": "id", "primitive": "string" }],
  "freshness_token": ""
}"#;

fn undescribed_rows(ws: &FossilWorkspace) -> Vec<CheckRow> {
    ws.check_rows()
        .into_iter()
        .filter(|r| r.code == "source/not-found")
        .collect()
}

#[test]
fn a_source_that_could_not_be_described_is_one_row_at_its_call() {
    let mut ws = FossilWorkspace::new();
    let program = ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    ws.register_inferred_descriptor_native(ORDERS_DESCRIPTOR)
        .expect("descriptor");
    ws.register_undescribed_native("@lake/users.csv", NOT_FOUND)
        .expect("problem");

    let rows = undescribed_rows(&ws);
    assert_eq!(
        rows.len(),
        1,
        "two bindings read one key, one row: {rows:#?}"
    );
    let row = &rows[0];
    assert_eq!(row.uri, "prog.fossil");
    assert_eq!(row.severity, DiagnosticSeverity::WARNING);
    assert_eq!(row.range.start, Position::new(0, 9), "starts at `io.csv`");
    assert_eq!(row.range.end, Position::new(0, 34), "ends after the call");
    assert_eq!(row.detail.title, "A source names no file");
    assert_eq!(row.detail.data["locator"], "s3://lake/users.csv");
    assert_eq!(
        row.detail.help.as_deref(),
        Some("check the path under the connection")
    );
    assert_eq!(
        row.message,
        "the source `s3://lake/users.csv` names no file"
    );

    assert_eq!(
        ws.diagnostics_for_rows(program)
            .expect("open")
            .iter()
            .filter(|r| r.code == "source/not-found")
            .count(),
        1,
        "the per-file drain carries it too"
    );
}

#[test]
fn a_described_source_has_no_row() {
    let mut ws = FossilWorkspace::new();
    ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    ws.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("descriptor");
    ws.register_inferred_descriptor_native(ORDERS_DESCRIPTOR)
        .expect("descriptor");
    assert!(undescribed_rows(&ws).is_empty());
}

/// The latest answer about a key is the one reported, whichever came first.
#[test]
fn describing_a_source_later_forgets_why_it_could_not_be() {
    let mut ws = FossilWorkspace::new();
    ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    ws.register_undescribed_native("@lake/users.csv", NOT_FOUND)
        .expect("problem");
    assert_eq!(undescribed_rows(&ws).len(), 1);

    ws.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("descriptor");
    assert!(undescribed_rows(&ws).is_empty());
}

#[test]
fn a_key_the_program_does_not_read_is_not_reported() {
    let mut ws = FossilWorkspace::new();
    ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    ws.register_undescribed_native("@lake/elsewhere.csv", NOT_FOUND)
        .expect("problem");
    assert!(undescribed_rows(&ws).is_empty());
}

#[test]
fn a_problem_fossil_does_not_know_is_refused() {
    let mut ws = FossilWorkspace::new();
    let failure = ws
        .register_undescribed_native("@lake/users.csv", r#"{ "code": "nope/never", "data": {} }"#)
        .expect_err("unknown code");
    assert_eq!(failure.problem.code(), "api/invalid-argument");
}
