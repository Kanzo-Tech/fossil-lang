//! `fossil-lsp` — the native LSP transport (Phase 6 LSP-01).
//!
//! Wires the LSP server from day 1 (per ROADMAP.md Phase 1 success
//! criterion #4 + RESEARCH.md commit #9 of 10) so Fossil avoids the fase-4
//! trap that killed the predecessor project. Phase 6 LSP-01 grows the stub
//! into the full transport: it advertises and serves the complete capability
//! set, each handler being a thin adapter over a `fossil-ide` free function:
//!
//! - `initialize` → [`ServerCapabilities`] advertising `TextDocumentSync::FULL`,
//!   `hover_provider`, `definition_provider`, `completion_provider` (trigger
//!   characters `.` and `:`), `document_symbol_provider`, `code_action_provider`,
//!   and `semantic_tokens_provider` (full-document, legend from
//!   [`fossil_ide::semantic_legend`]).
//! - `textDocument/didOpen` → records `(uri, SourceFile)` and publishes the
//!   real `parse → def_map → typecheck` diagnostics.
//! - `textDocument/didChange` → mutates the SAME `SourceFile` via the Salsa
//!   `Setter` (`set_text`), which BUMPS THE REVISION — the real cancellation
//!   trigger (ADR-0022; NOT a fictional `db.cancel_pending()`) — then republishes
//!   diagnostics.
//! - `textDocument/hover` → [`fossil_ide::hover_bidirectional`] (the LSP db
//!   carries an `Arc<OutputDescriptorKind>` so the target-side `ShEx` type is
//!   reachable when a schema is loaded).
//! - `textDocument/definition` → [`fossil_ide::goto_definition`] → `Location`s.
//! - `textDocument/completion` → [`fossil_ide::completions`].
//! - `textDocument/documentSymbol` → [`fossil_ide::document_symbols`].
//! - `textDocument/semanticTokens/full` → [`fossil_ide::semantic_tokens`].
//! - `textDocument/codeAction` → [`fossil_ide::code_actions`].
//! - `shutdown` (request) / `exit` (notification) → `Connection::handle_shutdown`.
//!
//! Per ADR-0001 (`lsp-server`, NOT `tower-lsp`) and CLAUDE.md "Hard Rules"
//! (`fossil-lsp` is native-only). The dispatch loop pattern is derived from
//! `rust-analyzer/lsp-server/examples/goto_def.rs` (verified via `WebFetch`
//! per `01-RESEARCH.md` Example 16).

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-lsp is native-only (lsp-server uses crossbeam-channel + stdio); \
     the playground exposes LSP features via fossil-wasm directly, not through this binary"
);

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use fossil_base::{Diagnostic, Files, NativeSystem, Severity, SourceFile, Span, System};
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_hir::HirDb;
use fossil_ide::{LineIndex, Utf16Position};
use lsp_server::{Connection, ErrorCode, ExtractError, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest,
    Request as _, SemanticTokensFullRequest,
};
use lsp_types::{
    CodeActionProviderCapability, CompletionOptions, Diagnostic as LspDiagnostic,
    DiagnosticSeverity, DocumentSymbolResponse, GotoDefinitionResponse, Hover, HoverContents,
    HoverProviderCapability, Location, MarkupContent, MarkupKind, OneOf, Position,
    PublishDiagnosticsParams, Range, SemanticTokens, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
    WorkDoneProgressOptions,
};

/// The LSP database.
///
/// A `#[salsa::db]` struct (it cannot be `FossilDb` because `fossil-hir` only
/// `impl HirDb for FossilDb` under `#[cfg(test)]`) that carries the Salsa
/// runtime + the host [`System`] + an `Arc<OutputDescriptorKind>` it returns
/// from the [`HirDb`] override. This is the production realisation of the
/// host-wrapper contract the 06-05 `ShExHostDb` integration test stands in for:
/// it lets [`fossil_ide::hover_bidirectional`] / [`fossil_ide::completions`]
/// read the host's output descriptor so the target-side `ShEx` type/properties
/// are reachable when a schema is loaded (`AcceptAll` → source-only otherwise).
#[salsa::db]
#[derive(Clone)]
struct LspDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    descriptor: Arc<OutputDescriptorKind>,
}

impl std::fmt::Debug for LspDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspDb").finish_non_exhaustive()
    }
}

#[salsa::db]
impl salsa::Database for LspDb {}

#[salsa::db]
impl fossil_base::Db for LspDb {
    fn system(&self) -> &dyn System {
        &*self.system
    }
    fn files(&self) -> &Files {
        &self.files
    }
}

impl HirDb for LspDb {
    fn output_descriptor_kind(&self) -> &OutputDescriptorKind {
        &self.descriptor
    }
}

impl LspDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            system: Arc::new(NativeSystem::default()),
            files: Files::default(),
            descriptor: Arc::new(OutputDescriptorKind::ACCEPT_ALL_DEFAULT),
        }
    }

    /// Replace the host output descriptor (e.g. after loading a `ShEx` schema
    /// sibling). Re-`Arc`s a new descriptor; cheap and rare (per-`didOpen`).
    fn set_descriptor(&mut self, kind: OutputDescriptorKind) {
        self.descriptor = Arc::new(kind);
    }
}

/// Per-process server state. Holds the Salsa db + the open-file table
/// (URI → `SourceFile` interned id) needed by every feature handler to look
/// up the file's CST / `MappingLoc`s.
struct LspState {
    db: LspDb,
    /// Open-document table. URIs are owned by `lsp_types::Uri`; we key by the
    /// wrapped string representation.
    files: HashMap<String, SourceFile>,
}

impl LspState {
    fn new() -> Self {
        Self {
            db: LspDb::new(),
            files: HashMap::new(),
        }
    }

    /// Record a newly-opened file: intern a fresh `SourceFile` and try to load
    /// a sibling `<stem>.shex` output descriptor so target-side hover/completion
    /// resolve when a schema is present (`AcceptAll` otherwise — source-only).
    fn open(&mut self, uri: &Uri, text: String, path: String) -> SourceFile {
        if let Some(kind) = load_sibling_shex(&path) {
            self.db.set_descriptor(kind);
        }
        let file = SourceFile::new(&self.db, text, path);
        self.files.insert(uri.as_str().to_string(), file);
        file
    }

    /// Apply a `didChange` to an already-open file. Mutates the SAME
    /// `SourceFile` via the Salsa [`salsa::Setter`] (`set_text`) — this BUMPS
    /// THE REVISION, the real cancellation trigger (ADR-0022): any in-flight
    /// analysis from the previous keystroke observes the new revision at its
    /// next cooperative checkpoint. Falls back to a fresh intern if the URI was
    /// never opened (a `didChange` before `didOpen` — tolerated, not an error).
    fn change(&mut self, uri: &Uri, text: String, path: String) -> SourceFile {
        use salsa::Setter as _;
        if let Some(&file) = self.files.get(uri.as_str()) {
            file.set_text(&mut self.db).to(text);
            file
        } else {
            self.open(uri, text, path)
        }
    }

    /// Look up a previously-opened file. Returns `None` if the client requests
    /// a feature before sending `didOpen`.
    fn get(&self, uri: &Uri) -> Option<SourceFile> {
        self.files.get(uri.as_str()).copied()
    }

    /// The open-file set as a slice — the cross-file workspace for goto-def /
    /// completion (ADR-0023: workspace == open files).
    fn open_files(&self) -> Vec<SourceFile> {
        self.files.values().copied().collect()
    }
}

/// Try to read + parse a sibling `<stem>.shex` next to the opened `.fossil`
/// file. Mirrors the CLI's `--shape` auto-discovery. Returns `None` (→ the
/// `AcceptAll` default stays) when there is no sibling or it fails to parse —
/// a missing/broken schema must never break the editor.
fn load_sibling_shex(path: &str) -> Option<OutputDescriptorKind> {
    let p = std::path::Path::new(path);
    let shex = p.with_extension("shex");
    let bytes = std::fs::read(&shex).ok()?;
    match ShExDescriptor::from_reader(bytes.as_slice()) {
        Ok(d) => {
            tracing::debug!(shape = %shex.display(), "loaded sibling ShEx descriptor");
            Some(OutputDescriptorKind::ShEx(d))
        }
        Err(e) => {
            tracing::warn!(shape = %shex.display(), "sibling ShEx failed to parse: {e:?}");
            None
        }
    }
}

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    init_tracing();
    tracing::info!("fossil-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let capabilities = server_capabilities();
    let server_capabilities = serde_json::to_value(&capabilities)?;

    let initialization_params = match connection.initialize(server_capabilities) {
        Ok(p) => p,
        Err(e) if e.channel_is_disconnected() => {
            // Client hung up before the handshake completed (e.g. the test
            // harness only wanted to verify the binary launches). Drain the
            // I/O threads cleanly and return success.
            io_threads.join()?;
            return Ok(());
        }
        Err(e) => return Err(Box::new(e)),
    };

    main_loop(connection, &initialization_params)?;
    // `connection` was moved into `main_loop` and dropped at its return — the
    // writer IO thread can now finish flushing and exit, allowing
    // `io_threads.join()` to return without deadlocking. (If this function
    // took `&Connection` instead, the sender side would outlive `join()` and
    // the writer thread would block forever. Verified the hard way.)
    io_threads.join()?;
    tracing::info!("fossil-lsp shutting down");
    Ok(())
}

/// The full Phase-6 LSP-01 capability set. Each provider is backed by a thin
/// `fossil-ide` adapter in [`handle_request`].
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            // `.` opens field/property completion; `:` opens prefixed-name
            // completion (after a `prefix:`).
            trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
            ..CompletionOptions::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: fossil_ide::semantic_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                work_done_progress_options: WorkDoneProgressOptions::default(),
            },
        )),
        ..ServerCapabilities::default()
    }
}

/// Initialise `tracing-subscriber` with an `EnvFilter` honouring `RUST_LOG`,
/// defaulting to `fossil=info` when the variable is unset (per CLAUDE.md
/// "Style" — `RUST_LOG=fossil=debug` is the canonical filter).
///
/// Logs are written to **stderr**, not stdout — the LSP transport owns stdout
/// and any extra bytes there would corrupt JSON-RPC framing.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("fossil=info"));
    // `try_init` so a second call (e.g. from a test harness) is a no-op rather
    // than a panic, mirroring the fossil-cli pattern from plan 01-07.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Main message-dispatch loop.
///
/// **Takes `Connection` by value** so the sender side drops on return —
/// otherwise the writer IO thread inside `Connection::stdio()` keeps
/// awaiting messages forever and `io_threads.join()` deadlocks. Mirrors
/// the rust-analyzer canonical example signature.
//
// Clippy thinks `Connection` should be passed by reference. It must NOT —
// the by-value drop on return is exactly what unblocks the writer IO thread
// for `io_threads.join()`. This is the rust-analyzer canonical signature
// (per `lsp-server` examples/goto_def.rs).
#[allow(clippy::needless_pass_by_value)]
fn main_loop(
    connection: Connection,
    _initialization_params: &serde_json::Value,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut state = LspState::new();
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(&connection, &state, req)?;
            }
            Message::Notification(notif) => {
                handle_notification(&connection, &mut state, notif)?;
            }
            Message::Response(_) => {
                // The server initiates no requests, so an incoming Response is
                // spurious.
            }
        }
    }
    Ok(())
}

/// Dispatch a request. Each arm decodes its params, calls the corresponding
/// `fossil-ide` free function over `state.db` + the open-file set, and encodes
/// the `lsp_types` result. Unhandled methods fall through to
/// [`respond_method_not_found`].
fn handle_request(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    match req.method.as_str() {
        HoverRequest::METHOD => handle_hover(connection, state, req),
        GotoDefinition::METHOD => handle_definition(connection, state, req),
        Completion::METHOD => handle_completion(connection, state, req),
        DocumentSymbolRequest::METHOD => handle_document_symbol(connection, state, req),
        SemanticTokensFullRequest::METHOD => handle_semantic_tokens(connection, state, req),
        CodeActionRequest::METHOD => handle_code_action(connection, state, req),
        _ => respond_method_not_found(connection, req),
    }
}

/// `textDocument/hover` → [`fossil_ide::hover_bidirectional`] (target-aware via
/// the LSP db's `HirDb` descriptor). Renders Markdown with a UTF-16 range.
fn handle_hover(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let req_id = req.id.clone();
    let Some(params) = extract::<HoverRequest, _>(connection, req)? else {
        return Ok(());
    };
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let Some(file) = state.get(uri) else {
        return send_null_response(connection, req_id);
    };
    let info = fossil_ide::hover_bidirectional(&state.db, file, pos.line, pos.character);
    let payload = info.map(|hi| Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hi.markdown,
        }),
        range: Some(byte_range_to_lsp_range(&state.db, file, hi.range)),
    });
    send_result(connection, req_id, &payload)
}

/// `textDocument/definition` → [`fossil_ide::goto_definition`]; each
/// `NavigationTarget` (file + byte range) becomes a UTF-16 [`Location`].
fn handle_definition(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let req_id = req.id.clone();
    let Some(params) = extract::<GotoDefinition, _>(connection, req)? else {
        return Ok(());
    };
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let Some(file) = state.get(uri) else {
        return send_null_response(connection, req_id);
    };
    let files = state.open_files();
    let targets = fossil_ide::goto_definition(&state.db, &files, file, pos.line, pos.character);
    let locations: Vec<Location> = targets
        .into_iter()
        .filter_map(|t| {
            let target_uri = Uri::from_str_maybe(t.file.path(&state.db))?;
            Some(Location {
                uri: target_uri,
                range: byte_range_to_lsp_range(&state.db, t.file, t.range),
            })
        })
        .collect();
    let payload = (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations));
    send_result(connection, req_id, &payload)
}

/// `textDocument/completion` → [`fossil_ide::completions`] (already returns
/// `lsp_types::CompletionItem`, no translation).
fn handle_completion(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let req_id = req.id.clone();
    let Some(params) = extract::<Completion, _>(connection, req)? else {
        return Ok(());
    };
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let Some(file) = state.get(uri) else {
        return send_null_response(connection, req_id);
    };
    let files = state.open_files();
    let items = fossil_ide::completions(&state.db, &files, file, pos.line, pos.character);
    send_result(connection, req_id, &items)
}

/// `textDocument/documentSymbol` → [`fossil_ide::document_symbols`] (already
/// `lsp_types::DocumentSymbol`).
fn handle_document_symbol(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let req_id = req.id.clone();
    let Some(params) = extract::<DocumentSymbolRequest, _>(connection, req)? else {
        return Ok(());
    };
    let Some(file) = state.get(&params.text_document.uri) else {
        return send_null_response(connection, req_id);
    };
    let symbols = fossil_ide::document_symbols(&state.db, file);
    send_result(connection, req_id, &DocumentSymbolResponse::Nested(symbols))
}

/// `textDocument/semanticTokens/full` → [`fossil_ide::semantic_tokens`]
/// (the spec-mandated flat `Vec<u32>` delta stream).
fn handle_semantic_tokens(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let req_id = req.id.clone();
    let Some(params) = extract::<SemanticTokensFullRequest, _>(connection, req)? else {
        return Ok(());
    };
    let Some(file) = state.get(&params.text_document.uri) else {
        return send_null_response(connection, req_id);
    };
    let data = fossil_ide::semantic_tokens(&state.db, file);
    let payload = SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: decode_to_lsp_tokens(&data),
    });
    send_result(connection, req_id, &payload)
}

/// `textDocument/codeAction` → [`fossil_ide::code_actions`]. The request's own
/// diagnostics are passed through (the actions key off the structured
/// `did_you_mean` / `suggestion_source` fields — but the client sends the
/// published diagnostics back, so we re-derive them from the current document
/// to recover the structured carriers the LSP wire form drops).
fn handle_code_action(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let req_id = req.id.clone();
    let Some(params) = extract::<CodeActionRequest, _>(connection, req)? else {
        return Ok(());
    };
    let Some(file) = state.get(&params.text_document.uri) else {
        return send_null_response(connection, req_id);
    };
    // The wire `CodeActionParams.context.diagnostics` are stripped of our
    // structured carriers; re-drain the Salsa accumulator to recover the
    // full `Diagnostic`s (with `did_you_mean` / `suggestion_source`).
    let diagnostics = diagnostics_for(&state.db, file);
    let actions = fossil_ide::code_actions(&state.db, file, params.range, &diagnostics);
    let payload: Vec<lsp_types::CodeActionOrCommand> = actions
        .into_iter()
        .map(lsp_types::CodeActionOrCommand::CodeAction)
        .collect();
    send_result(connection, req_id, &payload)
}

/// Convert the flat `fossil_ide::semantic_tokens` `Vec<u32>` 5-tuple stream
/// into the `lsp_types::SemanticToken` struct list (`SemanticTokens.data`).
fn decode_to_lsp_tokens(data: &[u32]) -> Vec<lsp_types::SemanticToken> {
    data.chunks_exact(5)
        .map(|c| lsp_types::SemanticToken {
            delta_line: c[0],
            delta_start: c[1],
            length: c[2],
            token_type: c[3],
            token_modifiers_bitset: c[4],
        })
        .collect()
}

/// Decode the params of a request typed by `R`. Returns `Ok(None)` and sends a
/// null response if decoding fails (per the LSP "MAY return null" contract for
/// providers).
fn extract<R, P>(
    connection: &Connection,
    req: Request,
) -> Result<Option<P>, Box<dyn Error + Sync + Send>>
where
    R: lsp_types::request::Request<Params = P>,
    P: serde::de::DeserializeOwned,
{
    let req_id = req.id.clone();
    match req.extract::<P>(R::METHOD) {
        Ok((_, p)) => Ok(Some(p)),
        Err(e) => {
            tracing::warn!("{} params decode failed: {e:?}", R::METHOD);
            send_null_response(connection, req_id)?;
            Ok(None)
        }
    }
}

/// Send a `Response { result: <serde(payload)> }`.
fn send_result<T: serde::Serialize>(
    connection: &Connection,
    req_id: lsp_server::RequestId,
    payload: &T,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let resp = Response {
        id: req_id,
        result: Some(serde_json::to_value(payload)?),
        error: None,
    };
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

/// Send `Response { result: null }` for a Request we couldn't fulfil but
/// don't want to error on (per LSP spec — providers MAY return null).
fn send_null_response(
    connection: &Connection,
    req_id: lsp_server::RequestId,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    send_result(connection, req_id, &serde_json::Value::Null)
}

/// Respond with `MethodNotFound` to any request not implemented above.
fn respond_method_not_found(
    connection: &Connection,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    tracing::debug!("unhandled request: {}", req.method);
    let resp = Response {
        id: req.id,
        result: None,
        error: Some(lsp_server::ResponseError {
            code: ErrorCode::MethodNotFound as i32,
            message: format!("not implemented yet: {}", req.method),
            data: None,
        }),
    };
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

/// Dispatch a notification. `didOpen` / `didChange` record the buffer and
/// publish the REAL `parse → def_map → typecheck` diagnostics. `didChange`
/// mutates via the Salsa `Setter` (revision bump → cancellation trigger).
fn handle_notification(
    connection: &Connection,
    state: &mut LspState,
    notif: Notification,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = cast_notif::<DidOpenTextDocument>(notif)?;
            let uri = params.text_document.uri.clone();
            let path = uri.as_str().to_string();
            tracing::debug!("didOpen: {}", uri.as_str());
            let file = state.open(&uri, params.text_document.text, path);
            publish_diagnostics(connection, &state.db, &uri, file)?;
        }
        DidChangeTextDocument::METHOD => {
            let params = cast_notif::<DidChangeTextDocument>(notif)?;
            let uri = params.text_document.uri.clone();
            // Full-sync model (TextDocumentSyncKind::FULL): the last change
            // carries the entire new document text.
            if let Some(last) = params.content_changes.into_iter().last() {
                let path = uri.as_str().to_string();
                // `change` mutates via `set_text` (Setter) → revision bump →
                // cancels in-flight analysis from the previous keystroke
                // (ADR-0022, the real cancellation trigger).
                let file = state.change(&uri, last.text, path);
                tracing::debug!("didChange: {}", uri.as_str());
                publish_diagnostics(connection, &state.db, &uri, file)?;
            }
        }
        m => tracing::debug!("ignoring notification: {m}"),
    }
    Ok(())
}

/// Run `parse → def_map → typecheck_mapping` and drain the Salsa `Diagnostic`
/// accumulator across every mapping (the same pattern as the `fossil check`
/// CLI, plan 06-02). Returns the structured `fossil_base::Diagnostic`s.
fn diagnostics_for(db: &LspDb, file: SourceFile) -> Vec<Diagnostic> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mappings = def_map.mappings(db);
    let mut out = Vec::new();
    for mapping in mappings {
        let _ = fossil_hir::check::typecheck_mapping(db, *mapping);
        let diags = fossil_hir::check::typecheck_mapping::accumulated::<Diagnostic>(db, *mapping);
        out.extend(diags.into_iter().cloned());
    }
    out
}

/// Publish `textDocument/publishDiagnostics` for `file`: drain the accumulator
/// and convert each `fossil_base::Diagnostic` to an `lsp_types::Diagnostic`
/// with a UTF-16 range (06-05 `LineIndex`).
fn publish_diagnostics(
    connection: &Connection,
    db: &LspDb,
    uri: &Uri,
    file: SourceFile,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let index = fossil_ide::line_index(db, file);
    let diagnostics: Vec<LspDiagnostic> = diagnostics_for(db, file)
        .iter()
        .map(|d| to_lsp_diagnostic(&index, d))
        .collect();
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    let notif = Notification {
        method: PublishDiagnostics::METHOD.to_string(),
        params: serde_json::to_value(&params)?,
    };
    connection.sender.send(Message::Notification(notif))?;
    Ok(())
}

/// Convert one `fossil_base::Diagnostic` to an `lsp_types::Diagnostic`. The
/// byte span is converted to a UTF-16 range; the `suggestion_source` (when
/// present) is folded into the message as a help line (the structured carriers
/// are recovered for code actions by re-draining the accumulator).
fn to_lsp_diagnostic(index: &LineIndex, d: &Diagnostic) -> LspDiagnostic {
    let range = span_to_lsp_range(index, d.span);
    let message = d.suggestion_source.as_ref().map_or_else(
        || d.message.clone(),
        |s| format!("{}\nhelp: {s}", d.message),
    );
    LspDiagnostic {
        range,
        severity: Some(severity_to_lsp(d.severity)),
        message,
        ..LspDiagnostic::default()
    }
}

const fn severity_to_lsp(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}

/// Translate a byte-offset range (from a `fossil-ide` feature) to a UTF-16 LSP
/// `Range` via the 06-05 `LineIndex` (replaces the Phase-2 ASCII-only walk).
fn byte_range_to_lsp_range(db: &LspDb, file: SourceFile, range: std::ops::Range<u32>) -> Range {
    let index = fossil_ide::line_index(db, file);
    Range {
        start: utf16_to_position(fossil_ide::offset_to_lsp_position(&index, range.start)),
        end: utf16_to_position(fossil_ide::offset_to_lsp_position(&index, range.end)),
    }
}

/// Translate a `fossil_base::Span` to a UTF-16 LSP `Range`.
fn span_to_lsp_range(index: &LineIndex, span: Span) -> Range {
    Range {
        start: utf16_to_position(fossil_ide::offset_to_lsp_position(index, span.start)),
        end: utf16_to_position(fossil_ide::offset_to_lsp_position(index, span.end)),
    }
}

const fn utf16_to_position(p: Utf16Position) -> Position {
    Position {
        line: p.line,
        character: p.character,
    }
}

/// Thin wrapper around [`Notification::extract`] — typed by `N`'s associated
/// `METHOD` constant so a method/params-type mismatch becomes a compile error.
fn cast_notif<N>(notif: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
{
    notif.extract::<N::Params>(N::METHOD)
}

/// Parse a path or URI string into an `lsp_types::Uri`. The LSP keys documents
/// by URI; a `SourceFile.path` may be either a `file://` URI (from the LSP) or
/// a bare path (from a test) — prepend `file://` when schemeless.
trait UriExt: Sized {
    fn from_str_maybe(s: &str) -> Option<Self>;
}

impl UriExt for Uri {
    fn from_str_maybe(s: &str) -> Option<Self> {
        use std::str::FromStr as _;
        if s.contains("://") {
            Self::from_str(s).ok()
        } else if let Some(rest) = s.strip_prefix('/') {
            Self::from_str(&format!("file:///{rest}")).ok()
        } else {
            Self::from_str(&format!("file://{s}")).ok()
        }
    }
}
