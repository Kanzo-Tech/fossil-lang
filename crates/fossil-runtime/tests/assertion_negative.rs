//! SC#4 / P-CRIT-4 native negative test — the named runtime assertion must FAIL
//! LOUDLY (never silently) when its guard is tripped at runtime.
//!
//! Plan 04-06 emits, where a static check cannot be discharged,
//! `CASE WHEN <guard> THEN <value> ELSE
//! error('fossil_assertion_<name>:line=<N>') END` into the generated SQL. This
//! test proves the contract end-to-end against native `DuckDB` 1.10502: feed a
//! row whose guarded column is NULL, execute the assertion SQL via
//! [`fossil_runtime::execute`], and assert it returns `Err` with the `DuckDB`
//! error message CONTAINING the named assertion (`fossil_assertion_…`). If the
//! assertion silently passed (the P-CRIT-4 failure mode), `execute` would
//! return `Ok` and this test would fail.
//!
//! NATIVE-ONLY: `fossil-runtime` carries a `compile_error!` cfg-tripwire on
//! `wasm32` (DuckDB-WASM parity for `error()` is exercised by plan 04-07's
//! harness, not here). The table is built in-memory (`CREATE TABLE` + `INSERT`),
//! so the test needs no CSV file on disk.

#![cfg(not(target_arch = "wasm32"))]

/// The exact `CASE WHEN ... error(...)` shape codegen emits for the
/// `iri_template_unbound` assertion (mirrors
/// `crates/fossil-codegen/tests/snapshots/codegen_ops__assert_iri_template_unbound.snap`).
/// The `line=42` matches the codegen snapshot's fixed line.
const ASSERTION_SQL: &str = "\
CREATE TABLE users(id VARCHAR, name VARCHAR);
INSERT INTO users VALUES (NULL, 'Alice');
SELECT
    'https://example.org/user/' || CASE WHEN users.id IS NOT NULL THEN users.id ELSE error('fossil_assertion_iri_template_unbound:line=42') END AS subject
FROM users;";

#[test]
fn null_field_trips_named_assertion_loudly() {
    let result = fossil_runtime::execute(ASSERTION_SQL);

    let err = result.expect_err(
        "P-CRIT-4: a NULL field in an IRI template MUST trip the named runtime \
         assertion (DuckDB error) — never silently produce a malformed IRI",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("fossil_assertion_iri_template_unbound"),
        "DuckDB must raise with the NAMED assertion message; got: {msg}"
    );
    assert!(
        msg.contains("line=42"),
        "the named assertion must carry its source line; got: {msg}"
    );
}

/// Companion sanity check: when the guarded column is NOT NULL, the SAME SQL
/// must SUCCEED — the assertion only raises on the un-dischargeable case, it
/// does not break valid data.
#[test]
fn non_null_field_passes_the_assertion() {
    let sql = "\
CREATE TABLE users(id VARCHAR, name VARCHAR);
INSERT INTO users VALUES ('1', 'Alice');
SELECT
    'https://example.org/user/' || CASE WHEN users.id IS NOT NULL THEN users.id ELSE error('fossil_assertion_iri_template_unbound:line=42') END AS subject
FROM users;";

    fossil_runtime::execute(sql)
        .expect("a non-NULL field must satisfy the guard and execute cleanly");
}
