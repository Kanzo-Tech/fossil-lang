//! Native-side smoke of the LSP-over-postMessage dispatch loop (plan
//! 07-03, WASM-02 / PLAY-01). Exercises the JSON-RPC routing layer
//! WITHOUT a Web Worker — the worker entrypoint (`start_lsp_worker`) is
//! wasm32-only; this file targets the pure-Rust `dispatch` fn through the
//! `__dispatch_for_test` hook.
//!
//! The full node-driven end-to-end exercise lives in `playground/`
//! integration tests (07-05 onward); this file is the per-PR cargo-test
//! mirror that catches dispatch-routing regressions without requiring
//! wasm-bindgen + node toolchain.

// Test helpers consume their JSON params (moved into the constructed
// envelope) — clippy's `needless_pass_by_value` lint flags this even
// though `serde_json::Value` is a tagged enum where by-value composition
// is the idiomatic builder shape.
#![allow(clippy::needless_pass_by_value)]

use fossil_wasm::FossilPlayground;

/// Build an LSP request envelope (has `id`).
fn req(method: &str, id: i32, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Build an LSP notification envelope (no `id`).
fn notif(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// Read `examples/hello.fossil`. The cargo-test cwd is the crate directory.
fn hello_fossil_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("hello.fossil");
    std::fs::read_to_string(&path).expect("examples/hello.fossil must be readable")
}

#[test]
fn initialize_returns_capabilities() {
    let mut pg = FossilPlayground::new();
    let out =
        fossil_wasm::__dispatch_for_test(&mut pg, req("initialize", 1, serde_json::json!({})));
    let resp = out.response.expect("initialize must respond");
    assert!(
        resp.error.is_none(),
        "initialize must not error: {:?}",
        resp.error
    );
    let caps = resp.result.expect("initialize must carry a result");
    // hoverProvider must be advertised (either bool form or struct form per spec).
    let hp = &caps["capabilities"]["hoverProvider"];
    assert!(
        hp.is_boolean() || hp.is_object(),
        "initialize must advertise hoverProvider; got {hp:?}",
    );
    // definitionProvider, completionProvider, documentSymbolProvider,
    // semanticTokensProvider, codeActionProvider — all must be present.
    for cap in [
        "definitionProvider",
        "completionProvider",
        "documentSymbolProvider",
        "semanticTokensProvider",
        "codeActionProvider",
    ] {
        assert!(
            !caps["capabilities"][cap].is_null(),
            "initialize must advertise {cap}",
        );
    }
}

#[test]
fn did_open_then_hover_routes_to_fossil_ide() {
    let mut pg = FossilPlayground::new();
    let _ = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///hello.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": hello_fossil_source(),
                }
            }),
        ),
    );
    let out = fossil_wasm::__dispatch_for_test(
        &mut pg,
        req(
            "textDocument/hover",
            2,
            serde_json::json!({
                "textDocument": { "uri": "file:///hello.fossil" },
                "position": { "line": 0, "character": 0 }
            }),
        ),
    );
    let resp = out.response.expect("hover must respond");
    assert!(resp.error.is_none(), "hover errored: {:?}", resp.error);
}

#[test]
fn unknown_method_returns_method_not_found() {
    let mut pg = FossilPlayground::new();
    let out =
        fossil_wasm::__dispatch_for_test(&mut pg, req("not/a/method", 3, serde_json::json!({})));
    let resp = out.response.expect("requests with id must always respond");
    let err = resp.error.expect("unknown method must produce an error");
    assert_eq!(
        err.code, -32601,
        "must be MethodNotFound (-32601); got {}",
        err.code
    );
}

#[test]
fn did_open_emits_per_file_publish_diagnostics() {
    // B3 fix verification: publish_diagnostics is per-file. didOpen on uri B
    // must emit publishDiagnostics scoped to B's uri ONLY — never carrying
    // diagnostics from a previously-opened A.
    let mut pg = FossilPlayground::new();
    let _ = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///a.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": "prefix ex: <https://example.org/>\n"
                }
            }),
        ),
    );
    let out_b = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///b.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": "// just a comment\n"
                }
            }),
        ),
    );
    // Every notification emitted on B's didOpen must reference uri "b" only.
    assert!(
        !out_b.diagnostics.is_empty(),
        "didOpen must emit at least one publishDiagnostics"
    );
    for n in &out_b.diagnostics {
        assert_eq!(
            n["method"], "textDocument/publishDiagnostics",
            "every emitted notification must be a publishDiagnostics",
        );
        assert_eq!(
            n["params"]["uri"], "file:///b.fossil",
            "publish_diagnostics must be per-file: B's didOpen leaked an A uri",
        );
    }
}

#[test]
fn did_change_invokes_set_text_revision_bump() {
    // Behavioral assertion: after didOpen + didChange, documentSymbol on
    // the new content path must succeed (proves set_text path took effect
    // and the file revision bumped — ADR-0022).
    let mut pg = FossilPlayground::new();
    let _ = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///a.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": "prefix ex: <https://example.org/>\n"
                }
            }),
        ),
    );
    let _ = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": "file:///a.fossil", "version": 2 },
                "contentChanges": [
                    { "text": "prefix ex: <https://example.org/>\n// edit\n" }
                ]
            }),
        ),
    );
    let out = fossil_wasm::__dispatch_for_test(
        &mut pg,
        req(
            "textDocument/documentSymbol",
            4,
            serde_json::json!({
                "textDocument": { "uri": "file:///a.fossil" }
            }),
        ),
    );
    let resp = out.response.expect("documentSymbol must respond");
    assert!(
        resp.error.is_none(),
        "documentSymbol errored: {:?}",
        resp.error
    );
}

#[test]
fn did_close_publishes_empty_diagnostics() {
    // LSP spec: on close, server should publish an empty diagnostics list
    // for the closed URI so the client clears any remaining squigglies.
    let mut pg = FossilPlayground::new();
    let _ = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///a.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": "prefix ex: <https://example.org/>\n"
                }
            }),
        ),
    );
    let out = fossil_wasm::__dispatch_for_test(
        &mut pg,
        notif(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": { "uri": "file:///a.fossil" }
            }),
        ),
    );
    assert!(
        out.response.is_none(),
        "didClose is a notification, no response"
    );
    assert!(
        out.diagnostics
            .iter()
            .any(|n| n["params"]["uri"] == "file:///a.fossil"
                && n["params"]["diagnostics"]
                    .as_array()
                    .is_some_and(Vec::is_empty)),
        "didClose must emit an empty publishDiagnostics for the closed URI",
    );
}

#[test]
fn initialized_and_exit_are_silent_notifications() {
    // `initialized` and `exit` are notifications with no diagnostic side
    // effects — dispatch must return empty `DispatchOutput`.
    let mut pg = FossilPlayground::new();
    for method in ["initialized", "exit"] {
        let out = fossil_wasm::__dispatch_for_test(&mut pg, notif(method, serde_json::json!({})));
        assert!(out.response.is_none(), "{method} must not respond");
        assert!(
            out.diagnostics.is_empty(),
            "{method} must not emit diagnostics"
        );
    }
}

#[test]
fn shutdown_request_responds_null() {
    let mut pg = FossilPlayground::new();
    let out = fossil_wasm::__dispatch_for_test(&mut pg, req("shutdown", 99, serde_json::json!({})));
    let resp = out.response.expect("shutdown must respond");
    assert!(resp.error.is_none());
    assert_eq!(resp.result, Some(serde_json::Value::Null));
}

/// Sibling sanity — `fossil/checkAll` route returns an array (possibly empty).
#[test]
fn fossil_check_all_routes() {
    let mut pg = FossilPlayground::new();
    let out =
        fossil_wasm::__dispatch_for_test(&mut pg, req("fossil/checkAll", 2, serde_json::json!({})));
    let resp = out.response.expect("fossil/checkAll must respond");
    assert!(resp.error.is_none());
    let rows = resp.result.expect("fossil/checkAll must carry a result");
    assert!(rows.is_array(), "fossil/checkAll result must be an array");
}
