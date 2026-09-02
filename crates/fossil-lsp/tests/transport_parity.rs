//! The transport-parity guard: one LSP server, two wire channels, and until
//! this file existed nothing checked that they answered alike.
//!
//! `fossil-lsp` speaks JSON-RPC over stdio to an editor; `fossil-wasm`'s
//! `lsp_worker` speaks the same JSON-RPC over `postMessage` to a Web Worker.
//! They cannot be collapsed into one binary — two transports, two hosts (a real
//! filesystem versus buffers only), two `#[salsa::db]` structs — and neither is
//! deletable. What IS one thing is the ANSWERS, and the module docs on
//! both sides claimed that agreement in prose. Prose does not notice a defect
//! being fixed on one side:
//!
//! - Both dropped `d.labels` on the floor, and both were fixed separately.
//! - `fossil-wasm`'s drain got a `fossil_base::claimed` guard in `353228c` so a
//!   `.shex` buffer is not parsed as fossil. `fossil-lsp`'s got the identical
//!   guard in `470a13b`, a day later. The same defect, twice, in two files.
//!
//! So this drives BOTH — the real binary over stdio, and `dispatch` in
//! process — with the same buffers and the same cursors, and compares the JSON
//! they write. It restates no answer: there is no expected hover string here, no
//! expected diagnostic count, no capability literal. A copy of the answer cannot
//! notice the answer changing; only the other implementation can.
//!
//! # The differences are DECLARED, not tolerated
//!
//! [`WASM_ONLY_METHODS`] and [`initialize_capabilities`]'s note are the whole
//! list of ways the two are allowed to differ, and each says why. A difference
//! that is not in that list fails here. Adding to the list is a deliberate act
//! with a reason attached; drifting is not.
//!
//! # WHAT THIS CANNOT PROVE
//!
//! - **That either side is right.** It proves they say the same thing. Both
//!   route into the same `fossil-ide` free functions, so a defect there is
//!   equally present on both sides and invisible here. The feature tests
//!   (`lsp_features.rs`, `fossil-wasm/tests/lsp_worker.rs`) are what assert the
//!   content of an answer; this asserts only that there is one answer.
//! - **Anything about the two HOSTS.** The fixture is two buffers and no
//!   filesystem, chosen so the hosts cannot differ. `fossil-lsp` reads unopened
//!   shape documents off disk and `fossil-wasm` has no disk; that difference is
//!   real, is deliberate, and this file is arranged to avoid it rather than to
//!   measure it. A program whose document is only on disk is answered
//!   differently by the two and nothing here says so.
//! - **The `fossil/*` methods.** They exist only on the worker
//!   ([`WASM_ONLY_METHODS`]) and this checks only that the SET of extras is the
//!   declared one — not what any of them does.
//!
//!   This bullet used to read «the `fossil/*` methods, or `didClose`», and the
//!   file then contradicted itself: [`WASM_ONLY_METHODS`]'s own comment says
//!   `didClose` is deliberately NOT in the table because both hosts have it, and
//!   `fossil-lsp`'s `handle_notification` handles `DidCloseTextDocument::METHOD`. The table
//!   is the one that was right, and [`did_close_clears_the_buffer_on_both`] is
//!   the test that holds it — so `didClose` is not an undertested extra, it is a
//!   checked agreement.
//! - **That the wasm worker's transport works.** `dispatch` is called directly;
//!   `start_lsp_worker`, `postMessage` and `serde_wasm_bindgen` are wasm32-only
//!   and untouched here. A defect in the Worker glue is invisible.
//! - **Notification ordering, cancellation, or anything with a clock.** Every
//!   frame is written upfront and drained; this is a comparison of answers, not
//!   of scheduling.

#![cfg(not(target_arch = "wasm32"))]
// The fixture is LITERAL Fossil source with `{User.email}` interpolation holes
// and `type { … }` braces — not Rust format arguments.
#![allow(clippy::literal_string_with_formatting_args)]

mod common;

use common::{did_open, drive, notif, req, text_pos};
use serde_json::{Value, json};

/// The methods the WORKER answers and the native server does not, each with the
/// reason it is not a drift.
///
/// `textDocument/didClose` is deliberately NOT here: it is a notification of the
/// standard LSP surface, both hosts have an open-file table to remove from, and
/// [`did_close_clears_the_buffer_on_both`] holds them to the same behaviour.
const WASM_ONLY_METHODS: &[(&str, &str)] = &[
    (
        "fossil/checkAll",
        "the playground's workspace-wide drain, for a UI panel that lists every \
         open file's diagnostics at once. An editor gets the same information \
         from the per-file publishDiagnostics it already receives, so there is \
         nothing for the native server to answer.",
    ),
    (
        "fossil/registerInferredDescriptor",
        "the browser has no filesystem to DESCRIBE a CSV from, so the host \
         introspects with DuckDB-WASM and pushes the columns in. The native \
         server has a filesystem and goes and looks itself, on `didOpen` and \
         `didChange`, so there is nothing for a client to push. It answered «no \
         descriptor table at all (`LspSystem` returns `None`)» until 2026-08-25, \
         which was a capability gap and is now one method the worker needs \
         because of where it runs.",
    ),
];

// ---------- the fixture ----------
//
// Two buffers and nothing on disk, so the two HOSTS cannot differ (see the
// module docs). The program is the two-identities conflict from
// `fossil-hir`'s `identity.rs`: it is the smallest program that produces a
// diagnostic with LABELS, which is the half both sides dropped on the floor and
// the half whose LSP rendering is the thing most likely to disagree.

const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
User := io.csv(\"u.csv\")
Legacy := io.csv(\"l.csv\")

Users : Person from User
    @subject = \"https://shop.example/user/{User.email}\"
    name = User.name

Imported : Person from Legacy
    @subject = \"https://shop.example/user/{Legacy.account_id}\"
    name = Legacy.full_name
";

/// The shape document, in `ShExC` — the compact form is the one that carries
/// positions, so a label pointing into it has a range to point at.
const DOCUMENT: &str = "\
PREFIX ex: <http://example.org/>

ex:Person {
  ex:name .
}
";

/// A directory that does not exist. `fossil-lsp` resolves an unopened shape
/// document against the program's own URI and reads it from disk; pointing that
/// read at nothing is what makes the OPEN BUFFER the only copy on both sides.
const DIR: &str = "file:///fossil-transport-parity";

fn program_uri() -> String {
    format!("{DIR}/prog.fossil")
}

/// The URI the program's `io.shex("person.shex")` resolves to — beside the
/// program, which is `fossil_hir::documents::registry_key`'s one rule.
fn document_uri() -> String {
    format!("{DIR}/person.shex")
}

/// `(line, character)` of the position `inside` bytes into the first occurrence
/// of `needle`. The fixture is ASCII, so a byte column is a UTF-16 column.
///
/// A needle that is not in the fixture panics rather than defaulting: a silent
/// miss would put both cursors at `0:0`, where the two sides agree trivially and
/// this file would pass forever.
fn pos_of(needle: &str, inside: u32) -> (u32, u32) {
    let idx = PROGRAM
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} is not in the fixture"));
    let before = &PROGRAM[..idx];
    let line = u32::try_from(before.matches('\n').count()).expect("line fits u32");
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let col = u32::try_from(idx - line_start).expect("column fits u32") + inside;
    (line, col)
}

/// The positional requests, in the order the two sides will be asked them.
///
/// One entry per shared request handler. `initialize`, `shutdown` and the two
/// text-sync notifications are exercised by the surrounding conversation rather
/// than listed here.
fn probes() -> Vec<(&'static str, Value)> {
    let uri = program_uri();
    // Inside the identity template of the first mapping — an interpolated
    // string synthesises a reference without needing an input descriptor,
    // which neither host has for `u.csv`.
    let (hover_l, hover_c) = pos_of("@subject = \"https://shop.example/user/{User.email}\"", 14);
    // On the shape name in the first header: it is declared in the document, so
    // this is the cross-file, cross-language jump.
    let (def_l, def_c) = pos_of("Users : Person from User", 9);
    // Inside the first mapping's body.
    let (comp_l, comp_c) = pos_of("name = User.name", 12);
    // A range over the second mapping's `@subject` line — the one the identity
    // diagnostic underlines, so the code-action handler is asked about a range
    // that has a diagnostic on it.
    let (ca_l, ca_c) = pos_of(
        "@subject = \"https://shop.example/user/{Legacy.account_id}\"",
        0,
    );
    vec![
        ("textDocument/hover", text_pos(&uri, hover_l, hover_c)),
        ("textDocument/definition", text_pos(&uri, def_l, def_c)),
        ("textDocument/completion", text_pos(&uri, comp_l, comp_c)),
        (
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        ),
        (
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        ),
        (
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": ca_l, "character": ca_c },
                    "end": { "line": ca_l, "character": ca_c + 8 }
                },
                "context": { "diagnostics": [] }
            }),
        ),
    ]
}

// ---------- driving the two sides ----------

/// What one side answered: the `result` of every probe in [`probes()`] order,
/// the advertised capabilities, and the `diagnostics` array it published for the
/// program.
struct Answers {
    capabilities: Value,
    results: Vec<Value>,
    published: Value,
}

/// Drive the real binary over stdio, exactly as an editor does.
fn native() -> Answers {
    let uri = program_uri();
    let doc = document_uri();
    let mut frames = vec![
        req(
            1,
            "initialize",
            json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", json!({})),
        // The document FIRST: registered under the key the program resolves to,
        // so neither host ever looks for it anywhere else.
        did_open(&doc, DOCUMENT),
        did_open(&uri, PROGRAM),
    ];
    for (i, (method, params)) in probes().into_iter().enumerate() {
        frames.push(req(
            i64::try_from(i).expect("probe index fits i64") + 10,
            method,
            params,
        ));
    }
    frames.push(req(99, "shutdown", Value::Null));
    frames.push(notif("exit", Value::Null));

    let t = drive(&frames);
    let published = t
        .published(&uri)
        .last()
        .and_then(|n| n.pointer("/params/diagnostics"))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the native server published no diagnostics for {uri}; stderr: {}",
                t.stderr
            )
        });
    Answers {
        capabilities: t.by_id(1)["result"]["capabilities"].clone(),
        results: (0..probes().len())
            .map(|i| {
                t.by_id(i64::try_from(i).expect("probe index fits i64") + 10)["result"].clone()
            })
            .collect(),
        published,
    }
}

/// Drive the worker's `dispatch` in process — the same JSON envelopes, minus
/// the framing and the `postMessage` hop.
fn worker() -> Answers {
    let mut pg = fossil_wasm::FossilPlayground::new();
    let uri = program_uri();
    let doc = document_uri();

    let capabilities = dispatch(&mut pg, 1, "initialize", &json!({}))["capabilities"].clone();
    notify(&mut pg, "initialized", &json!({}));
    notify(
        &mut pg,
        "textDocument/didOpen",
        &open_params(&doc, DOCUMENT),
    );
    let published = notify(&mut pg, "textDocument/didOpen", &open_params(&uri, PROGRAM))
        .into_iter()
        .filter(|n| n.pointer("/params/uri").and_then(Value::as_str) == Some(uri.as_str()))
        .next_back()
        .and_then(|n| n.pointer("/params/diagnostics").cloned())
        .unwrap_or_else(|| panic!("the worker published no diagnostics for {uri}"));

    let results = probes()
        .into_iter()
        .enumerate()
        .map(|(i, (method, params))| {
            dispatch(
                &mut pg,
                i64::try_from(i).expect("probe index fits i64") + 10,
                method,
                &params,
            )
        })
        .collect();

    Answers {
        capabilities,
        results,
        published,
    }
}

fn open_params(uri: &str, text: &str) -> Value {
    json!({ "textDocument": { "uri": uri, "languageId": "fossil", "version": 1, "text": text } })
}

/// One request through the worker; the `result`, or a panic naming the error.
fn dispatch(
    pg: &mut fossil_wasm::FossilPlayground,
    id: i64,
    method: &str,
    params: &Value,
) -> Value {
    let out = fossil_wasm::__dispatch_for_test(
        pg,
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
    );
    let resp = out
        .response
        .unwrap_or_else(|| panic!("the worker did not respond to {method}"));
    assert!(
        resp.error.is_none(),
        "the worker errored on {method}: {:?}",
        resp.error
    );
    resp.result.unwrap_or(Value::Null)
}

/// One notification through the worker; the `publishDiagnostics` it emitted.
fn notify(pg: &mut fossil_wasm::FossilPlayground, method: &str, params: &Value) -> Vec<Value> {
    let out = fossil_wasm::__dispatch_for_test(
        pg,
        json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    );
    assert!(
        out.response.is_none(),
        "{method} is a notification and must draw no response"
    );
    out.diagnostics
}

// ---------- the guards ----------

/// The two advertise the same capabilities.
///
/// The whole object, not a list of keys: a trigger character added on one side,
/// a `range: true` on the other, or a legend that grew a token type are each a
/// client behaving differently against the two transports.
///
/// DECLARED DIFFERENCE: the `initialize` RESULT differs by `serverInfo`, which
/// the worker sends and `lsp-server`'s `Connection::initialize` does not
/// construct. That is the transport crate's envelope and not a fossil answer, so
/// this compares `result.capabilities` rather than `result`.
#[test]
fn initialize_capabilities() {
    let (n, w) = (native(), worker());
    assert_eq!(
        n.capabilities, w.capabilities,
        "the two transports advertise different capabilities; native left, worker right"
    );
}

/// The same buffer and the same cursor produce the same answer, for every
/// request handler both sides have.
#[test]
fn every_shared_request_answers_alike() {
    let (n, w) = (native(), worker());
    let methods: Vec<&str> = probes().iter().map(|(m, _)| *m).collect();
    assert_eq!(
        n.results.len(),
        methods.len(),
        "the native side answered a different number of probes than were sent"
    );
    let mut differ: Vec<&str> = Vec::new();
    for (i, method) in methods.iter().enumerate() {
        if n.results[i] != w.results[i] {
            differ.push(method);
        }
    }
    assert!(
        differ.is_empty(),
        "{} of {} shared handlers answer differently: {differ:?}\n\n{}",
        differ.len(),
        methods.len(),
        methods
            .iter()
            .enumerate()
            .filter(|(i, _)| n.results[*i] != w.results[*i])
            .map(|(i, m)| format!(
                "--- {m}\nnative: {}\nworker: {}\n",
                n.results[i], w.results[i]
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// `textDocument/publishDiagnostics` carries the same diagnostics.
///
/// This is the one both sides got wrong separately — `d.labels` dropped on the
/// floor twice, the `claimed` guard added twice a day apart — so it is compared
/// as JSON and not as a count. The notification's method and `params.uri` are
/// the same by construction; what is asserted is `params.diagnostics`, which is
/// the payload an editor renders.
#[test]
fn published_diagnostics_are_the_same_json() {
    let (n, w) = (native(), worker());
    assert_ne!(
        n.published,
        json!([]),
        "the fixture must produce a diagnostic or this test compares two empty \
         arrays and passes forever"
    );
    assert_eq!(
        n.published, w.published,
        "the two transports publish different diagnostics for one buffer; \
         native left, worker right"
    );
}

/// A diagnostic in the fixture carries `relatedInformation`, and both sides
/// spell it the way LSP does.
///
/// Without this, [`published_diagnostics_are_the_same_json`] would still pass if
/// both sides dropped labels again — two identical wrong answers agree. The
/// fixture is chosen so exactly this field is populated; if the identity check
/// stops reporting labels, this goes red and says so.
#[test]
fn the_labelled_diagnostic_reaches_both_as_related_information() {
    for (side, published) in [
        ("native", native().published),
        ("worker", worker().published),
    ] {
        let with_related: Vec<&Value> = published
            .as_array()
            .unwrap_or_else(|| panic!("{side}: params.diagnostics is not an array: {published}"))
            .iter()
            .filter(|d| d.get("relatedInformation").is_some())
            .collect();
        assert!(
            !with_related.is_empty(),
            "{side}: no diagnostic carries `relatedInformation`, the LSP spelling \
             of a report about two places. The fixture's identity conflict \
             underlines both `@subject` lines. Got: {published:#}"
        );
        for d in with_related {
            let entries = d["relatedInformation"]
                .as_array()
                .unwrap_or_else(|| panic!("{side}: relatedInformation is not an array: {d}"));
            for e in entries {
                assert!(
                    e.pointer("/location/uri").is_some_and(Value::is_string)
                        && e.pointer("/location/range").is_some()
                        && e.get("message").is_some_and(Value::is_string),
                    "{side}: a relatedInformation entry must be \
                     `{{ location: {{ uri, range }}, message }}`; got {e:#}"
                );
            }
        }
    }
}

/// The set of methods each side answers is the same, modulo
/// [`WASM_ONLY_METHODS`].
///
/// Probed rather than read off the source: each side is asked every method
/// either one routes, and a `MethodNotFound` is the answer meaning «not mine».
/// A handler added to one transport and forgotten on the other lands here.
#[test]
fn the_method_sets_differ_only_where_declared() {
    const METHOD_NOT_FOUND: i64 = -32601;
    let extras: Vec<&str> = WASM_ONLY_METHODS.iter().map(|(m, _)| *m).collect();
    let shared: Vec<&str> = probes().iter().map(|(m, _)| *m).collect();
    let every: Vec<&str> = shared
        .iter()
        .copied()
        .chain(extras.iter().copied())
        .collect();

    // Native: send them all and see which come back MethodNotFound. The params
    // are empty objects — a handler that decodes them badly still answers, and
    // «answers at all» is the question here.
    let mut frames = vec![
        req(
            1,
            "initialize",
            json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", json!({})),
        did_open(&program_uri(), PROGRAM),
    ];
    for (i, m) in every.iter().enumerate() {
        frames.push(req(
            i64::try_from(i).expect("index fits i64") + 20,
            m,
            json!({ "textDocument": { "uri": program_uri() } }),
        ));
    }
    frames.push(req(99, "shutdown", Value::Null));
    frames.push(notif("exit", Value::Null));
    let t = drive(&frames);

    let mut pg = fossil_wasm::FossilPlayground::new();
    notify(
        &mut pg,
        "textDocument/didOpen",
        &open_params(&program_uri(), PROGRAM),
    );

    let mut native_missing: Vec<&str> = Vec::new();
    let mut worker_missing: Vec<&str> = Vec::new();
    for (i, m) in every.iter().enumerate() {
        let resp = t.by_id(i64::try_from(i).expect("index fits i64") + 20);
        if resp.pointer("/error/code").and_then(Value::as_i64) == Some(METHOD_NOT_FOUND) {
            native_missing.push(m);
        }
        let out = fossil_wasm::__dispatch_for_test(
            &mut pg,
            json!({ "jsonrpc": "2.0", "id": 1, "method": m,
                    "params": { "textDocument": { "uri": program_uri() } } }),
        );
        let code = out
            .response
            .and_then(|r| r.error)
            .map(|e| i64::from(e.code));
        if code == Some(METHOD_NOT_FOUND) {
            worker_missing.push(m);
        }
    }

    assert!(
        worker_missing.is_empty(),
        "the worker answers MethodNotFound for {worker_missing:?}, which this \
         guard's own tables say it routes"
    );
    assert_eq!(
        native_missing, extras,
        "the native server answers a different set of methods than declared. \
         Everything in WASM_ONLY_METHODS must be absent natively (with the \
         reason recorded there) and nothing else may be."
    );
}

/// `textDocument/didClose` removes the buffer from both open-file tables and
/// clears the editor's squiggles.
///
/// The worker has handled it since it was written; the native server did not,
/// so a closed file stayed in `LspState::files` forever — still the workspace
/// goto-def and completion resolve against, and still carrying whatever
/// diagnostics were last published for it, with no notification to clear them.
///
/// Asserted through the wire on both sides: an empty `diagnostics` array for the
/// closed URI is the LSP's way of saying «nothing here any more», and a
/// subsequent request for that buffer answers `null` because it is gone.
#[test]
fn did_close_clears_the_buffer_on_both() {
    let uri = program_uri();

    let t = drive(&[
        req(
            1,
            "initialize",
            json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", json!({})),
        did_open(&uri, PROGRAM),
        notif(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        ),
        req(
            10,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        ),
        req(99, "shutdown", Value::Null),
        notif("exit", Value::Null),
    ]);
    let native_last = t
        .published(&uri)
        .last()
        .and_then(|n| n.pointer("/params/diagnostics"))
        .cloned()
        .unwrap_or_else(|| panic!("no publishDiagnostics at all for {uri}"));
    assert_eq!(
        native_last,
        json!([]),
        "the native server's LAST word on a closed buffer must be an empty \
         diagnostics array, or the squiggles never go away"
    );
    assert_eq!(
        t.by_id(10)["result"],
        Value::Null,
        "a request for a closed buffer must answer null: it is not open"
    );

    let mut pg = fossil_wasm::FossilPlayground::new();
    notify(&mut pg, "textDocument/didOpen", &open_params(&uri, PROGRAM));
    let closed = notify(
        &mut pg,
        "textDocument/didClose",
        &json!({ "textDocument": { "uri": uri } }),
    );
    let worker_last = closed
        .last()
        .and_then(|n| n.pointer("/params/diagnostics").cloned())
        .unwrap_or_else(|| panic!("the worker published nothing on didClose: {closed:?}"));
    assert_eq!(
        worker_last,
        json!([]),
        "and the worker's, for the same reason"
    );
    assert_eq!(
        dispatch(
            &mut pg,
            11,
            "textDocument/documentSymbol",
            &json!({ "textDocument": { "uri": uri } }),
        ),
        Value::Null,
        "a request for a closed buffer must answer null on the worker too"
    );
}
