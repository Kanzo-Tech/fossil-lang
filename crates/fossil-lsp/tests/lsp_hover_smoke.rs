//! End-to-end smoke test for `textDocument/hover` — Phase 2 plan 02-06.
//!
//! Spawns the `fossil-lsp` binary, drives it through `initialize` →
//! `initialized` → `didOpen` (with a `.fossil` source containing an
//! iri-template property) → `textDocument/hover` (at a position inside the
//! template) → `shutdown` → `exit`. Asserts:
//!
//! 1. The hover response is a JSON-RPC Response (matched on `id`).
//! 2. `result.contents.value` contains the rendered ty (`"IriTemplate"`).
//! 3. `result.contents.value` contains the fenced fossil code block
//!    opener (```` ```fossil ````).
//! 4. `result.contents.kind` is `"markdown"`.
//!
//! Per checker Warning W4: the test binary is built via `cargo build -p
//! fossil-lsp` BEFORE the test runs (Phase 1 baseline pattern from
//! `lsp_smoke.rs`'s `fossil_lsp_binary()` helper) so a build error surfaces
//! distinctly from a test failure ("binary not found" vs. assertion fail).
//!
//! Pattern mirrors Phase 1's `lsp_smoke.rs`: all frames written upfront,
//! then stdin dropped, then stdout drained and parsed.

#![cfg(not(target_arch = "wasm32"))]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

/// Build + cache the `fossil-lsp` binary path. Per checker Warning W4, we
/// build via `cargo build` (NOT `cargo test`) so build errors are reported
/// distinctly from test failures.
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

fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn parse_frames(mut buf: &[u8]) -> Vec<serde_json::Value> {
    const HEADER: &str = "Content-Length:";
    let mut out = Vec::new();
    while !buf.is_empty() {
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

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// `.fossil` source the hover smoke test opens. Line layout (0-indexed):
///   0: prefix ex: <https://example.org/>
///   1: users := io.csv("x.csv")
///   2: User : ex:Person from users
///   3:     iri = `${ex:}u/${.id}`
///   4:     ex:name = .name
///
/// The hover request targets line 3, character 10 — inside the iri
/// template property. `ty_origin` synthesises `IriTemplate` for `ExprId(0)`.
const FOSSIL_SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

#[test]
#[allow(clippy::too_many_lines)]
fn lsp_hover_on_iri_template_returns_markdown_with_iri_template_label() {
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

        // 1. initialize (id=1)
        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "capabilities": {}, "processId": null, "rootUri": null }
        });
        stdin
            .write_all(frame(&init_req.to_string()).as_bytes())
            .expect("write initialize");

        // 2. initialized
        let init_notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        stdin
            .write_all(frame(&init_notif.to_string()).as_bytes())
            .expect("write initialized");

        // 3. didOpen
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/hover.fossil",
                    "languageId": "fossil",
                    "version": 1,
                    "text": FOSSIL_SRC,
                }
            }
        });
        stdin
            .write_all(frame(&did_open.to_string()).as_bytes())
            .expect("write didOpen");

        // 4. textDocument/hover (id=2) — line 3, character 10 (inside iri template)
        let hover_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": "file:///tmp/hover.fossil" },
                "position": { "line": 3, "character": 10 }
            }
        });
        stdin
            .write_all(frame(&hover_req.to_string()).as_bytes())
            .expect("write hover");

        // 5. shutdown (id=3)
        let shutdown_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "shutdown",
            "params": null
        });
        stdin
            .write_all(frame(&shutdown_req.to_string()).as_bytes())
            .expect("write shutdown");

        // 6. exit
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
    drop(child.stdin.take());

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

    let frames = parse_frames(&stdout_buf);
    assert!(
        !frames.is_empty(),
        "expected at least one framed response on stdout; got 0 (raw bytes: {})",
        String::from_utf8_lossy(&stdout_buf),
    );

    // The hover response (id=2) MUST be a Markdown Hover.
    let hover_response = frames
        .iter()
        .find(|m| m.get("id").and_then(serde_json::Value::as_i64) == Some(2))
        .unwrap_or_else(|| {
            panic!(
                "missing hover response (id=2); frames were: {frames:#?}\n\
                 stderr: {}",
                String::from_utf8_lossy(&stderr_buf),
            )
        });
    let contents = hover_response
        .pointer("/result/contents")
        .unwrap_or_else(|| panic!("hover response missing result.contents: {hover_response}"));
    let kind = contents
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .expect("hover contents missing kind");
    assert_eq!(
        kind, "markdown",
        "expected MarkupKind::Markdown, got {kind}"
    );
    let value = contents
        .get("value")
        .and_then(serde_json::Value::as_str)
        .expect("hover contents missing value");
    assert!(
        value.contains("IriTemplate"),
        "expected hover markdown to contain 'IriTemplate' (the rendered Ty); got {value:?}",
    );
    assert!(
        value.contains("```fossil"),
        "expected hover markdown to contain a fenced fossil code block; got {value:?}",
    );
    assert!(
        value.contains("Literal"),
        "expected hover markdown to mention the Literal provenance kind; got {value:?}",
    );

    // The initialize response: id == 1, hover_provider advertised.
    let init_response = frames
        .iter()
        .find(|m| m.get("id").and_then(serde_json::Value::as_i64) == Some(1))
        .expect("missing initialize response (id=1)");
    let hover_cap = init_response
        .pointer("/result/capabilities/hoverProvider")
        .expect("initialize response missing capabilities.hoverProvider");
    assert_eq!(
        hover_cap,
        &serde_json::Value::Bool(true),
        "Phase 2 plan 02-06 must advertise hoverProvider: true; got {hover_cap}",
    );

    // The shutdown response: id == 3, result == null.
    let shutdown_response = frames
        .iter()
        .find(|m| m.get("id").and_then(serde_json::Value::as_i64) == Some(3))
        .expect("missing shutdown response (id=3)");
    assert!(
        shutdown_response
            .get("result")
            .is_some_and(serde_json::Value::is_null),
        "shutdown response must have result: null per LSP spec; got {shutdown_response}",
    );
}
