//! End-to-end LSP feature integration test.
//!
//! Drives the `fossil-lsp` binary over JSON-RPC/stdio (the SAME transport a
//! real editor uses) and asserts that all six capabilities answer over
//! the wire, each a thin adapter over the corresponding `fossil-ide` free
//! function (the feature LOGIC is unit-tested in `fossil-ide`; this test
//! exercises the transport + translation layer):
//!
//!   1. `textDocument/hover`            → Markdown contents
//!   2. `textDocument/definition`       → a `Location` in the SHAPE DOCUMENT
//!   3. `textDocument/completion`       → a non-empty item list
//!   4. `textDocument/documentSymbol`   → the outline
//!   5. `textDocument/semanticTokens/full` → a non-empty token stream
//!   6. `textDocument/codeAction`       → no panic over a (broken-variant) range
//!
//! # The second file is the `.shex`, and goto-def is what proves it
//!
//! This test used to open TWO `.fossil` fixtures, because a `prefix` declared in
//! one resolved from the other. There is no `prefix` and
//! there is no import: a Fossil file is compiled alone, so **no name a program
//! can write resolves into another `.fossil` file**. `canonical_200_b.fossil`
//! was retired with the form it existed for.
//!
//! What crosses a file boundary now crosses a LANGUAGE boundary (ruling 12 of
//! `SURFACE-PLAN.md`): a shape name in a header is declared in the `.shex`, and
//! so is the predicate behind every property key. So case 2 below asserts a
//! `Location` whose URI is `canonical_200.shex` — which additionally exercises
//! the half a two-`.fossil` test never could, the server registering a document
//! it was never asked to open.
//!
//! **The program is opened under its REAL path.** It used to be `file:///tmp/…`,
//! and that was free when the fixture named no document. It is not free now:
//! the server resolves `io.shex("canonical_200.shex")` against the URI the
//! program was opened under, so a `/tmp` URI looks for `/tmp/canonical_200.shex`
//! and the whole output contract silently disappears.
//!
//! All frames are written upfront, then stdin is dropped and stdout drained.
//! That is `tests/common/mod.rs` now — and this file is where the LENIENT
//! `parse_frames` lived, the one that turned a malformed frame into a missing
//! one and reported `missing response id=N` for a server that had written
//! garbage. The shared one is strict; the module docs there say why.

#![cfg(not(target_arch = "wasm32"))]
// The fixtures contain `${ex:}` IRI-template placeholders — LITERAL Fossil
// source embedded in assertion messages, not Rust format args.
#![allow(clippy::literal_string_with_formatting_args)]

mod common;

use std::path::PathBuf;

use common::{did_open, drive, notif, req, text_pos};

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

/// The URI the program is opened under — its REAL path, because the shape
/// document it names is resolved relative to it. See the module docs.
fn uri_program() -> String {
    format!(
        "file://{}",
        repo_root()
            .join("tests/fixtures/canonical_200.fossil")
            .display()
    )
}

/// The URI the server must register the shape document under, on its own, from
/// the program's `io.shex("canonical_200.shex")`.
fn uri_document() -> String {
    format!(
        "file://{}",
        repo_root()
            .join("tests/fixtures/canonical_200.shex")
            .display()
    )
}

fn fixture_a() -> String {
    std::fs::read_to_string(repo_root().join("tests/fixtures/canonical_200.fossil"))
        .expect("read canonical_200.fossil")
}

/// Locate the 0-indexed (line, byte-column) of the FIRST occurrence of `needle`
/// inside `src`, then return `(line, col)` for the position `inside` bytes into
/// the match. Both fixtures are ASCII, so byte columns == UTF-16 columns.
fn pos_of(src: &str, needle: &str, inside: u32) -> (u32, u32) {
    let idx = src
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} not in fixture"));
    let before = &src[..idx];
    let line = u32::try_from(before.matches('\n').count()).unwrap_or(u32::MAX);
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let col = u32::try_from(idx - line_start).unwrap_or(u32::MAX) + inside;
    (line, col)
}

#[test]
#[allow(clippy::too_many_lines)]
fn lsp_serves_all_six_capabilities_over_the_transport() {
    let src_a = fixture_a();
    let uri_a = uri_program();
    let uri_doc = uri_document();

    // The four cursors are located by LITERAL SEARCH in the fixture, so every
    // needle below has to appear in `tests/fixtures/canonical_200.fossil`
    // verbatim. Editing the fixture without editing these is the "needle not in
    // fixture" panic, and that is deliberate: a silent drift would move the
    // cursor onto whatever happened to be at the old offset.

    // hover: inside the identity template of mapping #1. An interpolated string
    // synthesises a reference without needing an input descriptor, which
    // the fixture's sources deliberately do not have.
    let (hover_l, hover_c) = pos_of(&src_a, "@subject = \"https://example.org/person/", 14);
    // definition: on the SHAPE NAME of mapping #1's header. `Person` is declared
    // in `canonical_200.shex`, so this is the language-boundary jump.
    let (def_l, def_c) = pos_of(&src_a, "People : Person from Users", 10);
    // completion: inside mapping #1's body — the stdlib half is unconditional
    // and the shape-property half needs the resolved target shape.
    let (comp_l, comp_c) = pos_of(&src_a, "name = str.trim(Users.name)", 12);
    // codeAction range: over the first property of mapping #1.
    let (ca_l, ca_c) = pos_of(&src_a, "nick = str.lower(Users.nick)", 0);

    {
        let t = drive(&[
            req(
                1,
                "initialize",
                serde_json::json!({ "capabilities": {}, "processId": null, "rootUri": null }),
            ),
            notif("initialized", serde_json::json!({})),
            did_open(&uri_a, &src_a),
            req(2, "textDocument/hover", text_pos(&uri_a, hover_l, hover_c)),
            req(3, "textDocument/definition", text_pos(&uri_a, def_l, def_c)),
            req(
                4,
                "textDocument/completion",
                text_pos(&uri_a, comp_l, comp_c),
            ),
            req(
                5,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri_a } }),
            ),
            req(
                6,
                "textDocument/semanticTokens/full",
                serde_json::json!({ "textDocument": { "uri": uri_a } }),
            ),
            req(
                7,
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": uri_a },
                    "range": {
                        "start": { "line": ca_l, "character": ca_c },
                        "end": { "line": ca_l, "character": ca_c + 9 }
                    },
                    "context": { "diagnostics": [] }
                }),
            ),
            req(99, "shutdown", serde_json::Value::Null),
            notif("exit", serde_json::Value::Null),
        ]);

        // 1. hover → Markdown contents with a fenced fossil block.
        let hover = t.by_id(2);
        let value = hover
            .pointer("/result/contents/value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("hover missing contents.value: {hover}"));
        assert!(
            value.contains("```fossil"),
            "hover markdown must carry a fenced fossil block; got {value:?}"
        );

        // 2. definition → a Location in the SHAPE DOCUMENT. The cursor is on the
        //    shape name `Person`, which is declared in `canonical_200.shex` and
        //    nowhere in the program. Two things have to be true for this to pass
        //    and only one of them is goto-def: the server had to register a
        //    document it was never asked to open, and the resolution had to
        //    leave the language.
        let def = t.by_id(3);
        let locs = def
            .pointer("/result")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("definition result not an array: {def}"));
        assert!(
            !locs.is_empty(),
            "definition must return at least one Location: {def}"
        );
        let doc_hit = locs
            .iter()
            .find(|l| {
                l.pointer("/uri").and_then(serde_json::Value::as_str) == Some(uri_doc.as_str())
            })
            .unwrap_or_else(|| {
                panic!("goto-def on a shape name must point at {uri_doc}; got {locs:?}")
            });
        // And at the shape itself, not at the top of the file — `0:0` is the
        // documented answer for "right file, position unknown", which is what a
        // decoder that kept no offsets would leave. It is not what happens here.
        let line = doc_hit
            .pointer("/range/start/line")
            .and_then(serde_json::Value::as_u64);
        assert!(
            line.is_some_and(|l| l > 0),
            "the target must be the shape's own line in the document, not 0:0; got {doc_hit}"
        );

        // 3. completion → a non-empty item list (stdlib + prefixes are unconditional).
        let comp = t.by_id(4);
        let items = comp
            .pointer("/result")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("completion result not an array: {comp}"));
        assert!(!items.is_empty(), "completion must return items: {comp}");

        // 4. documentSymbol → a non-empty outline.
        let sym = t.by_id(5);
        let symbols = sym
            .pointer("/result")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("documentSymbol result not an array: {sym}"));
        assert!(
            !symbols.is_empty(),
            "documentSymbol must return the outline: {sym}"
        );

        // 5. semanticTokens/full → a non-empty token stream (data is a flat u32 array).
        let tokens = t.by_id(6);
        let data = tokens
            .pointer("/result/data")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("semanticTokens missing result.data: {tokens}"));
        assert!(
            !data.is_empty() && data.len().is_multiple_of(5),
            "semantic tokens must be a non-empty multiple-of-5 stream; got {} entries",
            data.len()
        );

        // 6. codeAction → a Response (array, possibly empty — the fixture is
        //    clean so no quick-fixes; the point is the handler answers without
        //    error and the transport translation works).
        let ca = t.by_id(7);
        assert!(
            ca.pointer("/result")
                .is_some_and(serde_json::Value::is_array),
            "codeAction must return a (possibly empty) array result: {ca}"
        );

        // The published diagnostics for the clean fixture must be empty.
        let diags_a = t.published(&uri_a);
        assert!(
            !diags_a.is_empty(),
            "expected a publishDiagnostics for the program"
        );
        for d in diags_a {
            let arr = d
                .pointer("/params/diagnostics")
                .and_then(serde_json::Value::as_array);
            assert_eq!(
                arr.map(Vec::len),
                Some(0),
                "the canonical fixture type-checks cleanly — diagnostics must be empty: {d}"
            );
        }
    }
}
