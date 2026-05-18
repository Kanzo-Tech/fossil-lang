//! `fossil-lsp` — Phase 1 stub + Phase 2 plan 02-06 hover delta.
//!
//! Wires the LSP transport from day 1 (per ROADMAP.md Phase 1 success
//! criterion #4 + RESEARCH.md commit #9 of 10) so Fossil avoids the fase-4
//! trap that killed the predecessor project. The dispatch loop responds to:
//!
//! - `initialize` → [`ServerCapabilities`] advertising `TextDocumentSync::FULL`
//!   AND `hover_provider: Simple(true)` (Phase 2 plan 02-06 delta).
//! - `textDocument/didOpen` → records `(uri, SourceFile)` in the per-process
//!   state map AND emits an empty `textDocument/publishDiagnostics`.
//! - `textDocument/didChange` → re-records the buffer (full-sync model)
//!   AND emits an empty `publishDiagnostics`.
//! - `textDocument/hover` → routes to [`fossil_ide::hover`]; renders the
//!   returned [`fossil_ide::HoverInfo`] as `Hover { contents: Markdown,
//!   range: Some(...) }` (Phase 2 plan 02-06).
//! - `shutdown` (request) → handled by `Connection::handle_shutdown`.
//! - `exit` (notification) → `Connection::handle_shutdown` returns the
//!   request side; the matching `exit` notification is what closes the
//!   receiver and ends the loop. All other requests respond with
//!   `MethodNotFound`; all other notifications are logged and ignored.
//!
//! Phase 6 LSP-01 replaces the empty-diagnostics stub with the real
//! `parse → def_map → typecheck → diagnostics` pipeline, wires Salsa
//! cancellation on every `didChange`, and adds the goto-def + completion
//! providers (hover already lives here as of plan 02-06).
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

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use lsp_server::{Connection, ErrorCode, ExtractError, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{HoverRequest, Request as _};
use lsp_types::{
    Hover, HoverContents, HoverProviderCapability, MarkupContent, MarkupKind, Position,
    PublishDiagnosticsParams, Range, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri,
};

/// Per-process server state. Holds the Salsa db + the open-file table
/// (URI → `SourceFile` interned id) needed by hover to look up the file's
/// CST / `MappingLoc`s.
///
/// Phase 1 had no state because the only handlers were stateless ack-with-
/// empty-diagnostics. Phase 2 plan 02-06's hover handler needs both the db
/// (to run the `ty_origin` Salsa query) and the URI→SourceFile mapping
/// (populated by didOpen / refreshed by didChange).
struct LspState {
    db: FossilDb,
    /// Open-document table. URIs are owned by `lsp_types::Uri` so we can
    /// hash by the wrapped string representation.
    files: HashMap<String, SourceFile>,
}

impl LspState {
    fn new() -> Self {
        let system: Arc<dyn System> = Arc::new(NativeSystem);
        Self {
            db: FossilDb::new(system),
            files: HashMap::new(),
        }
    }

    /// Record a file's text content; subsequent invocations on the same URI
    /// re-intern a fresh `SourceFile` (full-sync model — Phase 6 LSP-01
    /// switches to incremental sync with text edits).
    fn upsert(&mut self, uri: &Uri, text: String, path: String) -> SourceFile {
        let file = SourceFile::new(&self.db, text, path);
        self.files.insert(uri.as_str().to_string(), file);
        file
    }

    /// Look up a previously-opened file. Returns `None` if the client
    /// hovers before sending `didOpen` (Phase 2 hover then silently no-ops).
    fn get(&self, uri: &Uri) -> Option<SourceFile> {
        self.files.get(uri.as_str()).copied()
    }
}

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    init_tracing();
    tracing::info!("fossil-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        // Phase 2 plan 02-06 delta: advertise hover.
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        // Phase 6 LSP-01 adds: definition_provider, completion_provider,
        // semantic_tokens_provider, …
        ..ServerCapabilities::default()
    };
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
///
/// `_initialization_params` is reserved for Phase 6 LSP-01, which will use
/// the client's `workspace_folders` and capability declarations to decide
/// what providers to advertise. Phase 1 ignores it.
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
                // Phase 1: server doesn't initiate any request, so an
                // incoming Response would be spurious. Phase 6 LSP-01 may
                // change this once we adopt server-initiated `workspace/
                // configuration` requests.
            }
        }
    }
    Ok(())
}

/// Dispatch a request. Phase 2 plan 02-06 implements `textDocument/hover`
/// via [`fossil_ide::hover`]; everything else falls through to
/// [`respond_method_not_found`].
fn handle_request(
    connection: &Connection,
    state: &LspState,
    req: Request,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    if req.method == HoverRequest::METHOD {
        let req_id = req.id.clone();
        let params: lsp_types::HoverParams =
            match req.extract::<lsp_types::HoverParams>(HoverRequest::METHOD) {
                Ok((_, p)) => p,
                Err(e) => {
                    tracing::warn!("hover params decode failed: {e:?}");
                    send_null_response(connection, req_id)?;
                    return Ok(());
                }
            };
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some(file) = state.get(uri) else {
            tracing::debug!("hover on unknown uri {}; responding null", uri.as_str());
            send_null_response(connection, req_id)?;
            return Ok(());
        };
        let info = fossil_ide::hover(&state.db, file, pos.line, pos.character);
        let response_payload = info.map(|hi| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hi.markdown,
            }),
            range: Some(byte_range_to_lsp_range(&state.db, file, hi.range)),
        });
        let resp = Response {
            id: req_id,
            result: Some(serde_json::to_value(&response_payload)?),
            error: None,
        };
        connection.sender.send(Message::Response(resp))?;
        return Ok(());
    }
    respond_method_not_found(connection, req)
}

/// Send `Response { result: null }` for a Request we couldn't fulfil but
/// don't want to error on (per LSP spec — hover MAY return null).
fn send_null_response(
    connection: &Connection,
    req_id: lsp_server::RequestId,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let resp = Response {
        id: req_id,
        result: Some(serde_json::Value::Null),
        error: None,
    };
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

/// Respond with `MethodNotFound` to any request the Phase 1/2 stub doesn't
/// implement. Phase 6 LSP-01 replaces individual arms (definition,
/// completion, …) before falling back here.
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

/// Dispatch a notification. Handles `textDocument/didOpen` and
/// `textDocument/didChange` by recording the buffer in state + acknowledging
/// with an **empty diagnostics** publish; ignores everything else (logged
/// at `debug`).
///
/// The `exit` notification is NOT handled here — `Connection::handle_shutdown`
/// owns the shutdown request, and `exit` is what the client sends to close
/// the channel. When the client closes its end, the receiver iterator
/// terminates and `main_loop` returns naturally. (See `lsp-server` 0.7
/// `Connection::stdio` IO-thread shutdown semantics.)
fn handle_notification(
    connection: &Connection,
    state: &mut LspState,
    notif: Notification,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = cast_notif::<DidOpenTextDocument>(notif)?;
            let uri = params.text_document.uri.clone();
            let text = params.text_document.text;
            tracing::debug!("didOpen: {}", uri.as_str());
            let path = uri.as_str().to_string();
            state.upsert(&uri, text, path);
            send_empty_diagnostics(connection, uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let params = cast_notif::<DidChangeTextDocument>(notif)?;
            let uri = params.text_document.uri.clone();
            // Full-sync model (TextDocumentSyncKind::FULL): the last change
            // contains the entire new document text.
            if let Some(last) = params.content_changes.into_iter().last() {
                let path = uri.as_str().to_string();
                state.upsert(&uri, last.text, path);
            }
            tracing::debug!("didChange: {}", uri.as_str());
            send_empty_diagnostics(connection, uri)?;
        }
        m => tracing::debug!("ignoring notification: {m}"),
    }
    Ok(())
}

/// Emit `textDocument/publishDiagnostics` with `diagnostics: []` for the
/// given URI. Phase 6 LSP-01 replaces `vec![]` with the
/// `parse → def_map → typecheck` accumulator output.
fn send_empty_diagnostics(
    connection: &Connection,
    uri: Uri,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics: vec![],
        version: None,
    };
    let notif = Notification {
        method: PublishDiagnostics::METHOD.to_string(),
        params: serde_json::to_value(&params)?,
    };
    connection.sender.send(Message::Notification(notif))?;
    Ok(())
}

/// Translate a byte-offset range (from [`fossil_ide::HoverInfo::range`]) to
/// an LSP `Range` using the file's text. Walks the text up to each
/// endpoint to count newlines + intra-line bytes. ASCII-only for Phase 2
/// (Phase 6 LSP-01 will use a UTF-16 conversion table).
fn byte_range_to_lsp_range(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    range: std::ops::Range<u32>,
) -> Range {
    let text = file.text(db);
    Range {
        start: offset_to_position(text, range.start),
        end: offset_to_position(text, range.end),
    }
}

fn offset_to_position(text: &str, offset: u32) -> Position {
    let offset_usize = offset as usize;
    let prefix = if offset_usize > text.len() {
        text
    } else {
        &text[..offset_usize]
    };
    let line = u32::try_from(prefix.matches('\n').count()).unwrap_or(u32::MAX);
    let character = prefix.rfind('\n').map_or(offset, |n| {
        offset - u32::try_from(n).unwrap_or(u32::MAX) - 1
    });
    Position { line, character }
}

/// Thin wrapper around [`Notification::extract`] — typed by `N`'s associated
/// `METHOD` constant so a method/params-type mismatch becomes a compile error.
fn cast_notif<N>(notif: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
{
    notif.extract::<N::Params>(N::METHOD)
}
