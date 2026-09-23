//! `fossil-lsp` — the stdio transport, and only that.
//!
//! The server itself is the library half of this crate (`src/lib.rs`): the
//! host's [`System`](fossil_base::System), the `#[salsa::db]` struct, the
//! open-file table, the capability set and every handler. This file owns
//! the things that need a socket or a process and nothing else — `Connection::stdio()`,
//! the `initialize` handshake, `handle_shutdown`, the message loop, the IO-thread
//! join, and `tracing` to stderr.
//!
//! **The split is the point.** `fossil_lsp::handle_request` returns a
//! [`Response`] and `fossil_lsp::handle_notification` returns the
//! `publishDiagnostics` notifications it wants sent, so both can be called
//! without a channel. The alternative — a handler that takes `&Connection` — is
//! what made `tests/didchange_budget.rs` keep a private reimplementation of the
//! `didChange` path for months, and that copy silently stopped measuring the
//! handler twice.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-lsp is native-only (lsp-server uses crossbeam-channel + stdio); \
     the playground exposes LSP features via fossil-wasm directly, not through this binary"
);

use std::error::Error;

use fossil_lsp::{LspState, handle_notification, handle_request, server_capabilities};
use lsp_server::{Connection, Message, Response};

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    init_tracing();
    tracing::info!("fossil-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let capabilities = server_capabilities();
    let capabilities = serde_json::to_value(&capabilities)?;

    let initialization_params = match connection.initialize(capabilities) {
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
    // than a panic, mirroring the fossil-cli pattern.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Main message-dispatch loop: read a message, ask the library what the answer
/// is, put it on the wire.
///
/// `shutdown` is the one request handled here rather than in the library, and
/// it is handled here because `Connection::handle_shutdown` IS transport — it
/// replies over the channel and then waits on it for the `exit` notification.
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
                send_response(&connection, handle_request(&state, req))?;
            }
            Message::Notification(notif) => {
                for published in handle_notification(&mut state, notif)? {
                    connection.sender.send(Message::Notification(published))?;
                }
            }
            Message::Response(_) => {
                // The server initiates no requests, so an incoming Response is
                // spurious.
            }
        }
    }
    Ok(())
}

fn send_response(
    connection: &Connection,
    resp: Response,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}
