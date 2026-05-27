//! Phase-5 registry tests (Task 1 smoke + Task 2 completeness/invariant).

use super::*;
use std::sync::Arc;

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

#[test]
fn back_compat_phase1_entries_present() {
    let r = FunctionRegistry::stdlib_default();
    // The historical four entries must survive (walking-skeleton / lower.rs).
    assert!(r.lookup("io.csv").is_some());
    assert!(r.lookup("core.iri").is_some());
    assert!(r.lookup("core.triple").is_some());
    assert!(r.lookup("core.literal").is_some());
    assert!(r.lookup("nonexistent").is_none());
    // phase1_default is now a thin alias for stdlib_default.
    assert!(
        FunctionRegistry::phase1_default()
            .lookup("io.csv")
            .is_some()
    );
}

#[test]
fn signature_materializes_real_fn_sig() {
    let db = db();
    let r = FunctionRegistry::stdlib_default();
    let trim = r.lookup("clean.trim").expect("clean.trim present");
    let sig = trim.signature(&db);
    // clean.trim :: String -> String.
    assert_eq!(sig.params(&db).len(), 1);
    let want = fossil_hir::ty::Ty::new(
        &db,
        fossil_hir::ty::TyKind::Primitive(fossil_hir::ty::Primitive::String),
    );
    assert_eq!(sig.return_ty(&db), want);
    assert_eq!(sig.params(&db)[0], want);
}

#[test]
fn anon_redact_is_inline_literal_pure_sql() {
    let r = FunctionRegistry::stdlib_default();
    let redact = r.lookup("anon.redact").expect("anon.redact present");
    assert_eq!(redact.wasm_class, WasmClass::PureSql);
    match &redact.lowering {
        LoweringKind::Inline(InlineForm::LiteralStr { value }) => {
            assert_eq!(value.as_str(), "[REDACTED]");
        }
        other => panic!("anon.redact must be Inline(LiteralStr), got {other:?}"),
    }
}

#[test]
fn validate_regex_is_builtin_regexp_matches_pure_sql() {
    let r = FunctionRegistry::stdlib_default();
    let regex = r.lookup("validate.regex").expect("validate.regex present");
    assert_eq!(regex.wasm_class, WasmClass::PureSql);
    match &regex.lowering {
        LoweringKind::Builtin { duckdb_name } => {
            assert_eq!(duckdb_name.as_str(), "regexp_matches");
        }
        other => panic!("validate.regex must be Builtin, got {other:?}"),
    }
}

#[test]
fn math_namespace_has_exactly_six_no_ceil_floor() {
    let r = FunctionRegistry::stdlib_default();
    let math: Vec<&str> = r
        .iter()
        .map(|en| en.name.as_str())
        .filter(|n| n.starts_with("math."))
        .collect();
    assert_eq!(
        math.len(),
        6,
        "math/ must have exactly 6 functions, got {math:?}"
    );
    assert!(r.lookup("math.ceil").is_none(), "math.ceil must NOT exist");
    assert!(
        r.lookup("math.floor").is_none(),
        "math.floor must NOT exist"
    );
}

/// The authoritative function set from `stdlib.md`, eight surface namespaces.
/// `io/` is intentionally excluded (source constructors; `io.sql`/`io.http`
/// out of scope this milestone).
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

#[test]
fn catalog_is_bidirectionally_complete_against_stdlib_md() {
    use std::collections::BTreeSet;

    let r = FunctionRegistry::stdlib_default();
    // The catalog set, excluding the io/ source constructors (not surface fns).
    let catalog: BTreeSet<String> = r
        .iter()
        .map(|en| en.name.to_string())
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
        "catalog must equal the stdlib.md eight-namespace set exactly.\n  MISSING (omissions): {missing:?}\n  EXTRA (not in stdlib.md): {extra:?}"
    );
    // Sanity on the count: 8 + 13 + 6 + 7 + 6 + 8 + 5 + 3 = 56 surface functions.
    assert_eq!(catalog.len(), 56);
    // The three io/ constructors are retained on top.
    assert_eq!(r.iter().count(), 56 + 3);
}

#[test]
fn pure_sql_iff_non_udf_invariant_holds_for_every_entry() {
    let r = FunctionRegistry::stdlib_default();
    for en in r.iter() {
        assert_eq!(
            en.wasm_class,
            derive_wasm_class(&en.lowering),
            "entry {} violates the PureSql ⟺ non-Udf invariant",
            en.name
        );
        // Equivalent direct statement of the invariant.
        let is_udf = matches!(en.lowering, LoweringKind::Udf { .. });
        assert_eq!(
            en.wasm_class == WasmClass::NativeUdfOnly,
            is_udf,
            "entry {} : NativeUdfOnly must hold iff lowering is Udf",
            en.name
        );
    }
}

#[test]
fn no_builtin_has_an_empty_duckdb_name() {
    let r = FunctionRegistry::stdlib_default();
    for en in r.iter() {
        if let LoweringKind::Builtin { duckdb_name } = &en.lowering {
            assert!(
                !duckdb_name.is_empty(),
                "entry {} has an empty Builtin duckdb_name",
                en.name
            );
        }
    }
}
