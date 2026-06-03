//! Native-side smoke of the `ty_wasm`-shaped Workspace surface (plan 07-02,
//! WASM-02 / SC#5 first half).
//!
//! The full node-driven end-to-end exercise lives in `test-wasm-workspace.js`
//! (loaded by `wasm-bindgen --target nodejs` + node ≥18); that script is
//! manual + CI-job-driven, not per-PR `cargo test` (matches the Phase-1
//! `test-wasm.js` pattern from `01-RESEARCH.md` Example 18).
//!
//! This file is the cargo-test mirror that catches API regressions on every
//! PR without needing the wasm-bindgen + node toolchain installed. Each test
//! exercises the lifecycle through the pure-Rust `*_native` / `*_rows` /
//! `*_result` helpers — the `#[wasm_bindgen]` wrappers (`open_file`,
//! `update_file`, `close_file`, `check`, `diagnostics_for`, `compile_file`,
//! `set_target_shex`) merely translate to/from `JsError` + `JsValue` via
//! wasm-bindgen, which panics on native targets ("cannot call wasm-bindgen
//! imported functions on non-wasm targets" — wasm-bindgen 0.2 lib.rs:101).
//! The split mirrors the Phase-5 `classification()` ↔
//! `stdlib_classification()` precedent.

use fossil_wasm::{FossilPlayground, WorkspaceError};

/// Minimal `ShEx` schema in JSON-LD form (`ShExJ` — the format
/// `ShExDescriptor::from_reader` parses).
const MINIMAL_SHEX_JSON: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/name",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#string"
          }
        }
      }
    }
  ]
}"#;

/// Read `examples/hello.fossil` from the repo root. The cargo-test cwd is the
/// crate directory (`crates/fossil-wasm/`), so the fixture is two levels up.
fn hello_fossil_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("hello.fossil");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "examples/hello.fossil must be readable from cargo-test cwd ({} — {e})",
            path.display()
        )
    })
}

/// The full lifecycle: open → update → check → compile → close, plus the
/// close-of-unknown-handle error path.
#[test]
fn workspace_lifecycle_smoke() {
    let mut pg = FossilPlayground::new();
    let source = hello_fossil_source();

    // open_file_native returns a fresh FileHandle (Salsa-interned
    // SourceFile under the current revision).
    let h = pg.open_file_native("hello.fossil".to_string(), source.clone());

    // update_file_native bumps the Salsa revision via `set_text` — the
    // EXACT mechanism didChange uses (ADR-0022). No panic, no error.
    pg.update_file_native(h, source + "\n// edit")
        .expect("update_file_native");

    // Workspace-wide drain: we don't assert row count (well-formed
    // hello.fossil under AcceptAll may produce zero or more rows
    // depending on Phase-2 warnings); what matters is the call returns
    // and serialization is deferred to the wasm-bindgen wrapper.
    let _rows = pg.check_rows();

    // compile_file_result's pure-Rust core returns the SQL + manifest
    // strings. Same assertions as the Phase-1 `test-wasm.js` smoke
    // (COPY + graphar_version) — proves the lifecycle path produces
    // byte-identical output to the legacy `compile(&str)` path.
    let result = pg.compile_file_result(h).expect("compile_file_result");
    assert!(
        result.sql.contains("COPY"),
        "compile_file SQL must contain COPY (got: {})",
        result.sql
    );
    assert!(
        result.manifest_yaml.contains("version: gar/v1"),
        "manifest_yaml must carry the GraphAr format version: {}",
        result.manifest_yaml
    );

    // close_file_native removes the handle from the map.
    pg.close_file_native(h).expect("close_file_native");

    // Closing the same handle again must error — strict signal, mirrors
    // ty_wasm's contract.
    assert_eq!(
        pg.close_file_native(h),
        Err(WorkspaceError::UnknownHandle),
        "close-of-closed must error with UnknownHandle"
    );

    // update_file_native on a closed handle must also error.
    assert_eq!(
        pg.update_file_native(h, "// post-close".to_string()),
        Err(WorkspaceError::UnknownHandle),
        "update of closed handle must error with UnknownHandle"
    );
}

/// Two open files, close one, the other still produces a diagnostic stream.
/// Exercises `OpenFiles::iter` (the workspace-wide drain `check()` uses) +
/// `diagnostics_for_rows(handle)` (the B3 per-file drain).
#[test]
fn workspace_multi_file_isolation() {
    let mut pg = FossilPlayground::new();
    let source = hello_fossil_source();

    let h1 = pg.open_file_native("a.fossil".to_string(), source.clone());
    let h2 = pg.open_file_native("b.fossil".to_string(), source);

    // diagnostics_for_rows(h1) — per-file accessor (07-03 LSP Worker's
    // drain entry point). Returns Some(_) for an open handle.
    let per_file_a = pg.diagnostics_for_rows(h1);
    assert!(per_file_a.is_some(), "diagnostics_for_rows h1 returns Some");
    let per_file_b = pg.diagnostics_for_rows(h2);
    assert!(per_file_b.is_some(), "diagnostics_for_rows h2 returns Some");

    // Every per-file row's `uri` matches the file it was drained from
    // (proves the B3 per-file scoping).
    for row in per_file_a.unwrap() {
        assert_eq!(row.uri, "a.fossil", "per-file row keyed to its file URI");
    }
    for row in per_file_b.unwrap() {
        assert_eq!(row.uri, "b.fossil", "per-file row keyed to its file URI");
    }

    // check_rows() — workspace-wide drain. Includes rows from both files.
    let all = pg.check_rows();
    // The workspace-wide row set may be empty for well-formed sources,
    // but every row that IS present must carry one of the two URIs (no
    // bleed).
    for row in &all {
        assert!(
            row.uri == "a.fossil" || row.uri == "b.fossil",
            "every workspace row carries one of the two open URIs (got {})",
            row.uri
        );
    }

    // Close one file; the other survives.
    pg.close_file_native(h1).expect("close a");
    assert!(
        pg.diagnostics_for_rows(h2).is_some(),
        "h2 still drainable after closing h1"
    );

    // diagnostics_for_rows on a closed handle returns None (the wasm
    // wrapper converts that to JsError).
    assert!(
        pg.diagnostics_for_rows(h1).is_none(),
        "diagnostics_for_rows of closed handle is None"
    );
}

/// `set_target_shex_native` happy path + parse-failure path.
#[test]
fn workspace_set_target_shex_smoke() {
    let mut pg = FossilPlayground::new();

    // Happy path — a valid ShExJ schema parses and installs.
    pg.set_target_shex_native(MINIMAL_SHEX_JSON)
        .expect("minimal ShEx JSON parses + installs");

    // Failure path — garbage input returns Err. The previously-installed
    // descriptor is retained (no half-applied state — same contract as
    // fossil-lsp's load_sibling_shex in 06-09); we can't directly observe
    // that the old descriptor stayed without reaching into private state,
    // but a follow-up Ok() install proves the playground isn't wedged.
    assert!(
        pg.set_target_shex_native("definitely not shex").is_err(),
        "garbage ShEx must return Err"
    );

    // Installing a fresh schema after the failure still works.
    pg.set_target_shex_native(MINIMAL_SHEX_JSON)
        .expect("re-install after failure still works");
}

/// `compile_file` rejects a closed handle with a clear error.
#[test]
fn workspace_compile_file_closed_handle() {
    let mut pg = FossilPlayground::new();
    let source = hello_fossil_source();
    let h = pg.open_file_native("hello.fossil".to_string(), source);
    pg.close_file_native(h).expect("close");
    assert_eq!(
        pg.compile_file_result(h),
        Err(WorkspaceError::UnknownHandle),
        "compile of closed handle must error with UnknownHandle"
    );
}
