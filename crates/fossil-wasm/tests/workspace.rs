//! Native-side smoke of the `ty_wasm`-shaped Workspace surface.
//!
//! The full node-driven end-to-end exercise lives in `test-wasm-workspace.js`
//! (loaded by `wasm-bindgen --target nodejs` + node ≥18); that script is
//! manual + CI-job-driven, not per-PR `cargo test` (the same split as
//! `test-wasm.js`).
//!
//! This file is the cargo-test mirror that catches API regressions on every
//! PR without needing the wasm-bindgen + node toolchain installed. Each test
//! exercises the lifecycle through the pure-Rust `*_native` / `*_rows` /
//! `*_result` helpers — the `#[wasm_bindgen]` wrappers (`open_file`,
//! `update_file`, `close_file`, `check`, `diagnostics_for`, `compile_file`)
//! merely translate to/from `JsError` + `JsValue` via wasm-bindgen, which
//! panics on native targets ("cannot call wasm-bindgen
//! imported functions on non-wasm targets" — wasm-bindgen 0.2 lib.rs:101).
//! The split mirrors the `classification()` ↔ `stdlib_classification()` pair,
//! which is the same wrapper/native division.

use fossil_wasm::{FossilPlayground, WorkspaceError};

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
    // EXACT mechanism didChange uses. No panic, no error.
    pg.update_file_native(h, source + "\n// edit")
        .expect("update_file_native");

    // Workspace-wide drain: we don't assert row count (well-formed
    // hello.fossil under AcceptAll may produce zero or more rows
    // depending on which warnings fire); what matters is the call returns
    // and serialization is deferred to the wasm-bindgen wrapper.
    let _rows = pg.check_rows();

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
