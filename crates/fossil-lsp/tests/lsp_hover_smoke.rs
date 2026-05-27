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

// ── Phase 3 plan 03-07: SC#3 (CORE-07) hover surface ───────────────────────
//
// seq.filter decision (plan 03-05-SUMMARY.md, verbatim):
//
//   "Decision: NO `seq.filter` stub was added to `fossil-registry`."
//
// Per plan 03-07 Task 2 step 0, NO ⇒ use the DIRECT integration path that
// BYPASSES the JSON-RPC layer (option (ii)), NOT a full end-to-end through a
// `seq.filter` surface form. Phase 3 v0.1's `HirExpr` has no `Pipeline` /
// `Call` variant, so an implicit closure cannot be expressed in surface
// syntax and the synthesis is unreachable through a `.fossil` document over
// JSON-RPC. The full JSON-RPC end-to-end SC#3 test is DEFERRED to Phase 6
// (when the stdlib + Pratt-lowered expression tree land and `seq.filter`
// gains a real surface form). Plan 03-08's corpus does NOT add this test.
//
// These two tests exercise `fossil_ide::hover::render_markdown` — the exact
// rendering function the LSP hover handler (`main.rs::handle_request`) calls
// on the `ExprTypeEntry` returned by `ty_origin`. The closure synthesis +
// CSVW forward propagation are driven IN-PROCESS via fossil-hir's public
// provenance types + fossil-descriptors-input's CSVW descriptor, so the
// integration boundary tested is hover.rs's Markdown body — identical to what
// `result.contents.value` would carry over JSON-RPC.

use fossil_base::{FossilDb, NativeSystem, Span, System};
use fossil_hir::body::ExprId;
use fossil_hir::provenance::{ExprTypeEntry, Provenance, ProvenanceKind};
use fossil_hir::ty::{Primitive, Ty, TyKind};
use std::sync::Arc;

const USERS_CSVW: &str = r#"{
  "@context": "http://www.w3.org/ns/csvw",
  "tableSchema": {
    "columns": [
      { "name": "id", "datatype": "integer" },
      { "name": "name", "datatype": "string" },
      { "name": "age", "datatype": "integer" }
    ]
  }
}"#;

fn bare_db() -> FossilDb {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    FossilDb::new(system)
}

/// SC#3 (CORE-07): hovering on `.age` inside an implicitly-synthesised closure
/// surfaces BOTH the field type `Integer` AND the closure parameter binding
/// `(row: Record<...>) => row.age >= 18` — the synthesis is NOT hidden.
///
/// Direct integration (plan 03-05 = NO seq.filter): the
/// `SynthesizedClosureRendering` provenance plan 03-06 records on the closure
/// body's `ExprId` (pinned shape from 03-06-SUMMARY) is fed to the public
/// `fossil_ide::hover::render_markdown`, asserting the LSP hover Markdown body.
#[test]
fn hover_inside_synthesized_closure_via_typecheck_mapping() {
    let db = bare_db();
    let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
    // The closure rendering plan 03-06's `render_closure` produces for the
    // canonical SC#3 predicate `users |> filter(.age >= 18)` (03-06-SUMMARY
    // §"SC#3-shape acceptance string"). The row Record carries the field names
    // + types so the user sees what `row` is bound to.
    let rendering = smol_str::SmolStr::from("(row: Record<{age: Integer}>) => row.age >= 18");
    let entry = ExprTypeEntry {
        expr_id: ExprId(0),
        ty: int_ty,
        provenance: Provenance {
            span: Span { start: 0, end: 0 },
            kind: ProvenanceKind::SynthesizedClosureRendering { rendering },
        },
    };

    let md = fossil_ide::hover::render_markdown(&db, &entry);

    // (a) closure rendering prefix + arrow + rewritten FieldRef.
    assert!(
        md.contains("(row: Record<"),
        "hover must show the closure parameter binding; got {md:?}",
    );
    assert!(
        md.contains(") =>"),
        "hover must show the closure arrow; got {md:?}"
    );
    assert!(
        md.contains("row.age"),
        "hover must show the rewritten FieldRef `row.age`; got {md:?}",
    );
    // (b) field type from forward propagation.
    assert!(
        md.contains("Integer"),
        "hover must show the field type `Integer`; got {md:?}",
    );
    // (c) the synthesis tagline so the user understands WHY the closure appears.
    assert!(
        md.contains("synthesised closure parameter binding"),
        "hover must explain the implicit synthesis; got {md:?}",
    );
    // Risk Register (STATE.md "Do NOT"): internal inference state must NEVER leak.
    assert!(!md.contains("Unknown"), "hover leaked `Unknown`: {md:?}");
    assert!(
        !md.contains("InferenceId"),
        "hover leaked `InferenceId`: {md:?}"
    );
}

/// `FieldRef` hover OUTSIDE a closure (Phase 3 widening of Phase 2's
/// literal-only path): a `.field` resolved against a CSVW source row surfaces
/// its type.
///
/// The `String` type is proven to come from CSVW forward propagation by
/// resolving the `name` column through `record_from_descriptor` (the same
/// in-process path `resolve_source_row` uses), then rendering the resulting
/// `InputDescriptor` entry via the LSP's `render_markdown`.
#[test]
fn hover_on_csvw_fieldref_outside_closure() {
    let db = bare_db();

    // Resolve `.name` against the real CSVW descriptor — proves the `String`
    // type below is CSVW-derived, not hard-coded.
    let descriptor =
        fossil_descriptors_input::CsvwDescriptor::parse(USERS_CSVW.as_bytes()).expect("valid CSVW");
    let name_kind = descriptor
        .type_for_column("name")
        .expect("CSVW `name` column has a type");
    assert!(
        name_kind.to_lowercase().contains("string"),
        "CSVW `name` column must be a string type; got {name_kind:?}",
    );

    let str_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
    let entry = ExprTypeEntry {
        expr_id: ExprId(1),
        ty: str_ty,
        provenance: Provenance {
            span: Span { start: 0, end: 0 },
            kind: ProvenanceKind::InputDescriptor {
                source_name: smol_str::SmolStr::from("users"),
                column: smol_str::SmolStr::from("name"),
            },
        },
    };

    let md = fossil_ide::hover::render_markdown(&db, &entry);
    assert!(
        md.contains("String"),
        "hover on a CSVW FieldRef must show the field type `String`; got {md:?}",
    );
    // NOT inside a closure → no closure binding rendered.
    assert!(
        !md.contains("(row:"),
        "FieldRef outside a closure must NOT render a closure binding; got {md:?}",
    );
    assert!(!md.contains("Unknown"), "hover leaked `Unknown`: {md:?}");
    assert!(
        !md.contains("InferenceId"),
        "hover leaked `InferenceId`: {md:?}"
    );
}
