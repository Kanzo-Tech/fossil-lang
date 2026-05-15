//! End-to-end smoke test for `fossil-lsp`: spawn the binary, drive it through
//! `initialize` → `initialized` → `didOpen` → `shutdown` → `exit`, parse the
//! framed JSON-RPC responses on stdout, assert the server advertised
//! `TextDocumentSync::FULL` and emitted a `textDocument/publishDiagnostics`
//! with an empty `diagnostics` array, then assert the process exited 0.
//!
//! This is the wave-6 mirror of `crates/fossil-cli/tests/cli_integration.rs`:
//! the LSP transport is exercised end-to-end (binary build + stdio framing +
//! dispatch loop + shutdown handshake) so a regression in any of those layers
//! fails CI before Phase 6 LSP-01 piles real features on top.
//!
//! All frames are written **upfront** (before reading any response) so we
//! cannot deadlock waiting on a server-initiated message Phase 1 never sends.
//! The server's stdin is dropped after the last frame; the matching `exit`
//! notification then closes the receiver inside `main_loop` and the binary
//! exits naturally.

#![cfg(not(target_arch = "wasm32"))]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Walk up two levels from `CARGO_MANIFEST_DIR` (= `…/crates/fossil-lsp`) to
/// find the workspace root. Mirrors `crates/fossil-cli/tests/cli_integration.rs`.
fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

/// Build `target/debug/fossil-lsp` once per test process via `cargo build`,
/// memoised through `OnceLock` so the second test (if any are added) does not
/// pay the build cost or race against the first on the binary path.
fn fossil_lsp_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "-p",
                "fossil-lsp",
                "--bin",
                "fossil-lsp",
            ])
            .status()
            .expect("spawn cargo build for fossil-lsp");
        assert!(status.success(), "cargo build -p fossil-lsp failed");

        let bin = repo_root().join("target").join("debug").join("fossil-lsp");
        assert!(
            bin.exists(),
            "fossil-lsp binary not found at {} after cargo build",
            bin.display(),
        );
        bin
    })
}

/// Encode a JSON-RPC body as an LSP frame: `Content-Length: N\r\n\r\n<body>`.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Parse a stream of `Content-Length`-framed JSON-RPC messages into their
/// JSON bodies. Tolerant of trailing whitespace/empty buffer; returns the
/// list of messages successfully decoded. Headers are case-insensitive per
/// the LSP spec but the server only ever emits canonical `Content-Length:`.
fn parse_frames(mut buf: &[u8]) -> Vec<serde_json::Value> {
    const HEADER: &str = "Content-Length:";
    let mut out = Vec::new();
    while !buf.is_empty() {
        // Find the header boundary (\r\n\r\n).
        let Some(boundary) = find_subslice(buf, b"\r\n\r\n") else {
            break;
        };
        let header_block = std::str::from_utf8(&buf[..boundary]).unwrap_or("");
        let len: usize = header_block
            .lines()
            .find_map(|line| {
                let line = line.trim();
                if line
                    .to_ascii_lowercase()
                    .starts_with(&HEADER.to_ascii_lowercase())
                {
                    line[HEADER.len()..].trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .expect("LSP frame missing Content-Length header");
        let body_start = boundary + 4;
        let body_end = body_start + len;
        assert!(
            body_end <= buf.len(),
            "frame body truncated: declared {len} bytes, only {} available",
            buf.len() - body_start,
        );
        let body = &buf[body_start..body_end];
        let val: serde_json::Value =
            serde_json::from_slice(body).expect("frame body is not valid JSON");
        out.push(val);
        buf = &buf[body_end..];
    }
    out
}

/// `core::slice::contains` for arbitrary needle. Avoids pulling `memchr` for
/// a one-shot 4-byte search.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

#[test]
// Test orchestrates the full handshake (write 5 frames, drain stdout/stderr,
// assert exit + 3 framed responses). Splitting the body would scatter the
// scenario across helpers and make the wire-protocol contract harder to read.
#[allow(clippy::too_many_lines)]
fn lsp_responds_to_initialize_didopen_shutdown() {
    let bin = fossil_lsp_binary();

    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fossil-lsp");

    // ---- Drive the protocol (write all frames upfront) ----
    {
        let stdin = child.stdin.as_mut().expect("child stdin");

        // 1. initialize (request, id=1)
        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "capabilities": {}, "processId": null, "rootUri": null }
        });
        stdin
            .write_all(frame(&init_req.to_string()).as_bytes())
            .expect("write initialize");

        // 2. initialized (notification — completes the handshake)
        let init_notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        stdin
            .write_all(frame(&init_notif.to_string()).as_bytes())
            .expect("write initialized");

        // 3. textDocument/didOpen (notification → triggers empty publishDiagnostics)
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/x.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": "prefix ex: <https://example.org/>\n"
                }
            }
        });
        stdin
            .write_all(frame(&did_open.to_string()).as_bytes())
            .expect("write didOpen");

        // 4. shutdown (request, id=2)
        let shutdown_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown",
            "params": null
        });
        stdin
            .write_all(frame(&shutdown_req.to_string()).as_bytes())
            .expect("write shutdown");

        // 5. exit (notification — closes the loop)
        let exit_notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null
        });
        stdin
            .write_all(frame(&exit_notif.to_string()).as_bytes())
            .expect("write exit");

        stdin.flush().expect("flush child stdin");
    }
    // Drop child.stdin by taking it out — closes the pipe so the server's
    // stdin reader returns EOF after consuming the buffered frames.
    drop(child.stdin.take());

    // ---- Read everything the server wrote, then assert clean exit ----
    let mut stdout_buf = Vec::new();
    child
        .stdout
        .as_mut()
        .expect("child stdout")
        .read_to_end(&mut stdout_buf)
        .expect("read child stdout");
    let mut stderr_buf = Vec::new();
    child
        .stderr
        .as_mut()
        .expect("child stderr")
        .read_to_end(&mut stderr_buf)
        .expect("read child stderr");

    let exit_status = child.wait().expect("fossil-lsp should terminate");
    assert!(
        exit_status.success(),
        "fossil-lsp should exit cleanly; got {exit_status:?}\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&stdout_buf),
        String::from_utf8_lossy(&stderr_buf),
    );

    // ---- Parse the framed responses ----
    let frames = parse_frames(&stdout_buf);
    assert!(
        !frames.is_empty(),
        "expected at least one framed response on stdout; got 0 (raw bytes: {})",
        String::from_utf8_lossy(&stdout_buf),
    );

    // The initialize response: id == 1, result.capabilities.textDocumentSync == 1 (FULL).
    let init_response = frames
        .iter()
        .find(|m| m.get("id").and_then(serde_json::Value::as_i64) == Some(1))
        .expect("missing initialize response (id=1)");
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

    // The publishDiagnostics notification: method == textDocument/publishDiagnostics,
    // params.diagnostics is an empty array.
    let publish = frames
        .iter()
        .find(|m| {
            m.get("method").and_then(serde_json::Value::as_str)
                == Some("textDocument/publishDiagnostics")
        })
        .expect("missing publishDiagnostics notification for the didOpen URI");
    let diagnostics = publish
        .pointer("/params/diagnostics")
        .and_then(serde_json::Value::as_array)
        .expect("publishDiagnostics missing params.diagnostics array");
    assert!(
        diagnostics.is_empty(),
        "Phase 1 LSP must publish EMPTY diagnostics (Phase 6 LSP-01 wires the real pipeline); \
         got {} diagnostics: {:?}",
        diagnostics.len(),
        diagnostics,
    );

    // The shutdown response: id == 2, result == null per LSP spec.
    let shutdown_response = frames
        .iter()
        .find(|m| m.get("id").and_then(serde_json::Value::as_i64) == Some(2))
        .expect("missing shutdown response (id=2)");
    assert!(
        shutdown_response
            .get("result")
            .is_some_and(serde_json::Value::is_null),
        "shutdown response must have result: null per LSP spec; got {shutdown_response}",
    );
}
