//! Cargo-test mirror of `FossilPlayground::register_inferred_descriptor` (13-03).
//!
//! Exercises the pure-Rust path via `register_inferred_descriptor_native`. The
//! `#[wasm_bindgen]`-attributed `register_inferred_descriptor` wrapper panics on
//! the native cargo-test target (wasm-bindgen 0.2 lib.rs:101 — "cannot call
//! wasm-bindgen imported functions on non-wasm targets"); the full
//! wasm-bindgen integration is exercised browser-side by the vitest suite at
//! `packages/wasm/tests/registerInferredDescriptor.test.ts`.
//!
//! Convention mirrors `crates/fossil-wasm/tests/workspace.rs` (Phase 7) and
//! `crates/fossil-wasm/tests/classification.rs` (Phase 5).

use fossil_graph_schema::Primitive;
use fossil_wasm::FossilPlayground;

fn sample_descriptor_json(source_name: &str) -> String {
    format!(
        r#"{{"source_name":"{source_name}","columns":[{{"name":"id","primitive":"integer"}},{{"name":"name","primitive":"string"}}],"content_hash":""}}"#
    )
}

#[test]
fn register_inferred_descriptor_parses_and_stores() {
    let pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(&sample_descriptor_json("users"))
        .expect("valid JSON ok");
    let got = pg
        .inferred_descriptor_native("users")
        .expect("registered descriptor present");
    assert_eq!(got.source_name.as_str(), "users");
    assert_eq!(got.columns.len(), 2);
    assert_eq!(got.columns[0].name.as_str(), "id");
    assert_eq!(got.columns[0].primitive, Primitive::Integer);
    assert_eq!(got.columns[1].name.as_str(), "name");
    assert_eq!(got.columns[1].primitive, Primitive::String);
}

#[test]
fn register_inferred_descriptor_overwrites_on_duplicate_source_name() {
    let pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(&sample_descriptor_json("users"))
        .expect("first ok");
    let second = r#"{"source_name":"users","columns":[{"name":"id","primitive":"integer"}],"content_hash":"h2"}"#;
    pg.register_inferred_descriptor_native(second)
        .expect("second ok");
    let got = pg
        .inferred_descriptor_native("users")
        .expect("present after re-register");
    assert_eq!(got.columns.len(), 1);
    assert_eq!(got.content_hash, "h2");
}

#[test]
fn register_inferred_descriptor_rejects_malformed_json() {
    let pg = FossilPlayground::new();
    let result = pg.register_inferred_descriptor_native("not json at all");
    assert!(result.is_err());
}

#[test]
fn register_inferred_descriptor_rejects_missing_required_fields() {
    let pg = FossilPlayground::new();
    let result = pg.register_inferred_descriptor_native(r#"{"source_name":"users"}"#);
    assert!(result.is_err(), "missing `columns` field should err");
}

#[test]
fn unknown_source_returns_none() {
    let pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(&sample_descriptor_json("users"))
        .expect("ok");
    assert!(pg.inferred_descriptor_native("nonexistent").is_none());
}

#[test]
fn distinct_sources_register_independently() {
    let pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(&sample_descriptor_json("users"))
        .expect("users ok");
    pg.register_inferred_descriptor_native(
        r#"{"source_name":"products","columns":[{"name":"sku","primitive":"string"}],"content_hash":""}"#,
    )
    .expect("products ok");
    assert!(pg.inferred_descriptor_native("users").is_some());
    let products = pg.inferred_descriptor_native("products").expect("present");
    assert_eq!(products.source_name.as_str(), "products");
    assert_eq!(products.columns[0].name.as_str(), "sku");
}
