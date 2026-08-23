//! Every `LoweringKind::Expr` template is real SQL that a real `DuckDB` binds.
//!
//! Native-only, and no longer for the reason this line used to give. It said
//! `fossil-runtime` carried a `wasm32` `compile_error!` tripwire; `1e11a91`
//! deleted that tripwire, and the crate now compiles for wasm32 and declares it
//! with `[package.metadata.fossil] wasm = true` — it is IN the
//! `cargo xtask wasm-check` closure. What stays native is THIS FILE, which
//! links the `DuckDB` dev-dependency the library no longer has. The gate never
//! builds it: it runs `cargo check --target wasm32-unknown-unknown` without
//! `--all-targets`, so no `tests/` target is compiled for wasm32.
//!
//! # What this replaced, and why it is strictly stronger
//!
//! There were TWO guards here and neither checked what mattered.
//! `DUCKDB_BUILTIN_ALLOWLIST` was a curated list of names a `Builtin` row was
//! allowed to render to, and `fossil-hir/tests/stdlib_classification_gate.rs`
//! asserted membership in it — a list checked against itself. This file then
//! executed `SELECT <duckdb_name>(<dummy args>)` to prove the NAME was real.
//! Between them they proved a function name existed, and they could not see the
//! nine `InlineForm` variants at all, because those rendered no name: the SQL
//! shape of `CAST(x AS BIGINT)` or `CASE WHEN x IS NULL THEN error(…) END` was
//! written in a doc-comment, where nothing could execute it.
//!
//! Ruling 15 of `SURFACE-PLAN.md` made the template the datum. So this test
//! executes THE TEMPLATE — every row, whole, with its arguments substituted —
//! and both old guards fall out of it: a misspelt function name and a
//! malformed expression are the same failure now, and the allowlist that had to
//! be kept in step by hand is gone.
//!
//! # The pass criterion, and why it is not "returns a value"
//!
//! A template is fed dummy arguments typed from its own [`SigSpec`], and dummy
//! arguments cannot satisfy every row: `strptime('abc', 'abc')` is a real call
//! to a real function that fails at RUNTIME, and `validate.uuid` is SUPPOSED to
//! raise on input that is not a UUID — that is the whole of what it does.
//!
//! So the criterion is BINDING, not evaluation. A `Catalog`, `Parser` or
//! `Binder` error means the template does not name real SQL and is a failure. A
//! `Conversion` or `Invalid Input` error means `DuckDB` bound the expression and
//! then disliked the data, which is the template working.

#![cfg(not(target_arch = "wasm32"))]

use duckdb::Connection;
use fossil_hir::stdlib::{FunctionRegistry, LoweringKind, ScalarTy, render_template};

/// A literal of the right SQL type for a parameter, so the template binds.
///
/// Deliberately NOT tailored per row: a per-row valid argument would make this
/// a test of the arguments. The types come from the row's own signature, which
/// is the only thing the catalogue promises about them.
fn dummy(ty: ScalarTy) -> String {
    match ty {
        ScalarTy::String => "'abc'".to_string(),
        ScalarTy::Integer => "2".to_string(),
        ScalarTy::Float => "1.5".to_string(),
        ScalarTy::Bool => "true".to_string(),
        ScalarTy::Date => "DATE '2026-05-21'".to_string(),
        ScalarTy::DateTime => "TIMESTAMP '2026-05-21 00:00:00'".to_string(),
        ScalarTy::SeqString => "['a', 'b']".to_string(),
    }
}

/// Did `DuckDB` fail to BIND this expression, as opposed to disliking the data?
///
/// The three binding failures are the ones that mean the template is not SQL:
/// a function that does not exist, a statement that does not parse, and an
/// expression whose types cannot be resolved.
fn is_binding_failure(msg: &str) -> bool {
    msg.contains("Catalog Error")
        || msg.contains("Parser Error")
        || msg.contains("Binder Error")
        || msg.contains("does not exist")
}

#[test]
fn every_expr_template_binds_on_a_real_duckdb() {
    let conn = Connection::open_in_memory().expect("open in-memory DuckDB");
    let reg = FunctionRegistry::stdlib_default();

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for entry in reg.iter() {
        let LoweringKind::Expr(template) = &entry.lowering else {
            // `Op` rows name an operator of the algebra, not a scalar
            // expression. There is nothing to SELECT.
            continue;
        };
        // Every parameter of an `Expr` row is a scalar: a relation and a
        // condition belong to the verbs, and those are `Op` rows, skipped
        // above. `expect` rather than a filter, because a scalar template whose
        // parameter is not a scalar is a catalogue bug this test should fail on.
        let args: Vec<String> = entry
            .sig
            .params
            .iter()
            .map(|p| dummy(p.ty.scalar().expect("an `Expr` row takes scalars")))
            .collect();
        let sql = format!("SELECT {}", render_template(template.as_str(), &args));

        checked += 1;
        if let Err(e) = conn.prepare(&sql).and_then(|mut s| {
            s.query_row([], |row| row.get::<_, duckdb::types::Value>(0))
                .map(|_| ())
        }) {
            let msg = e.to_string();
            if is_binding_failure(&msg) {
                failures.push(format!("`{}`\n    sql: {sql}\n    err: {msg}", entry.name));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} template(s) are not SQL DuckDB can bind:\n  {}",
        failures.len(),
        failures.join("\n  "),
    );
    // The catalogue is not allowed to become all-`Op` without anyone noticing:
    // an empty loop would pass the assertion above silently.
    // 48 surface rows, of which the 13 relation verbs and the 3 `io/`
    // constructors are `Op`. The remainder is what a real DuckDB just bound.
    assert_eq!(
        checked, 35,
        "the number of Expr templates moved; the catalogue or the lowering \
         kinds changed and this smoke should say so rather than shrink quietly"
    );
}

/// The four validators return their input when it is valid and RAISE when it is
/// not, and both halves are the point: a validator that returns NULL on bad
/// input is the silent-failure mode this project exists to make impossible.
///
/// This also pins the two `DuckDB` facts the templates rest on, measured rather
/// than assumed: `error()` inside a `CASE` arm is lazy, and a NULL predicate
/// takes the ELSE branch (which is why every validator opens `%0 IS NULL OR`).
#[test]
fn a_validator_returns_its_input_raises_on_bad_input_and_passes_null_through() {
    let conn = Connection::open_in_memory().expect("open in-memory DuckDB");
    let reg = FunctionRegistry::stdlib_default();

    // One valid value per validator, so the THEN arm is exercised.
    let valid: &[(&str, &str)] = &[
        ("validate.email", "'a@b.com'"),
        ("validate.url", "'https://example.org/x'"),
        ("validate.uuid", "'550e8400-e29b-41d4-a716-446655440000'"),
        ("validate.iso_date", "'2026-05-21'"),
    ];

    for (name, good) in valid {
        let entry = reg
            .lookup(name)
            .unwrap_or_else(|| panic!("`{name}` is catalogued"));
        let LoweringKind::Expr(template) = &entry.lowering else {
            panic!("`{name}` must be an Expr row");
        };

        // Valid input comes back unchanged.
        let sql = format!(
            "SELECT {}",
            render_template(template.as_str(), &[(*good).to_string()])
        );
        let got: String = conn
            .prepare(&sql)
            .and_then(|mut s| s.query_row([], |r| r.get::<_, String>(0)))
            .unwrap_or_else(|e| panic!("`{name}` on valid input failed: {e}\n{sql}"));
        assert_eq!(
            format!("'{got}'"),
            *good,
            "`{name}` must return its input unchanged"
        );

        // Invalid input RAISES, and the message names the function.
        let sql = format!(
            "SELECT {}",
            render_template(
                template.as_str(),
                &["'!! definitely not valid !!'".to_string()]
            )
        );
        let err = conn
            .prepare(&sql)
            .and_then(|mut s| s.query_row([], |r| r.get::<_, String>(0)))
            .expect_err("invalid input must raise, not return NULL");
        assert!(
            err.to_string().contains(name),
            "`{name}` must name itself in its error; got {err}"
        );

        // NULL passes through as NULL rather than raising. Without the
        // `%0 IS NULL OR` guard this is an error, because `regexp_matches(NULL,
        // …)` is NULL and a NULL predicate takes the ELSE branch.
        let sql = format!(
            "SELECT {}",
            render_template(template.as_str(), &["CAST(NULL AS VARCHAR)".to_string()])
        );
        let got: Option<String> = conn
            .prepare(&sql)
            .and_then(|mut s| s.query_row([], |r| r.get::<_, Option<String>>(0)))
            .unwrap_or_else(|e| panic!("`{name}` on NULL raised: {e}\n{sql}"));
        assert!(got.is_none(), "`{name}` must pass NULL through");
    }
}

/// `error()` in a `CASE` arm is lazy PER ROW, not merely per query.
///
/// This is the fact every validator rests on and the one that would be most
/// expensive to discover late: were it eager, a column containing one bad value
/// would not fail on that row — it would fail on every query that mentions the
/// column, including one whose rows are all valid.
#[test]
fn error_in_a_case_arm_is_lazy_per_row() {
    let conn = Connection::open_in_memory().expect("open in-memory DuckDB");

    let n: i64 = conn
        .prepare(
            "SELECT count(*) FROM (
               SELECT CASE WHEN i < 5000 THEN i ELSE error('boom') END AS v
               FROM range(5000) t(i)
             ) WHERE v IS NOT NULL",
        )
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .expect("5000 valid rows must not raise");
    assert_eq!(n, 5000);

    let err = conn
        .prepare(
            "SELECT count(*) FROM (
               SELECT CASE WHEN i < 4999 THEN i ELSE error('boom') END AS v
               FROM range(5000) t(i)
             ) WHERE v IS NOT NULL",
        )
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .expect_err("the one bad row at 4999 must raise");
    assert!(err.to_string().contains("boom"), "got {err}");
}
