//! Every `LoweringKind::Expr` template is real SQL that a real `DuckDB` binds.
//!
//! Native-only because THIS FILE links the `DuckDB` dev-dependency; the library
//! compiles for wasm32. The gate never builds it — `cargo xtask wasm-check` runs
//! without `--all-targets`, so no `tests/` target is compiled for wasm32.
//!
//! # The pass criterion, and why it is not "returns a value"
//!
//! A template is fed dummy arguments typed from its own [`SigSpec`], and dummy
//! arguments cannot satisfy every row: `strptime('abc', 'abc')` is a real call
//! to a real function that fails at RUNTIME, and `CAST('abc' AS BIGINT)` is a
//! real cast that refuses real data.
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
    // an empty loop would pass the assertion above silently. Derived rather
    // than the literal `35` it was — that number described a catalogue of 48
    // surface rows and went stale the first time one was deleted, and what this
    // needs to know is that EVERY `Expr` row reached the engine.
    let expr_rows = FunctionRegistry::stdlib_default()
        .iter()
        .filter(|e| matches!(e.lowering, LoweringKind::Expr(_)))
        .count();
    assert_eq!(
        checked, expr_rows,
        "every `Expr` row must bind on DuckDB; {checked} of {expr_rows} were tried"
    );
    assert!(
        checked > 20,
        "only {checked} template(s) were bound; a loop over almost nothing proves \
         almost nothing"
    );
}

// `a_validator_returns_its_input_raises_on_bad_input_and_passes_null_through`
// stood here. It fed each of the four `validate.*` templates a valid value, an
// invalid one and a NULL, and asserted the shape all five rows of that
// namespace shared: the value or an error, never a null.
//
// The namespace is gone from the language, so the test has no subject. What it
// proved about DuckDB is not gone, and the test below keeps the load-bearing
// half of it: `error()` in a `CASE` arm is lazy per row. That is why `validate/`
// was expressible on this engine at all — and, read the other way, exactly what
// DataFusion has no spelling for, which is why the rows were deleted rather than
// carried as a permanent gap. It is kept as the evidence behind that decision,
// executable rather than recalled.

/// `error()` in a `CASE` arm is lazy PER ROW, not merely per query.
///
/// This is the fact every `validate.*` row rested on, and it is kept because it
/// is the evidence for their deletion rather than a leftover: laziness is what
/// made "the value or an error" expressible on `DuckDB`, and having no way to
/// raise from an expression at all is what makes it inexpressible on
/// `DataFusion`.
/// Were it eager even here, a column containing one bad value would not fail on
/// that row — it would fail on every query that mentions the column, including
/// one whose rows are all valid.
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
