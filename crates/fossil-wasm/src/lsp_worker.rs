//! LSP-over-postMessage transport. The Web Worker host calls
//! [`start_lsp_worker`] exactly once; that installs an `onmessage`
//! handler that drains LSP JSON-RPC requests off the postMessage
//! channel, dispatches them to the [`FossilPlayground`] + `fossil-ide`,
//! and posts the JSON-RPC response back. Notifications produce no
//! response but trigger `textDocument/publishDiagnostics`.
//!
//! Architectural seam: ADR-0024 (`fossil-wasm` IS the LSP worker — NOT a
//! recompiled `fossil-lsp`; the cfg-tripwire on `fossil-lsp` stays). The
//! Phase-6 LSP capabilities + handlers (`fossil-lsp/src/main.rs`) are the
//! 1:1 model; the only difference is the wire channel (postMessage vs
//! stdio).
//!
//! ## Cancellation
//!
//! Same revision-bump trigger as `fossil-lsp` (ADR-0022) — no new
//! mechanism. `update_file` calls `set_text` via the Salsa `Setter`; any
//! in-flight analysis observes the new revision at its next cooperative
//! checkpoint.
//!
//! ## Per-file diagnostics drain (B3 fix — mandatory)
//!
//! [`publish_diagnostics`] takes `(&FossilPlayground, &str)` and returns
//! `Option<serde_json::Value>` — drains the Salsa accumulator for the SINGLE
//! file named by `uri` (never all open files) and constructs the LSP
//! `textDocument/publishDiagnostics` notification. The caller (the
//! `onmessage` closure) posts it to the Worker scope. Returns `None` if the
//! URI is not currently open.

// `pub(crate)` is the deliberate visibility on the wire types + `dispatch` +
// `publish_diagnostics` (the parent module re-exports `start_lsp_worker`,
// not these; tests reach them via the `__dispatch_for_test` hook). Clippy's
// `redundant_pub_crate` nursery lint suggests `pub` since the parent module
// is `pub(crate)`, but then `unreachable_pub` complains the other way —
// `pub(crate)` plus the allow is the wedge (matches `workspace.rs`).
#![allow(clippy::redundant_pub_crate)]

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use lsp_types::{
    CodeActionOrCommand, CodeActionProviderCapability, CompletionOptions,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolResponse, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, Location, MarkupContent, MarkupKind, OneOf, Position, Range,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use crate::{CheckRow, FossilPlayground};

// ---------- JSON-RPC wire types ----------

/// Decoded incoming LSP request or notification.
///
/// `id` is `None` for notifications. `params` is `serde_json::Value::Null`
/// when omitted on the wire (the `#[serde(default)]` covers that case).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct LspRequest {
    #[allow(dead_code)] // accepted for wire-completeness; we always emit "2.0".
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Outgoing LSP response. `result` and `error` are mutually exclusive per the
/// JSON-RPC 2.0 spec; serializer skips whichever is `None`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct LspResponse {
    pub jsonrpc: &'static str,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LspError>,
}

/// LSP / JSON-RPC error object. `code` is the JSON-RPC integer error code
/// (e.g. `-32601` `MethodNotFound`, `-32602` `InvalidParams`).
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct LspError {
    pub code: i32,
    pub message: String,
}

/// Result of [`dispatch`]: optional response (None for notifications) plus
/// any `textDocument/publishDiagnostics` notifications the caller must post
/// to the Worker scope separately. Per-file drain — one notification per
/// affected URI (B3 fix per the 07-03 plan revision).
pub(crate) struct DispatchOutput {
    pub response: Option<LspResponse>,
    pub diagnostics: Vec<serde_json::Value>,
}

// ---------- Worker entrypoint ----------

/// Worker entrypoint. Idempotent in spirit — calling twice replaces the
/// previous `onmessage` handler (the leaked Closure from the first call
/// stays alive but is no longer referenced by the global scope).
///
/// Must be called from a `DedicatedWorker` global scope; returns a JS error
/// otherwise (e.g. if called from the main thread).
///
/// # Errors
///
/// Returns a JS error when not running inside a `DedicatedWorker` scope.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn start_lsp_worker() -> Result<(), JsError> {
    console_error_panic_hook::set_once();
    let global = js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .map_err(|_| JsError::new("start_lsp_worker must be called from a DedicatedWorker"))?;
    let pg = Rc::new(RefCell::new(FossilPlayground::new()));
    let global_for_handler = global.clone();
    let pg_for_handler = pg.clone();

    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
        let data = evt.data();
        let req: LspRequest = match serde_wasm_bindgen::from_value(data) {
            Ok(r) => r,
            Err(_) => {
                // Malformed message — drop. The LSP spec says the server
                // SHOULD return a parse error response, but a notification-
                // shaped malformed payload has no id to address it to. Log
                // would be useful here but `eprintln!` in a Worker is silent;
                // a console.error via web_sys::console would work but adds
                // surface area for a corner case. Dropping is fine.
                return;
            }
        };
        let DispatchOutput {
            response,
            diagnostics,
        } = dispatch(&mut pg_for_handler.borrow_mut(), req);
        if let Some(resp) = response {
            let _ = post(&global_for_handler, &resp);
        }
        for notif in diagnostics {
            let _ = post(&global_for_handler, &notif);
        }
    });

    global.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    Ok(())
}

/// Native stub of [`start_lsp_worker`]. The Worker entrypoint only makes
/// sense inside a `DedicatedWorker` scope (wasm32); native builds get a
/// no-op error so the crate still compiles for `cargo test`.
///
/// # Errors
///
/// Always returns an error on non-wasm32 targets.
#[cfg(not(target_arch = "wasm32"))]
pub fn start_lsp_worker() -> Result<(), JsError> {
    Err(JsError::new(
        "start_lsp_worker is only available on wasm32 targets",
    ))
}

#[cfg(target_arch = "wasm32")]
fn post<T: serde::Serialize>(g: &DedicatedWorkerGlobalScope, v: &T) -> Result<(), JsError> {
    let js = serde_wasm_bindgen::to_value(v).map_err(JsError::from)?;
    g.post_message(&js)
        .map_err(|e| JsError::new(&format!("post_message: {e:?}")))?;
    Ok(())
}

// ---------- Dispatch ----------

/// Route one decoded LSP message to the right handler. Returns the response
/// (None for notifications) plus any `publishDiagnostics` notifications the
/// caller must post out-of-band. Per-file drain (B3 fix).
///
/// Test-reachable from native via `__dispatch_for_test` in the crate root.
pub(crate) fn dispatch(pg: &mut FossilPlayground, req: LspRequest) -> DispatchOutput {
    let LspRequest {
        id, method, params, ..
    } = req;
    let id_val = id.unwrap_or(serde_json::Value::Null);

    // ----- Notifications (no response) -----
    if let Some(diagnostics) = handle_notification(pg, &method, params.clone()) {
        return DispatchOutput {
            response: None,
            diagnostics,
        };
    }

    // ----- Requests (response required) -----
    let result: Result<serde_json::Value, LspError> = match method.as_str() {
        "initialize" => Ok(serde_json::json!({
            "capabilities": server_capabilities(),
            "serverInfo": { "name": "fossil-wasm-lsp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "shutdown" => Ok(serde_json::Value::Null),
        "textDocument/hover" => handle_hover(pg, params),
        "textDocument/definition" => handle_definition(pg, params),
        "textDocument/completion" => handle_completion(pg, params),
        "textDocument/documentSymbol" => handle_document_symbol(pg, params),
        "textDocument/semanticTokens/full" => handle_semantic_tokens_full(pg, params),
        "textDocument/codeAction" => handle_code_action(pg, params),
        // Custom fossil/* methods (B2 — wired in 07-06; the dispatch routes
        // are listed here so the plan-stated method-not-found surface is
        // accurate. 07-06 fills in the handlers.)
        "fossil/compileFile" => handle_compile_file(pg, params),
        "fossil/checkAll" => handle_check_all(pg),
        "fossil/setTargetShex" => handle_set_target_shex(pg, params),
        other => Err(LspError {
            code: -32601, // MethodNotFound
            message: format!("method not found: {other}"),
        }),
    };

    let resp = match result {
        Ok(v) => LspResponse {
            jsonrpc: "2.0",
            id: id_val,
            result: Some(v),
            error: None,
        },
        Err(e) => LspResponse {
            jsonrpc: "2.0",
            id: id_val,
            result: None,
            error: Some(e),
        },
    };
    DispatchOutput {
        response: Some(resp),
        diagnostics: Vec::new(),
    }
}

/// Handle the four LSP notifications (`initialized`, `exit`, didOpen,
/// didChange, didClose). Returns `Some(diagnostics)` when `method` was a
/// notification — the per-file `publishDiagnostics` notifications the
/// caller must post (empty for `initialized` / `exit`). Returns `None`
/// when `method` is not a notification — caller falls through to request
/// handling.
fn handle_notification(
    pg: &mut FossilPlayground,
    method: &str,
    params: serde_json::Value,
) -> Option<Vec<serde_json::Value>> {
    let mut diagnostics: Vec<serde_json::Value> = Vec::new();
    match method {
        "initialized" | "exit" => Some(diagnostics),
        "textDocument/didOpen" => {
            if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(params) {
                let uri = p.text_document.uri.to_string();
                let _ = pg.open_file_native(uri.clone(), p.text_document.text);
                if let Some(n) = publish_diagnostics(pg, &uri) {
                    diagnostics.push(n);
                }
            }
            Some(diagnostics)
        }
        "textDocument/didChange" => {
            if let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(params) {
                let uri = p.text_document.uri.to_string();
                if let Some(handle) = pg.lookup_handle_by_uri(&uri) {
                    // LSP `TextDocumentSyncKind::FULL` — the last content
                    // change carries the full document text; intermediate
                    // changes (if any) are dropped (matches fossil-lsp 06-09).
                    if let Some(change) = p.content_changes.into_iter().next_back() {
                        let _ = pg.update_file_native(handle, change.text);
                    }
                }
                if let Some(n) = publish_diagnostics(pg, &uri) {
                    diagnostics.push(n);
                }
            }
            Some(diagnostics)
        }
        "textDocument/didClose" => {
            if let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(params) {
                let uri = p.text_document.uri.to_string();
                if let Some(handle) = pg.lookup_handle_by_uri(&uri) {
                    let _ = pg.close_file_native(handle);
                }
                // Per the LSP spec: on file close, publish an empty
                // diagnostics list so the client clears any remaining
                // squigglies for that URI.
                diagnostics.push(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": { "uri": uri, "diagnostics": [] }
                }));
            }
            Some(diagnostics)
        }
        _ => None,
    }
}

// ---------- Capability surface ----------

fn server_capabilities() -> serde_json::Value {
    // Mirror fossil-lsp/src/main.rs `server_capabilities` 1:1 — construct an
    // `lsp_types::ServerCapabilities` and serialize it, so the shape stays
    // in sync with the typed Phase-6 surface.
    let caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), ":".into()]),
            ..Default::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: fossil_ide::semantic_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                ..Default::default()
            },
        )),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        ..Default::default()
    };
    serde_json::to_value(&caps).unwrap_or(serde_json::Value::Null)
}

// ---------- Per-file diagnostics drain (B3 fix) ----------

/// Drain Salsa diagnostics for the SINGLE file identified by `uri` (per-file
/// drain — NOT all open files). Returns the constructed
/// `textDocument/publishDiagnostics` notification as a `serde_json::Value`;
/// the dispatch caller posts it to the `DedicatedWorkerGlobalScope`. Returns
/// `None` if the URI is not currently open.
pub(crate) fn publish_diagnostics(pg: &FossilPlayground, uri: &str) -> Option<serde_json::Value> {
    let handle = pg.lookup_handle_by_uri(uri)?;
    let rows: Vec<CheckRow> = pg.diagnostics_for_rows(handle)?;
    Some(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": rows }
    }))
}

// ---------- Request handlers ----------

fn handle_hover(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    let p: HoverParams = serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let uri = p
        .text_document_position_params
        .text_document
        .uri
        .to_string();
    let pos = p.text_document_position_params.position;
    let Some(file) = pg.lookup_file_by_uri(&uri) else {
        return Ok(serde_json::Value::Null);
    };
    let info = fossil_ide::hover_bidirectional(pg.hir_db(), file, pos.line, pos.character);
    let payload = info.map(|hi| {
        let index = fossil_ide::line_index(pg.base_db(), file);
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hi.markdown,
            }),
            range: Some(byte_range_to_lsp_range(&index, hi.range)),
        }
    });
    Ok(serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null))
}

fn handle_definition(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    let p: TextDocumentPositionParams =
        serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let uri = p.text_document.uri.to_string();
    let pos = p.position;
    let Some(file) = pg.lookup_file_by_uri(&uri) else {
        return Ok(serde_json::Value::Null);
    };
    let files = pg.open_source_files();
    let targets = fossil_ide::goto_definition(pg.base_db(), &files, file, pos.line, pos.character);
    let locations: Vec<Location> = targets
        .into_iter()
        .filter_map(|t| {
            let path = t.file.path(pg.base_db()).clone();
            let uri = uri_from_str_maybe(&path)?;
            let index = fossil_ide::line_index(pg.base_db(), t.file);
            Some(Location {
                uri,
                range: byte_range_to_lsp_range(&index, t.range),
            })
        })
        .collect();
    let payload = (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations));
    Ok(serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null))
}

fn handle_completion(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    // CompletionParams's full shape is verbose; we only need the
    // `text_document_position` field, so deserialize that subset.
    #[derive(serde::Deserialize)]
    struct CompletionParamsSubset {
        #[serde(rename = "textDocument")]
        text_document: lsp_types::TextDocumentIdentifier,
        position: Position,
    }
    let p: CompletionParamsSubset =
        serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let uri = p.text_document.uri.to_string();
    let Some(file) = pg.lookup_file_by_uri(&uri) else {
        return Ok(serde_json::Value::Null);
    };
    let files = pg.open_source_files();
    let items = fossil_ide::completions(
        pg.hir_db(),
        &files,
        file,
        p.position.line,
        p.position.character,
    );
    Ok(serde_json::to_value(&items).unwrap_or(serde_json::Value::Null))
}

fn handle_document_symbol(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    #[derive(serde::Deserialize)]
    struct DocumentSymbolParamsSubset {
        #[serde(rename = "textDocument")]
        text_document: lsp_types::TextDocumentIdentifier,
    }
    let p: DocumentSymbolParamsSubset =
        serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let uri = p.text_document.uri.to_string();
    let Some(file) = pg.lookup_file_by_uri(&uri) else {
        return Ok(serde_json::Value::Null);
    };
    let symbols = fossil_ide::document_symbols(pg.base_db(), file);
    let payload = DocumentSymbolResponse::Nested(symbols);
    Ok(serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null))
}

fn handle_semantic_tokens_full(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    #[derive(serde::Deserialize)]
    struct SemanticTokensParamsSubset {
        #[serde(rename = "textDocument")]
        text_document: lsp_types::TextDocumentIdentifier,
    }
    let p: SemanticTokensParamsSubset =
        serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let uri = p.text_document.uri.to_string();
    let Some(file) = pg.lookup_file_by_uri(&uri) else {
        return Ok(serde_json::Value::Null);
    };
    let data = fossil_ide::semantic_tokens(pg.base_db(), file);
    let tokens: Vec<lsp_types::SemanticToken> = data
        .chunks_exact(5)
        .map(|c| lsp_types::SemanticToken {
            delta_line: c[0],
            delta_start: c[1],
            length: c[2],
            token_type: c[3],
            token_modifiers_bitset: c[4],
        })
        .collect();
    let payload = lsp_types::SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: tokens,
    });
    Ok(serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null))
}

fn handle_code_action(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    #[derive(serde::Deserialize)]
    struct CodeActionParamsSubset {
        #[serde(rename = "textDocument")]
        text_document: lsp_types::TextDocumentIdentifier,
        range: Range,
    }
    let p: CodeActionParamsSubset =
        serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let uri = p.text_document.uri.to_string();
    let Some(file) = pg.lookup_file_by_uri(&uri) else {
        return Ok(serde_json::Value::Null);
    };
    // Re-derive the structured `Diagnostic` carriers (the wire form drops
    // `did_you_mean` / `suggestion_source`). Mirrors fossil-lsp 06-09.
    let diagnostics = pg.drain_diagnostics_for_file(file);
    let actions = fossil_ide::code_actions(pg.base_db(), file, p.range, &diagnostics);
    let payload: Vec<CodeActionOrCommand> = actions
        .into_iter()
        .map(CodeActionOrCommand::CodeAction)
        .collect();
    Ok(serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null))
}

// ---------- Custom fossil/* methods (B2) ----------

fn handle_compile_file(
    pg: &FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    #[derive(serde::Deserialize)]
    struct CompileFileParams {
        uri: String,
    }
    let p: CompileFileParams = serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    let handle = pg.lookup_handle_by_uri(&p.uri).ok_or_else(|| LspError {
        code: -32602,
        message: format!("unknown uri: {}", p.uri),
    })?;
    pg.compile_file_result(handle)
        .map(|r| serde_json::to_value(&r).unwrap_or(serde_json::Value::Null))
        .map_err(|e| LspError {
            code: -32000,
            message: e.to_string(),
        })
}

// Uniform handler signature for the dispatch routing table — `check_all`
// cannot fail today, but the `Result` keeps the handler-shape symmetry with
// the rest of the routes.
#[allow(clippy::unnecessary_wraps)]
fn handle_check_all(pg: &FossilPlayground) -> Result<serde_json::Value, LspError> {
    let rows = pg.check_rows();
    Ok(serde_json::to_value(&rows).unwrap_or(serde_json::Value::Null))
}

fn handle_set_target_shex(
    pg: &mut FossilPlayground,
    params: serde_json::Value,
) -> Result<serde_json::Value, LspError> {
    #[derive(serde::Deserialize)]
    struct SetTargetShexParams {
        text: String,
    }
    let p: SetTargetShexParams = serde_json::from_value(params).map_err(|e| invalid_params(&e))?;
    pg.set_target_shex_native(&p.text).map_err(|e| LspError {
        code: -32000,
        message: e,
    })?;
    Ok(serde_json::Value::Null)
}

// ---------- Helpers ----------

fn invalid_params(e: &serde_json::Error) -> LspError {
    LspError {
        code: -32602,
        message: format!("invalid params: {e}"),
    }
}

fn byte_range_to_lsp_range(index: &fossil_ide::LineIndex, range: std::ops::Range<u32>) -> Range {
    Range {
        start: utf16_to_position(fossil_ide::offset_to_lsp_position(index, range.start)),
        end: utf16_to_position(fossil_ide::offset_to_lsp_position(index, range.end)),
    }
}

const fn utf16_to_position(p: fossil_ide::Utf16Position) -> Position {
    Position {
        line: p.line,
        character: p.character,
    }
}

/// Parse a path or URI string into an `lsp_types::Uri`. Mirrors fossil-lsp's
/// `UriExt::from_str_maybe` — `SourceFile.path` may be either a `file://`
/// URI (from the LSP / Worker) or a bare path (from a test).
fn uri_from_str_maybe(s: &str) -> Option<lsp_types::Uri> {
    use std::str::FromStr as _;
    if s.contains("://") {
        lsp_types::Uri::from_str(s).ok()
    } else if let Some(rest) = s.strip_prefix('/') {
        lsp_types::Uri::from_str(&format!("file:///{rest}")).ok()
    } else {
        lsp_types::Uri::from_str(&format!("file://{s}")).ok()
    }
}
