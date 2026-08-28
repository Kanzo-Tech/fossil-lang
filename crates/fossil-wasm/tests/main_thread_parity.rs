//! The third wire, checked against the second: `hover` / `completions` /
//! `gotoDefinition` called as methods must answer what `lsp_worker`'s
//! `textDocument/*` routes answer, over the same buffers at the same cursors.
//!
//! # Why this file exists and a feature test would not do
//!
//! `/docs/design/three-hosts` states the rule this is the instance of: *two
//! hosts that share a function still disagree if they serialize it
//! differently — so a capability offered to two hosts owes a test that crosses
//! each host's wire rather than each host's function.* Both surfaces call the
//! same `fossil-ide` free function, so a defect in the ANSWER is equally
//! present on both sides and invisible here. What is not shared is the
//! projection: the worker builds `lsp_types::Hover` and this one builds
//! [`fossil_wasm::HoverRow`], the worker converts a byte range with its own
//! private helper and this one calls `fossil_ide::byte_range_to_range`, and the
//! worker emits `file://…` where this one emits the registry key. Every one of
//! those is a place the two can drift while both tests stay green.
//!
//! `crates/fossil-lsp/tests/transport_parity.rs` is the same guard between the
//! native binary and the worker. This is its third leg.
//!
//! # What it does NOT prove
//!
//! - **That either side is right.** No expected hover string, no expected
//!   label, no range literal is written here; a copy of the answer cannot
//!   notice the answer changing. `crates/fossil-ide/tests/` is where content is
//!   asserted.
//! - **Anything about `serde_wasm_bindgen`.** Both sides are read as Rust
//!   values; the wasm-bindgen serialisation is wasm32-only. What removes that
//!   risk instead is that no row on this surface has an `Option` field — the
//!   one shape the two serializers are measured to disagree about.
//! - **The two HOSTS.** One `FossilPlayground` each, opened with identical
//!   buffers under identical keys, so the hosts cannot differ.

#![cfg(not(target_arch = "wasm32"))]

use fossil_wasm::FossilPlayground;

/// Both buffers are opened under `file://` keys because the worker's
/// `didOpen` deserialises `uri` into an `lsp_types::Uri` and the direct
/// surface has to be given the same registry keys, or the two would be
/// answering about different files rather than about the same one.
const PROGRAM_URI: &str = "file:///hello.fossil";
const SHEX_URI: &str = "file:///hello.shex";

fn read(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("examples/{name}: {e}"))
}

/// A workspace driven through the JSON-RPC dispatch loop.
fn worker_side() -> FossilPlayground {
    let mut pg = FossilPlayground::new();
    for (uri, text) in [
        (SHEX_URI, read("hello.shex")),
        (PROGRAM_URI, read("hello.fossil")),
    ] {
        let _ = fossil_wasm::__dispatch_for_test(
            &mut pg,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": {
                    "uri": uri, "languageId": "fossil", "version": 1, "text": text,
                }},
            }),
        );
    }
    pg
}

/// The same workspace, driven through `open_file` — and the handle for the
/// program, which is what the direct surface keys on.
fn direct_side() -> (FossilPlayground, fossil_wasm::FileHandle) {
    let mut pg = FossilPlayground::new();
    pg.open_file_native(SHEX_URI.to_string(), read("hello.shex"));
    let handle = pg.open_file_native(PROGRAM_URI.to_string(), read("hello.fossil"));
    (pg, handle)
}

fn request(
    pg: &mut FossilPlayground,
    method: &str,
    line: u32,
    character: u32,
) -> serde_json::Value {
    let out = fossil_wasm::__dispatch_for_test(
        pg,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": {
                "textDocument": { "uri": PROGRAM_URI },
                "position": { "line": line, "character": character },
            },
        }),
    );
    let resp = out
        .response
        .unwrap_or_else(|| panic!("{method} must respond"));
    assert!(resp.error.is_none(), "{method} errored: {:?}", resp.error);
    resp.result.unwrap_or(serde_json::Value::Null)
}

/// Every `(line, character)` in `hello.fossil`, one per line, plus one column
/// past each line's end.
///
/// Sweeping the file rather than naming cursors is what keeps this honest: a
/// hand-picked position is a place somebody already knew the two agreed, and
/// the interesting disagreement is the one nobody thought of. Ninety-odd
/// positions is nothing — the workspace is warm and every query is memoised.
fn every_cursor() -> Vec<(u32, u32)> {
    read("hello.fossil")
        .lines()
        .enumerate()
        .flat_map(|(line, text)| {
            let width = u32::try_from(text.chars().count()).expect("line width fits u32");
            #[allow(clippy::cast_possible_truncation)]
            (0..=width).map(move |ch| (line as u32, ch))
        })
        .collect()
}

#[test]
fn hover_agrees_over_the_whole_file() {
    let mut worker = worker_side();
    let (direct, handle) = direct_side();
    let mut answered = 0;

    for (line, character) in every_cursor() {
        let wire = request(&mut worker, "textDocument/hover", line, character);
        let row = direct.hover_row(handle, line, character);

        match (&wire, &row) {
            (serde_json::Value::Null, None) => {}
            (serde_json::Value::Null, Some(row)) => {
                panic!("{line}:{character}: the method hovers ({row:?}) and the wire does not")
            }
            (_, None) => {
                panic!("{line}:{character}: the wire hovers ({wire}) and the method does not")
            }
            (_, Some(row)) => {
                answered += 1;
                assert_eq!(
                    wire["contents"]["value"].as_str(),
                    Some(row.markdown.as_str()),
                    "{line}:{character}: two renderings of one hover",
                );
                assert_eq!(
                    wire["range"],
                    serde_json::to_value(row.range).expect("a Range serialises"),
                    "{line}:{character}: two conversions of one byte range",
                );
            }
        }
    }

    assert!(
        answered > 0,
        "no cursor in hello.fossil hovered, so this test compared nothing"
    );
}

#[test]
fn completion_agrees_over_the_whole_file() {
    let mut worker = worker_side();
    let (direct, handle) = direct_side();
    let mut offered = 0;

    for (line, character) in every_cursor() {
        let wire = request(&mut worker, "textDocument/completion", line, character);
        let rows = direct.completion_rows(handle, line, character);
        let items = wire.as_array().cloned().unwrap_or_default();

        assert_eq!(
            items.len(),
            rows.len(),
            "{line}:{character}: {} items on the wire, {} from the method",
            items.len(),
            rows.len(),
        );
        offered += rows.len();

        for (item, row) in items.iter().zip(&rows) {
            assert_eq!(
                item["label"].as_str(),
                Some(row.label.as_str()),
                "{line}:{character}"
            );
            assert_eq!(
                item["detail"].as_str().unwrap_or_default(),
                row.detail,
                "{line}:{character}: {}",
                row.label,
            );
            // The kind is an integer on the wire and a name off the method.
            // Routing the wire's number through the SAME function rather than
            // through a second table is the point: a table written here could
            // agree with neither side.
            let kind = item["kind"]
                .as_i64()
                .map(|k| serde_json::from_value(serde_json::json!(k)).expect("an LSP kind"));
            assert_eq!(
                fossil_wasm::ide::kind_name(kind),
                row.kind,
                "{line}:{character}: {} carries two kinds",
                row.label,
            );
        }
    }

    assert!(
        offered > 0,
        "no cursor in hello.fossil offered a completion, so this test compared nothing"
    );
}

#[test]
fn definition_agrees_over_the_whole_file() {
    let mut worker = worker_side();
    let (direct, handle) = direct_side();
    let mut resolved = 0;

    for (line, character) in every_cursor() {
        let wire = request(&mut worker, "textDocument/definition", line, character);
        let rows = direct.definition_rows(handle, line, character);
        let locations = wire.as_array().cloned().unwrap_or_default();

        assert_eq!(
            locations.len(),
            rows.len(),
            "{line}:{character}: {} locations on the wire, {} from the method",
            locations.len(),
            rows.len(),
        );
        resolved += rows.len();

        for (location, row) in locations.iter().zip(&rows) {
            // The wire converts the registry key to a `file://` URI because LSP
            // will not accept anything else; this surface hands back the key.
            // `file_uri` is the conversion, so putting the key through it is
            // the comparison — not a second spelling of the same string.
            let expected = fossil_ide::file_uri(&row.uri).map(|u| u.to_string());
            assert_eq!(
                location["uri"].as_str().map(ToString::to_string),
                expected,
                "{line}:{character}: two names for one file",
            );
            assert_eq!(
                location["range"],
                serde_json::to_value(row.range).expect("a Range serialises"),
                "{line}:{character}: two conversions of one byte range",
            );
        }
    }

    assert!(
        resolved > 0,
        "no cursor in hello.fossil resolved a definition, so this test compared nothing"
    );
}

/// The handle is not a URI, and a closed file answers nothing rather than
/// throwing — the three cases a host hits while a buffer is being torn down.
#[test]
fn a_closed_handle_answers_empty_on_all_three() {
    let (mut pg, handle) = direct_side();
    pg.close_file_native(handle).expect("the handle was open");

    assert_eq!(pg.hover_row(handle, 0, 0), None);
    assert!(pg.completion_rows(handle, 0, 0).is_empty());
    assert!(pg.definition_rows(handle, 0, 0).is_empty());
}
