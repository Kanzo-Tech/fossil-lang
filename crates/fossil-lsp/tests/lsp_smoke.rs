//! End-to-end smoke test for `fossil-lsp`: spawn the binary, drive it through
//! `initialize` → `initialized` → `didOpen` → `shutdown` → `exit`, parse the
//! framed JSON-RPC responses on stdout, assert the server advertised
//! `TextDocumentSync::FULL` and emitted a `textDocument/publishDiagnostics`
//! with an empty `diagnostics` array, then assert the process exited 0.
//!
//! The LSP transport end-to-end — binary build + stdio framing + dispatch loop
//! + shutdown handshake — so a regression in any of those layers fails CI
//! before the feature tests are reached.
//!
//! The wire mechanics — spawning, writing every frame upfront, dropping stdin,
//! draining both pipes, asserting exit 0, parsing the frames — are
//! `tests/common/mod.rs`, which also records why they are written once.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::{did_open, drive, notif, req};

/// The buffer this test opens. A `.fossil` URI, so the provider catalogue
/// claims nothing and the file is checked as a program — see
/// `documents_are_not_programs.rs` for the other half of that rule.
const URI: &str = "file:///tmp/x.fossil";

#[test]
fn lsp_responds_to_initialize_didopen_shutdown() {
    let t = drive(&[
        req(
            1,
            "initialize",
            serde_json::json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", serde_json::json!({})),
        did_open(URI, "users := io.csv(\"u.csv\")\n"),
        req(2, "shutdown", serde_json::Value::Null),
        notif("exit", serde_json::Value::Null),
    ]);

    assert!(
        !t.frames.is_empty(),
        "expected at least one framed response on stdout; got 0 (raw bytes: {})",
        t.stdout,
    );

    // The initialize response: id == 1, result.capabilities.textDocumentSync == 1 (FULL).
    let init_response = t.by_id(1);
    let sync = init_response
        .pointer("/result/capabilities/textDocumentSync")
        .expect("initialize response missing capabilities.textDocumentSync");
    // `TextDocumentSyncCapability::Kind(FULL)` serialises as the bare integer
    // `1`. (The alternative variant — `Options` — would serialise as an
    // object; we deliberately advertise the simple integer form.)
    assert_eq!(
        sync.as_i64(),
        Some(1),
        "expected text_document_sync to serialise as 1 (FULL); got {sync}",
    );

    // The publishDiagnostics notification for the opened buffer: params.diagnostics
    // is an empty array.
    let published = t.published(URI);
    let publish = published
        .first()
        .expect("missing publishDiagnostics notification for the didOpen URI");
    let diagnostics = publish
        .pointer("/params/diagnostics")
        .and_then(serde_json::Value::as_array)
        .expect("publishDiagnostics missing params.diagnostics array");
    assert!(
        diagnostics.is_empty(),
        "a program with no mistake in it publishes EMPTY diagnostics; got {} diagnostics: {:?}",
        diagnostics.len(),
        diagnostics,
    );

    // The shutdown response: id == 2, result == null per LSP spec.
    let shutdown_response = t.by_id(2);
    assert!(
        shutdown_response
            .get("result")
            .is_some_and(serde_json::Value::is_null),
        "shutdown response must have result: null per LSP spec; got {shutdown_response}",
    );
}
