//! Cargo-test mirror of `FossilWorkspace::register_inferred_descriptor`.
//!
//! Exercises the pure-Rust path via `register_inferred_descriptor_native`. The
//! `#[wasm_bindgen]`-attributed `register_inferred_descriptor` wrapper panics on
//! the native cargo-test target (wasm-bindgen 0.2 — "cannot call
//! wasm-bindgen imported functions on non-wasm targets"); the full
//! wasm-bindgen integration is exercised browser-side by the vitest suite at
//! `packages/wasm/tests/registerInferredDescriptor.test.ts`.
//!
//! Convention mirrors `bindings/editor/tests/workspace.rs`.

use fossil_graph_schema::Primitive;
use fossil_wasm::FossilWorkspace;

fn sample_descriptor_json(uri: &str) -> String {
    format!(
        r#"{{"key":"{uri}","columns":[{{"name":"id","primitive":"integer"}},{{"name":"name","primitive":"string"}}],"etag":""}}"#
    )
}

#[test]
fn register_inferred_descriptor_parses_and_stores() {
    let mut ws = FossilWorkspace::new();
    ws.register_inferred_descriptor_native(&sample_descriptor_json("users.csv"))
        .expect("valid JSON ok");
    let got = ws
        .inferred_descriptor_native("users.csv")
        .expect("registered descriptor present");
    assert_eq!(got.key.as_str(), "users.csv");
    assert_eq!(got.columns.len(), 2);
    assert_eq!(got.columns[0].name.as_str(), "id");
    assert_eq!(got.columns[0].primitive, Primitive::Integer);
    assert_eq!(got.columns[1].name.as_str(), "name");
    assert_eq!(got.columns[1].primitive, Primitive::STRING);
}

#[test]
fn register_inferred_descriptor_overwrites_on_duplicate_uri() {
    let mut ws = FossilWorkspace::new();
    ws.register_inferred_descriptor_native(&sample_descriptor_json("users.csv"))
        .expect("first ok");
    let second =
        r#"{"key":"users.csv","columns":[{"name":"id","primitive":"integer"}],"etag":"h2"}"#;
    ws.register_inferred_descriptor_native(second)
        .expect("second ok");
    let got = ws
        .inferred_descriptor_native("users.csv")
        .expect("present after re-register");
    assert_eq!(got.columns.len(), 1);
    assert_eq!(got.etag, "h2");
}

#[test]
fn register_inferred_descriptor_rejects_malformed_json() {
    let mut ws = FossilWorkspace::new();
    let failure = ws
        .register_inferred_descriptor_native("not json at all")
        .expect_err("malformed JSON should err");
    assert_eq!(failure.problem.code(), "api/invalid-argument");
    assert!(
        std::error::Error::source(&failure).is_some(),
        "the serde error is the cause"
    );
}

#[test]
fn register_inferred_descriptor_rejects_missing_required_fields() {
    let mut ws = FossilWorkspace::new();
    let result = ws.register_inferred_descriptor_native(r#"{"key":"users.csv"}"#);
    assert!(result.is_err(), "missing `columns` field should err");
}

#[test]
fn unknown_source_returns_none() {
    let mut ws = FossilWorkspace::new();
    ws.register_inferred_descriptor_native(&sample_descriptor_json("users.csv"))
        .expect("ok");
    assert!(ws.inferred_descriptor_native("nonexistent.csv").is_none());
}

#[test]
fn distinct_uris_register_independently() {
    let mut ws = FossilWorkspace::new();
    ws.register_inferred_descriptor_native(&sample_descriptor_json("users.csv"))
        .expect("users ok");
    ws.register_inferred_descriptor_native(
        r#"{"key":"products.csv","columns":[{"name":"sku","primitive":"string"}],"etag":""}"#,
    )
    .expect("products ok");
    assert!(ws.inferred_descriptor_native("users.csv").is_some());
    let products = ws
        .inferred_descriptor_native("products.csv")
        .expect("present");
    assert_eq!(products.key.as_str(), "products.csv");
    assert_eq!(products.columns[0].name.as_str(), "sku");
}

/// The stale check the descriptor table caused: a check that ran before the
/// host described a source kept refusing its columns afterwards, because the
/// table was read beside Salsa. It is an input now (`/docs/design/incremental`).
#[test]
fn describing_a_source_after_a_check_re_runs_the_check() {
    const PROGRAM: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.email
";
    let mut ws = FossilWorkspace::new();
    let _ = ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    ws.register_inferred_descriptor_native(&sample_descriptor_json("users.csv"))
        .expect("ok");
    let unknown_email = |ws: &FossilWorkspace| {
        ws.diagnostic_rows()
            .iter()
            .any(|r| r.code == "name/unknown-field")
    };
    assert!(unknown_email(&ws), "`email` is not a column yet");

    ws.register_inferred_descriptor_native(
        r#"{"key":"users.csv","columns":[{"name":"email","primitive":"string"}],"etag":""}"#,
    )
    .expect("ok");
    assert!(
        !unknown_email(&ws),
        "the check re-ran against the new columns"
    );
}
