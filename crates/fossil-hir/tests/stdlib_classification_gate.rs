//! SC#1 classification gate (STDL-07): `pure_sql` ⟺ DuckDB-builtin.
//!
//! This is the CI enforcement of Success Criterion #1: a `pure_sql`-tagged
//! function MUST NOT compile to anything other than a `DuckDB` builtin
//! (allowlist), an inline SQL form, or a plan op. Concretely it asserts, over
//! the FULL `FunctionRegistry::stdlib_default()` catalog:
//!
//! 1. **Structural invariant** — `WasmClass::PureSql ⟺ lowering ∈ {Builtin,
//!    Inline, Plan}` (equivalently `NativeUdfOnly ⟺ lowering is Udf`). This is
//!    the SC#1 structural half; it re-derives the invariant from the *public*
//!    `lowering`/`wasm_class` fields (the private `derive_wasm_class` is the
//!    catalog's own enforcement; this gate is an independent check).
//! 2. **Allowlist membership** — every `Builtin.duckdb_name` is a member of the
//!    curated [`DUCKDB_BUILTIN_ALLOWLIST`]. A typo'd or non-`DuckDB` builtin name
//!    fails CI here (the native `fossil-runtime/tests/builtin_smoke.rs` then
//!    proves each allowlisted name is a real `DuckDB` 1.10502 function).
//! 3. **Completeness** — every function across the eight `stdlib.md` namespaces
//!    is present exactly once (no untagged function): the canonical
//!    untagged-function guard. `math/` = 6 (no `ceil`/`floor`); `anon.redact`
//!    and `validate.regex` present.

use fossil_hir::stdlib::{DUCKDB_BUILTIN_ALLOWLIST, FunctionRegistry, LoweringKind, WasmClass};

/// SC#1 structural half: `PureSql ⟺ lowering ∈ {Builtin, Inline, Plan}`, and
/// every `Builtin.duckdb_name` is in the curated `DuckDB`-builtin allowlist.
#[test]
fn pure_sql_iff_duckdb_builtin_or_inline_or_plan_with_allowlist() {
    let reg = FunctionRegistry::stdlib_default();
    for entry in reg.iter() {
        let is_udf = matches!(entry.lowering, LoweringKind::Udf { .. });

        // (a) PureSql ⟺ NOT Udf  (i.e. PureSql ⟺ Builtin|Inline|Plan).
        assert_eq!(
            entry.wasm_class == WasmClass::PureSql,
            !is_udf,
            "SC#1 invariant violated: {} is wasm_class {:?} but lowering {:?}",
            entry.name,
            entry.wasm_class,
            entry.lowering,
        );
        assert_eq!(
            entry.wasm_class == WasmClass::NativeUdfOnly,
            is_udf,
            "SC#1 invariant violated: {} NativeUdfOnly must hold iff lowering is Udf",
            entry.name,
        );

        // (b) every pure_sql Builtin compiles to an allowlisted DuckDB builtin.
        if let LoweringKind::Builtin { duckdb_name } = &entry.lowering {
            assert_eq!(
                entry.wasm_class,
                WasmClass::PureSql,
                "{} is a Builtin yet not PureSql",
                entry.name,
            );
            assert!(
                DUCKDB_BUILTIN_ALLOWLIST.contains(&duckdb_name.as_str()),
                "SC#1 CI gate: {} compiles to DuckDB builtin `{}`, which is NOT in \
                 DUCKDB_BUILTIN_ALLOWLIST — a pure_sql function must lower to a known \
                 DuckDB builtin (add it to the allowlist only if it is a real, \
                 stdlib.md-authorised builtin)",
                entry.name,
                duckdb_name,
            );
        }
    }
}

/// The allowlist must not drift from the catalog: every allowlisted name is
/// actually used by some `Builtin` entry (no stray names), and conversely every
/// `Builtin.duckdb_name` is allowlisted (covered above). Aligns the allowlist to
/// the authoritative `stdlib.md` set — in particular it must carry NO `ceil`/`floor`.
#[test]
fn allowlist_is_aligned_to_catalog_no_ceil_floor() {
    use std::collections::BTreeSet;

    let reg = FunctionRegistry::stdlib_default();
    let used: BTreeSet<&str> = reg
        .iter()
        .filter_map(|e| match &e.lowering {
            LoweringKind::Builtin { duckdb_name } => Some(duckdb_name.as_str()),
            _ => None,
        })
        .collect();
    let allow: BTreeSet<&str> = DUCKDB_BUILTIN_ALLOWLIST.iter().copied().collect();

    // No stray allowlist entry with no catalog user (keeps the allowlist aligned
    // to stdlib.md and avoids drift).
    let stray: Vec<&&str> = allow.difference(&used).collect();
    assert!(
        stray.is_empty(),
        "DUCKDB_BUILTIN_ALLOWLIST has names with no Builtin catalog user (drift): {stray:?}",
    );

    // No ceil/floor — stdlib.md math/ = 6 (sum, avg, min, max, abs, round).
    assert!(
        !allow.contains("ceil"),
        "allowlist must NOT carry ceil (stdlib.md math/ has no ceil)",
    );
    assert!(
        !allow.contains("floor"),
        "allowlist must NOT carry floor (stdlib.md math/ has no floor)",
    );
}

/// The authoritative function set from `stdlib.md`, eight surface namespaces.
/// `io/` is intentionally excluded (source constructors; `io.sql`/`io.http`
/// out of scope this milestone). Mirrors `fossil-hir`'s own completeness
/// test — the canonical untagged-function guard, asserted here as part of the
/// SC#1 CI gate.
fn expected_stdlib_names() -> Vec<&'static str> {
    vec![
        // core/ (8)
        "core.iri",
        "core.triple",
        "core.blank",
        "core.literal",
        "core.typed",
        "core.lang",
        "core.emit",
        "core.require",
        // seq/ (13)
        "seq.filter",
        "seq.map",
        "seq.flatten",
        "seq.take",
        "seq.drop",
        "seq.distinct",
        "seq.sort",
        "seq.project",
        "seq.join",
        "seq.union",
        "seq.group_by",
        "seq.aggregate",
        "seq.count",
        // clean/ (6)
        "clean.trim",
        "clean.lower",
        "clean.upper",
        "clean.slug",
        "clean.normalize_unicode",
        "clean.strip_html",
        // parse/ (7)
        "parse.integer",
        "parse.float",
        "parse.decimal",
        "parse.date",
        "parse.datetime",
        "parse.json",
        "parse.csv_row",
        // math/ (6 — NO ceil/floor)
        "math.sum",
        "math.avg",
        "math.min",
        "math.max",
        "math.abs",
        "math.round",
        // str/ (8)
        "str.length",
        "str.slice",
        "str.contains",
        "str.starts_with",
        "str.ends_with",
        "str.replace",
        "str.split",
        "str.concat",
        // validate/ (5)
        "validate.email",
        "validate.url",
        "validate.uuid",
        "validate.iso_date",
        "validate.regex",
        // anon/ (3)
        "anon.hash",
        "anon.hmac",
        "anon.redact",
    ]
}

/// Completeness / untagged-function guard: every stdlib function from the eight
/// namespaces is present exactly once. Every entry carries a `wasm_class` tag by
/// construction (the type has no untagged state), so "present exactly once" is
/// the operative untagged guard. math/ = 6; anon.redact + validate.regex present.
#[test]
fn every_stdlib_function_is_tagged_exactly_once() {
    use std::collections::BTreeSet;

    let reg = FunctionRegistry::stdlib_default();
    let catalog: BTreeSet<String> = reg
        .iter()
        .map(|e| e.name.to_string())
        .filter(|n| !n.starts_with("io."))
        .collect();
    let expected: BTreeSet<String> = expected_stdlib_names()
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let missing: Vec<&String> = expected.difference(&catalog).collect();
    let extra: Vec<&String> = catalog.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "stdlib catalog must equal the stdlib.md eight-namespace set exactly.\n  \
         MISSING (untagged/omitted): {missing:?}\n  EXTRA (not in stdlib.md): {extra:?}",
    );
    assert_eq!(catalog.len(), 56, "8+13+6+7+6+8+5+3 = 56 surface functions");

    // math/ = exactly 6, no ceil/floor.
    let math_count = catalog.iter().filter(|n| n.starts_with("math.")).count();
    assert_eq!(math_count, 6, "math/ must have exactly 6 functions");
    assert!(!catalog.contains("math.ceil"));
    assert!(!catalog.contains("math.floor"));

    // anon.redact + validate.regex present (PureSql forms).
    assert!(catalog.contains("anon.redact"));
    assert!(catalog.contains("validate.regex"));
}
