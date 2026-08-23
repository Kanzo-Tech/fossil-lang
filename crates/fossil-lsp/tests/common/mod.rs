//! Shared JSON-RPC-over-stdio support for the `fossil-lsp` integration tests.
//!
//! Every test in this directory that talks to the SERVER talks to it the way an
//! editor does: spawn the binary, write framed JSON-RPC to its stdin, read
//! framed JSON-RPC back off its stdout. That was written out three times —
//! `lsp_smoke.rs`, `lsp_features.rs`, `lsp_hover_smoke.rs` each carried their
//! own `fossil_lsp_binary`, `frame`, `parse_frames`, `find_subslice` and a
//! ~40-line spawn/write/drain/exit block. This is the one copy.
//!
//! `didchange_budget.rs` is deliberately not a caller: it drives the db in
//! process and never touches the wire.
//!
//! # The three `parse_frames` were not the same function
//!
//! Two of them (`lsp_smoke.rs`, `lsp_hover_smoke.rs`) were STRICT: a frame
//! without a `Content-Length` header, a truncated body, or a body that is not
//! JSON each panicked with what was wrong. The third (`lsp_features.rs`) was
//! LENIENT — `unwrap_or(0)` for a missing header, `break` on a short body,
//! `if let Ok(..)` silently dropping a body that did not parse. [`parse_frames`]
//! below is the strict one, and the lenient one is gone rather than merged:
//! **it turned a malformed frame into a missing one.** Its failures read
//! `missing response id=4` when the truth was that the server wrote garbage on
//! the wire, which is a different bug in a different layer and the message
//! pointed at neither.
//!
//! What survives from the lenient version is the loop's one real tolerance:
//! stopping at a trailing partial header (`\r\n\r\n` never found), which is what
//! an empty or whitespace-padded tail of the stream looks like.
//!
//! # `dead_code`, deliberately allowed
//!
//! A `tests/common/mod.rs` is compiled once **per test binary**, so every
//! binary that declares `mod common;` gets all of it — and every helper that
//! particular binary does not call is dead code in that compilation. The
//! alternative is a feature-gate per helper, which is bookkeeping in exchange
//! for a lint that cannot tell us anything here: the module is dead only in the
//! binaries that do not need it, and it is `git grep` that says whether a
//! helper has any caller at all.

// See the module docs: per-binary compilation makes unused helpers unavoidable.
#![allow(dead_code)]
// `req` / `notif` take `serde_json::Value` by value for call-site ergonomics —
// `json!(..)` moves straight into them.
#![allow(clippy::needless_pass_by_value)]
// `pub(crate)` and not `pub` throughout: this module is private, so clippy calls
// the restriction redundant and rustc's `unreachable_pub` calls the unrestricted
// form unreachable. Only one of the two can be satisfied, and the rustc lint is
// the one this workspace opted into by name (root `Cargo.toml`,
// `[workspace.lints.rust]`). Same dance as `fossil-cli/tests/common/mod.rs`.
#![allow(clippy::redundant_pub_crate)]

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::Value;

/// The `fossil-lsp` binary these tests drive — cargo's own path for it.
///
/// **It used to shell out to `cargo build` and then hard-code
/// `<repo>/target/debug/fossil-lsp`, and that is a test that can pass against a
/// binary it did not build.** With `CARGO_TARGET_DIR` set — which this repo's
/// own instructions require, because several agents share one build directory —
/// the build lands somewhere else and the hard-coded path holds whatever was
/// left there last. It was measured on 2026-08-13: the file at that path was two
/// days old and still spoke a retired grammar, so every assertion was about a
/// compiler nobody had edited.
///
/// `CARGO_BIN_EXE_<name>` is cargo's answer: it is set for an integration test
/// to the path of that package's binary, and cargo has already BUILT it before
/// the test runs. No `Command`, no path arithmetic, and no way to drift.
pub(crate) fn fossil_lsp_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| PathBuf::from(env!("CARGO_BIN_EXE_fossil-lsp")))
}

/// Encode a JSON-RPC body as an LSP frame: `Content-Length: N\r\n\r\n<body>`.
pub(crate) fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// One JSON-RPC request frame.
pub(crate) fn req(id: i64, method: &str, params: Value) -> String {
    frame(
        &serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
            .to_string(),
    )
}

/// One JSON-RPC notification frame.
pub(crate) fn notif(method: &str, params: Value) -> String {
    frame(&serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string())
}

/// A `textDocument/didOpen` frame for `uri` carrying `text`.
pub(crate) fn did_open(uri: &str, text: &str) -> String {
    notif(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": { "uri": uri, "languageId": "fossil", "version": 1, "text": text }
        }),
    )
}

/// The `{ textDocument, position }` params every positional request takes.
pub(crate) fn text_pos(uri: &str, line: u32, character: u32) -> Value {
    serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character }
    })
}

/// What one server run wrote.
///
/// The handshake ids are NOT in here and not in [`drive`]: they are test data,
/// and two callers assert on the `initialize` and `shutdown` responses by the id
/// they chose.
pub(crate) struct Transcript {
    /// Every framed message the server wrote to stdout, in order.
    pub(crate) frames: Vec<Value>,
    /// Everything it wrote to stderr — the tracing log, and the only place a
    /// server-side panic shows up.
    pub(crate) stderr: String,
    /// The raw stdout bytes, lossily decoded. For the failure message when
    /// [`Self::frames`] is empty and the interesting thing is what was there
    /// instead.
    pub(crate) stdout: String,
}

impl Transcript {
    /// The Response with this `id`, or a panic naming the id, the frames and
    /// stderr — a missing response is almost always a server-side error, and
    /// stderr is where it is written.
    pub(crate) fn by_id(&self, id: i64) -> &Value {
        self.frames
            .iter()
            .find(|m| m.get("id").and_then(Value::as_i64) == Some(id))
            .unwrap_or_else(|| {
                panic!(
                    "missing response id={id}; frames were: {:#?}\nstderr: {}",
                    self.frames, self.stderr
                )
            })
    }

    /// Every `textDocument/publishDiagnostics` notification for `uri`, in order.
    ///
    /// Empty is a real answer and the caller decides what it means: the server
    /// publishes once per `didOpen`/`didChange`, so no entry means the buffer was
    /// never opened, and one entry with an empty array means it was opened and is
    /// clean.
    pub(crate) fn published(&self, uri: &str) -> Vec<&Value> {
        self.frames
            .iter()
            .filter(|m| {
                m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                    && m.pointer("/params/uri").and_then(Value::as_str) == Some(uri)
            })
            .collect()
    }
}

/// Spawn the server, write every frame **upfront**, close stdin, drain both
/// pipes, assert a clean exit, and parse what came back.
///
/// Writing everything before reading anything is what makes this deadlock-free:
/// the server never initiates a request, so there is nothing we could be
/// expected to answer mid-stream. Dropping stdin after the last frame is what
/// lets the `exit` notification close `main_loop`'s receiver and the process end
/// on its own.
///
/// The caller supplies the whole conversation including `initialize`,
/// `initialized`, `shutdown` and `exit` — a server driven without them is a
/// scenario a test may legitimately want.
pub(crate) fn drive(frames: &[String]) -> Transcript {
    let mut child = Command::new(fossil_lsp_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fossil-lsp");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for f in frames {
            stdin.write_all(f.as_bytes()).expect("write frame");
        }
        stdin.flush().expect("flush child stdin");
    }
    // Take stdin out to drop it — closes the pipe, so the server's reader sees
    // EOF once it has consumed the buffered frames.
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

    let status = child.wait().expect("fossil-lsp should terminate");
    let stdout = String::from_utf8_lossy(&stdout_buf).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_buf).into_owned();
    assert!(
        status.success(),
        "fossil-lsp should exit cleanly; got {status:?}\nstdout: {stdout}\nstderr: {stderr}",
    );

    Transcript {
        frames: parse_frames(&stdout_buf),
        stderr,
        stdout,
    }
}

/// Parse a stream of `Content-Length`-framed JSON-RPC messages into their JSON
/// bodies.
///
/// Strict on purpose — see the module docs. A frame whose header is missing,
/// whose body is short, or whose body is not JSON is a panic naming which,
/// because each is a different defect in the server and none of them is «no
/// response». Headers are matched case-insensitively per the LSP spec even
/// though the server only ever emits the canonical spelling.
fn parse_frames(mut buf: &[u8]) -> Vec<Value> {
    const HEADER: &str = "Content-Length:";
    let mut out = Vec::new();
    while !buf.is_empty() {
        // No header boundary left: the tail is whitespace or an empty buffer.
        let Some(boundary) = find_subslice(buf, b"\r\n\r\n") else {
            break;
        };
        let header_block = std::str::from_utf8(&buf[..boundary]).unwrap_or("");
        let len: usize = header_block
            .lines()
            .find_map(|line| {
                let line = line.trim();
                line.to_ascii_lowercase()
                    .starts_with(&HEADER.to_ascii_lowercase())
                    .then(|| line[HEADER.len()..].trim().parse::<usize>().ok())
                    .flatten()
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
        let val: Value = serde_json::from_slice(body).expect("frame body is not valid JSON");
        out.push(val);
        buf = &buf[body_end..];
    }
    out
}

/// `core::slice::contains` for an arbitrary needle. Avoids pulling `memchr` in
/// for a one-shot 4-byte search.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}
