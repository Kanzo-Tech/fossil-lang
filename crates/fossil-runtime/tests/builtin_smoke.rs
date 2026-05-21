//! SC#1 native-builtin smoke (STDL-07): every `pure_sql` Builtin name is real.
//!
//! NATIVE-ONLY (`fossil-runtime` carries a `wasm32` `compile_error!` tripwire).
//!
//! The structural half of the SC#1 gate lives in
//! `fossil-registry/tests/classification_gate.rs`: it proves every `pure_sql`
//! `Builtin.duckdb_name` is a member of the curated `DUCKDB_BUILTIN_ALLOWLIST`.
//! That allowlist is internally consistent, but a name could still be a typo or
//! a builtin that a future `DuckDB` renamed/removed. THIS test closes that gap
//! (RESEARCH Pitfall 4): for every `pure_sql` `Builtin` entry in
//! `FunctionRegistry::stdlib_default()` it executes a minimal
//! `SELECT <duckdb_name>(<dummy args>)` on a native bundled-DuckDB 1.10502
//! connection and asserts the call resolves (no "function does not exist"
//! error) — proving the name matches the real engine, on native, before it can
//! silently fail in the WASM playground.
//!
//! `native_udf_only` functions are deliberately excluded — their UDFs are
//! registered separately by `udf::register_stdlib_udfs` (STDL-03/05) and are not
//! `DuckDB` builtins.
//!
//! The test is catalog-driven (it enumerates `stdlib_default().iter()`), so it
//! cannot drift from the registry: a newly added `Builtin` entry whose
//! `duckdb_name` lacks an arg template here fails the explicit completeness
//! assertion below rather than being silently skipped.

#![cfg(not(target_arch = "wasm32"))]

use duckdb::Connection;
use fossil_registry::{FunctionRegistry, LoweringKind, WasmClass};

/// A representative `SELECT <duckdb_name>(<dummy args>)` for a given `DuckDB`
/// builtin name. The arg shapes match each builtin's real arity (most are unary
/// string/number; `strptime`/`substring`/`replace`/`string_split`/`contains`/
/// `starts_with`/`ends_with`/`regexp_matches`/`concat` have known multi-arg
/// shapes). Returns `None` for an unrecognised name so the caller fails loudly
/// (no silent skip).
fn smoke_sql(name: &str) -> Option<String> {
    let call = match name {
        // unary string
        "trim" => "trim('  x  ')",
        "lower" => "lower('XY')",
        "upper" => "upper('xy')",
        "length" => "length('abc')",
        "sha256" => "sha256('abc')",
        // (string, format) — date/datetime parsing
        "strptime" => "strptime('2020-01-02', '%Y-%m-%d')",
        // aggregates over a constant (valid in a bare SELECT in DuckDB)
        "sum" => "sum(1)",
        "avg" => "avg(1)",
        "min" => "min(1)",
        "max" => "max(1)",
        // scalar math
        "abs" => "abs(-3)",
        "round" => "round(2.5)",
        // (string, start[, length]) — 2-arg form
        "substring" => "substring('abcdef', 2)",
        // (string, string) predicates / split
        "contains" => "contains('abc', 'b')",
        "starts_with" => "starts_with('abc', 'a')",
        "ends_with" => "ends_with('abc', 'c')",
        "string_split" => "string_split('a,b,c', ',')",
        "regexp_matches" => "regexp_matches('abc', 'a.c')",
        // (string, string, string)
        "replace" => "replace('abc', 'b', 'X')",
        // variadic concat
        "concat" => "concat('a', 'b', 'c')",
        _ => return None,
    };
    Some(format!("SELECT {call}"))
}

#[test]
fn every_pure_sql_builtin_resolves_on_native_duckdb() {
    let conn = Connection::open_in_memory().expect("open in-memory DuckDB");
    let reg = FunctionRegistry::stdlib_default();

    let mut tested = 0usize;
    for entry in reg.iter() {
        let LoweringKind::Builtin { duckdb_name } = &entry.lowering else {
            continue;
        };
        // Sanity: the structural gate guarantees this, but assert it here too so
        // the smoke set is exactly the pure_sql Builtin set.
        assert_eq!(
            entry.wasm_class,
            WasmClass::PureSql,
            "{} is a Builtin but not PureSql",
            entry.name,
        );

        let sql = smoke_sql(duckdb_name).unwrap_or_else(|| {
            panic!(
                "no smoke arg template for Builtin `{}` (used by {}); add one so the \
                 builtin is exercised — do NOT skip silently",
                duckdb_name, entry.name,
            )
        });

        // PREPARE + EXECUTE the call. A renamed/removed builtin surfaces as a
        // "function ... does not exist" / catalog error here.
        let mut stmt = conn.prepare(&sql).unwrap_or_else(|err| {
            panic!(
                "DuckDB builtin `{}` (used by {}) failed to PREPARE `{}`: {} — \
                 the name is not a real DuckDB 1.10502 builtin (typo/renamed/removed)",
                duckdb_name, entry.name, sql, err,
            )
        });
        // Drain the single result row to force execution.
        let mut rows = stmt.query([]).unwrap_or_else(|err| {
            panic!(
                "DuckDB builtin `{}` (used by {}) failed to EXECUTE `{}`: {}",
                duckdb_name, entry.name, sql, err,
            )
        });
        let got = rows
            .next()
            .unwrap_or_else(|err| panic!("`{sql}` row fetch failed: {err}"));
        assert!(got.is_some(), "`{sql}` returned no row");

        tested += 1;
    }

    // The catalog has 20 distinct pure_sql Builtin entries (clean 3, parse 2,
    // math 6, str 8, validate 1, anon 1 = 21 entries; parse.date+parse.datetime
    // both use strptime). Assert we exercised every Builtin entry — a non-zero,
    // catalog-derived count so the loop can never be silently empty.
    let builtin_count = reg
        .iter()
        .filter(|e| matches!(e.lowering, LoweringKind::Builtin { .. }))
        .count();
    assert_eq!(
        tested, builtin_count,
        "every pure_sql Builtin entry must be smoke-tested (no silent skip)",
    );
    assert!(builtin_count >= 20, "expected at least 20 Builtin entries");
}
