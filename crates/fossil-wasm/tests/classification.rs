//! STDL-07: the `FossilPlayground` stdlib classification manifest.
//!
//! Native integration test (the `[lib] test = false` setting disables the
//! in-lib unit harness; integration tests run on the native target). Exercises
//! the same projection `FossilPlayground::classification` serializes — the
//! pure-data half via `stdlib_classification()` — and asserts the `pure_sql`
//! vs `native_udf_only` tags plus a serde round-trip.

use std::collections::BTreeMap;

use fossil_wasm::{FnClassification, stdlib_classification};

/// The manifest covers every stdlib function the registry knows, with each
/// `native_udf_only` function tagged so and each pure-SQL function tagged
/// `pure_sql`.
#[test]
fn manifest_covers_stdlib_with_correct_tags() {
    let manifest = stdlib_classification();
    let by_name: BTreeMap<&str, &str> = manifest
        .iter()
        .map(|c| (c.name.as_str(), c.wasm_class.as_str()))
        .collect();

    // Same count as the registry (56 surface fns + 3 io/ sources = 59).
    let registry = fossil_registry::FunctionRegistry::stdlib_default();
    assert_eq!(manifest.len(), registry.iter().count());
    assert_eq!(by_name.len(), manifest.len(), "no duplicate names");

    // native_udf_only functions — exactly the eight UDF entries (must match the
    // fossil-runtime register_stdlib_udfs set).
    for udf in [
        "clean.slug",
        "clean.normalize_unicode",
        "clean.strip_html",
        "validate.email",
        "validate.url",
        "validate.uuid",
        "validate.iso_date",
        "anon.hmac",
    ] {
        assert_eq!(by_name.get(udf), Some(&"native_udf_only"), "{udf}");
    }

    // pure_sql functions — a representative cross-section of Builtin/Inline/Plan.
    for pure in [
        "clean.trim",
        "clean.lower",
        "anon.hash",
        "anon.redact",
        "validate.regex",
        "str.length",
        "math.sum",
        "parse.integer",
        "seq.filter",
        "io.csv",
    ] {
        assert_eq!(by_name.get(pure), Some(&"pure_sql"), "{pure}");
    }
}

/// Every entry's `wasm_class` is one of the two known string tags — nothing
/// leaks an unexpected value to the playground.
#[test]
fn every_tag_is_one_of_the_two_known_strings() {
    for c in stdlib_classification() {
        assert!(
            c.wasm_class == "pure_sql" || c.wasm_class == "native_udf_only",
            "unexpected wasm_class {:?} for {}",
            c.wasm_class,
            c.name
        );
    }
}

/// The manifest is serde-serializable (the shape `FossilPlayground::classification`
/// hands to `serde_wasm_bindgen`) and round-trips through JSON unchanged.
#[test]
fn manifest_serde_round_trips() {
    let manifest = stdlib_classification();
    let json = serde_json::to_string(&manifest).expect("serialize manifest");
    assert!(json.contains("\"clean.slug\""));
    assert!(json.contains("\"native_udf_only\""));
    assert!(json.contains("\"pure_sql\""));

    let back: Vec<FnClassification> = serde_json::from_str(&json).expect("deserialize manifest");
    assert_eq!(back, manifest);
}
