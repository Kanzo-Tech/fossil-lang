//! `fossil-lsp` — Phase 1 stub Language Server.
//!
//! Wires the LSP transport from day 1 (per ROADMAP.md Phase 1 success
//! criterion #4 + RESEARCH.md commit #9 of 10) so Fossil avoids the fase-4
//! trap that killed the predecessor project. The dispatch loop responds to:
//!
//! - `initialize` → [`ServerCapabilities`] advertising `TextDocumentSync::FULL`.
//! - `textDocument/didOpen` → emits an empty
//!   `textDocument/publishDiagnostics`.
//! - `textDocument/didChange` → emits an empty
//!   `textDocument/publishDiagnostics`.
//! - `shutdown` (request) → handled by `Connection::handle_shutdown`.
//! - `exit` (notification) → `Connection::handle_shutdown` returns the
//!   request side; the matching `exit` notification is what closes the
//!   receiver and ends the loop. All other requests respond with
//!   `MethodNotFound`; all other notifications are logged and ignored.
//!
//! Phase 6 LSP-01 replaces the empty-diagnostics stub with the real
//! `parse → def_map → typecheck → diagnostics` pipeline, wires Salsa
//! cancellation on every `didChange`, and adds the hover/goto-def/completion
//! providers. Phase 1 deliberately ships zero compiler features so the LSP
//! transport itself is exercised before any real workload lands on it.
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

use lsp_server::{Connection, ErrorCode, ExtractError, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::{
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Uri,
};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    init_tracing();
    tracing::info!("fossil-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        // Phase 6 LSP-01 adds: hover_provider, definition_provider,
        // completion_provider, semantic_tokens_provider, …
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
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                respond_method_not_found(&connection, req)?;
            }
            Message::Notification(notif) => {
                handle_notification(&connection, notif)?;
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

/// Respond with `MethodNotFound` to any request the Phase 1 stub doesn't
/// implement. Phase 6 LSP-01 replaces individual arms (hover, definition,
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
            message: format!("not implemented in Phase 1: {}", req.method),
            data: None,
        }),
    };
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

/// Dispatch a notification. Handles `textDocument/didOpen` and
/// `textDocument/didChange` by acknowledging with an **empty diagnostics**
/// publish; ignores everything else (logged at `debug`).
///
/// The `exit` notification is NOT handled here — `Connection::handle_shutdown`
/// owns the shutdown request, and `exit` is what the client sends to close
/// the channel. When the client closes its end, the receiver iterator
/// terminates and `main_loop` returns naturally. (See `lsp-server` 0.7
/// `Connection::stdio` IO-thread shutdown semantics.)
fn handle_notification(
    connection: &Connection,
    notif: Notification,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = cast_notif::<DidOpenTextDocument>(notif)?;
            let uri = params.text_document.uri;
            tracing::debug!("didOpen: {}", uri.as_str());
            send_empty_diagnostics(connection, uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let params = cast_notif::<DidChangeTextDocument>(notif)?;
            let uri = params.text_document.uri;
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

/// Thin wrapper around [`Notification::extract`] — typed by `N`'s associated
/// `METHOD` constant so a method/params-type mismatch becomes a compile error.
fn cast_notif<N>(notif: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
{
    notif.extract::<N::Params>(N::METHOD)
}
