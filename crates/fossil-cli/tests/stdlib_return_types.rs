//! Every `LoweringKind::Expr` row returns the type its `sig.ret` declares, and
//! a real `DuckDB` is the one asked.
//!
//! NATIVE-ONLY (`fossil-cli` carries a `wasm32` `compile_error!` tripwire).
//!
//! # The defect this is the guard for
//!
//! `sig.ret` is what the CHECKER believes — `fossil_hir::check::check_call`
//! returns it, `fossil_mir::lower::call_result_ty` gives the property that type,
//! and the `GraphAr` writer stamps the column from it. `lowering` is what RUNS.
//! When the two disagree the program type-checks and produces something else,
//! and nothing anywhere fails.
//!
//! It had happened five times in a catalogue of 51 rows, and none of the five
//! was visible from the Rust:
//!
//! | row | declared | the template actually returned |
//! |---|---|---|
//! | `validate.regex` | `String` | `BOOLEAN` — a bare `regexp_matches` |
//! | `math.round` | `Integer` | `DOUBLE` — `round(DOUBLE)` is a `DOUBLE` |
//! | `parse.date` | `Date` | `TIMESTAMP` — `strptime` always is |
//! | `parse.json` | `String` | `JSON` — `DuckDB`'s own type |
//! | `parse.decimal` | `Float` | `DECIMAL(38,18)` |
//!
//! The two guards that existed could not see any of them. `builtin_smoke.rs`
//! executes each template and asks whether `DuckDB` BOUND it, which every one of
//! the five passed — they are all valid SQL, they are the WRONG valid SQL.
//! `fossil-df`'s `UNREACHABLE_ON_DATAFUSION` asks whether a template renders at
//! all. Neither reads `sig.ret`, so neither could compare the halves.
//!
//! # Why `DESCRIBE` and not `typeof`
//!
//! `typeof(<expr>)` evaluates its argument, and four rows are SUPPOSED to raise
//! on input that is not valid — asking `typeof(validate.uuid('abc'))` gets the
//! validator working, not its type. `DESCRIBE SELECT <expr>` answers from the
//! BINDER, before a row exists, so every one of the 35 templates is covered with
//! no exclusion list to keep in step. That is the difference between this and
//! putting it on `fossil-df`, where `error()` has no spelling and six rows would
//! have had to be declared unprovable.
//!
//! # The dummy arguments are CAST, and that is load-bearing
//!
//! `DuckDB` types the literal `1.5` as `DECIMAL(2,1)`, not `DOUBLE`. Fed that,
//! `math.abs` reports `DECIMAL(2,1)` and every `math/` row looks like a lie.
//! What is being asked is what the TEMPLATE returns given the parameter types
//! the row itself declares, so each dummy is cast to the SQL type
//! [`sql_type_of`] names — the same mapping the assertion uses, so an argument
//! and a return of the same `ScalarTy` cannot disagree about what it means.
//!
//! # What this does NOT prove
//!
//! - **The 16 `Op` rows.** A `PlanOp` names an operator of the algebra; there is
//!   no scalar expression to `DESCRIBE`. That covers the 12 relation verbs
//!   (`SigTy::Rows`, true by construction), `seq.count` (`Integer` over
//!   `COUNT(*)`) and the three `io/` constructors (`SigTy::Rows`). Their return
//!   types are asserted by reading, not by an engine.
//! - **That the type is the RIGHT one.** It proves `sig.ret` and the template
//!   agree, not that either is what the language should say. `parse.decimal`
//!   returning `Float` is a compromise recorded in its own row; this test would
//!   be just as green if the pair had settled on the wrong answer together.
//! - **VALUES.** `parse.json` changed from `json_extract` to
//!   `json_extract_string`, which alters the string as well as its type
//!   (`"x"` → `x`); nothing here would have noticed if only the type had moved.
//!   `builtin_smoke.rs` is where a value is asserted.
//! - **`DataFusion`.** `DuckDB` is the engine every template was measured against
//!   and the only one that can render all 35. What the other engine does with a
//!   row is `fossil-df`'s `UNREACHABLE_ON_DATAFUSION`, and it answers a
//!   different question: *does it render*, not *what does it return*.

#![cfg(not(target_arch = "wasm32"))]

use duckdb::Connection;
use fossil_hir::stdlib::{FunctionRegistry, LoweringKind, ScalarTy, SigTy, render_template};

/// The `DuckDB` type name a [`ScalarTy`] means — what `DESCRIBE` must print.
///
/// This is the catalogue's scalar lattice read as SQL, and it is deliberately
/// EXACT rather than a family: `Float` is `DOUBLE` and not "any numeric", so a
/// row that drifts to `DECIMAL(38,18)` or `FLOAT` is a failure and not a near
/// miss. Every one of the five defects this test was written for was a near
/// miss.
const fn sql_type_of(ty: ScalarTy) -> &'static str {
    match ty {
        ScalarTy::String => "VARCHAR",
        ScalarTy::Integer => "BIGINT",
        ScalarTy::Float => "DOUBLE",
        ScalarTy::Bool => "BOOLEAN",
        ScalarTy::Date => "DATE",
        ScalarTy::DateTime => "TIMESTAMP",
        ScalarTy::SeqString => "VARCHAR[]",
    }
}

/// A literal of exactly the SQL type the parameter declares.
///
/// Every one is wrapped in a `CAST` to the same name [`sql_type_of`] gives,
/// including the ones where it looks redundant — see the module header for the
/// `1.5` / `DECIMAL(2,1)` trap. The values are meaningless on purpose: a
/// per-row valid argument would make this a test of the arguments, and
/// `DESCRIBE` never looks at one.
fn dummy(ty: ScalarTy) -> String {
    let literal = match ty {
        ScalarTy::String => "'abc'",
        ScalarTy::Integer => "2",
        ScalarTy::Float => "1.5",
        ScalarTy::Bool => "true",
        ScalarTy::Date => "'2026-05-21'",
        ScalarTy::DateTime => "'2026-05-21 00:00:00'",
        ScalarTy::SeqString => "['a', 'b']",
    };
    format!("CAST({literal} AS {})", sql_type_of(ty))
}

#[test]
fn every_expr_template_returns_the_type_its_row_declares() {
    let conn = Connection::open_in_memory().expect("open in-memory DuckDB");
    let reg = FunctionRegistry::stdlib_default();

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for entry in reg.iter() {
        let LoweringKind::Expr(template) = &entry.lowering else {
            // An `Op` row names an operator of the algebra. There is no scalar
            // expression to describe; see the module header.
            continue;
        };

        // An `Expr` row is a scalar function: it takes scalars and gives one
        // back. Both `expect`s are catalogue bugs rather than test conditions —
        // `Rows` and `Predicate` belong to the verbs, and those are `Op` rows.
        let args: Vec<String> = entry
            .sig
            .params
            .iter()
            .map(|p| dummy(p.ty.scalar().expect("an `Expr` row takes scalars")))
            .collect();
        let declared = entry
            .sig
            .ret
            .scalar()
            .expect("an `Expr` row returns a scalar");

        let sql = format!(
            "DESCRIBE SELECT {} AS v",
            render_template(template.as_str(), &args)
        );
        let actual: String = conn
            .prepare(&sql)
            .and_then(|mut s| s.query_row([], |r| r.get::<_, String>(1)))
            .unwrap_or_else(|e| panic!("`{}` could not be described: {e}\n{sql}", entry.name));

        checked += 1;
        let expected = sql_type_of(declared);
        if actual != expected {
            mismatches.push(format!(
                "`{}`\n    sig.ret: {declared:?} ({expected})\n    engine:  {actual}\n    sql: {sql}",
                entry.name
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} row(s) declare a return type their template does not produce. \
         A program calling one type-checks and writes something else:\n  {}",
        mismatches.len(),
        mismatches.join("\n  "),
    );
    // An all-`Op` catalogue would pass the assertion above with an empty loop,
    // so the count has to be checked — but against the catalogue rather than
    // against a literal. This was `35`, and it went stale the first time a row
    // was deleted; what the test needs to know is that EVERY `Expr` row reached
    // a real DuckDB, which is a claim about two numbers agreeing.
    let expr_rows = FunctionRegistry::stdlib_default()
        .iter()
        .filter(|e| matches!(e.lowering, LoweringKind::Expr(_)))
        .count();
    assert_eq!(
        checked, expr_rows,
        "every `Expr` row must be typed by the engine; {checked} of {expr_rows} were"
    );
    assert!(
        checked > 20,
        "only {checked} template(s) were typed; a loop over almost nothing proves \
         almost nothing"
    );
}

/// The 16 rows the test above cannot reach, pinned by reading.
///
/// It is the weaker half and it is written down as such: no engine is consulted,
/// so this asserts the SHAPE of a `PlanOp` row's return and nothing about what
/// the operator does. What it does buy is that the `io/` rows cannot go back to
/// declaring `String` — which is what they declared, unread, because the helper
/// that inserted them took a `ScalarTy` and the one that did not was called
/// `add_verb`.
#[test]
fn every_op_row_returns_rows_except_the_one_that_counts_them() {
    let reg = FunctionRegistry::stdlib_default();

    let mut op_rows: Vec<(&str, SigTy)> = reg
        .iter()
        .filter(|e| matches!(e.lowering, LoweringKind::Op(_)))
        .map(|e| (e.name.as_str(), e.sig.ret))
        .collect();
    op_rows.sort_unstable_by_key(|(name, _)| *name);

    let scalar_returning: Vec<&str> = op_rows
        .iter()
        .filter(|(_, ret)| ret.scalar().is_some())
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        scalar_returning,
        ["seq.count"],
        "an operator of the algebra gives back a relation; `seq.count` is the \
         one that gives back a number, and any other is a row that says it \
         produces a value where the plan produces rows"
    );
    assert_eq!(
        reg.lookup("seq.count").map(|e| e.sig.ret),
        Some(SigTy::Scalar(ScalarTy::Integer)),
        "`COUNT(*)` is a BIGINT"
    );
    // Every `seq/` row plus every `io.` constructor that reads data — derived,
    // because this was the literal `15` written as "12 `seq/` rows and 3 `io/`
    // constructors" and both halves have since moved: `io.rdf` gained a
    // signature when the registry's hand-written `io.` half was folded into
    // `catalogue.bnf`, making it four.
    let seq = op_rows
        .iter()
        .filter(|(n, _)| n.starts_with("seq."))
        .count();
    let io = op_rows.iter().filter(|(n, _)| n.starts_with("io.")).count();
    assert_eq!(
        op_rows.len(),
        seq + io,
        "an operator row is a `seq/` verb or an `io.` constructor and nothing else; \
         got {op_rows:?}"
    );
    assert!(
        seq >= 12 && io >= 4,
        "expected every relation verb and every data constructor, got {seq} `seq/` \
         and {io} `io/`"
    );
}
