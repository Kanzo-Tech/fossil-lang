//! `fossil-lsp` — the LSP server, minus the wire.
//!
//! Everything a `didChange` costs lives here; `src/main.rs` is the stdio
//! transport around it and nothing else. The split is not tidiness. It is what
//! makes `tests/didchange_budget.rs` — the keystroke-latency hard gate — able to
//! CALL the handler instead of describing it. It described it for months, and
//! twice the description went out of date without going red: the introspection
//! step was outside the clock the moment it was added, and
//! `benches/lsp_didchange.rs` measured a hand-rolled `def_map` +
//! `typecheck_mapping` loop under a docblock claiming it was «kept in sync» with
//! a budget test that had already moved on.
//!
//! # Where the cut is
//!
//! `lsp_server::Connection` is a pair of `crossbeam` channels attached to
//! stdio. A handler that takes `&Connection` needs a socket to be called at all,
//! which is the whole reason the copies existed. So no function here takes one:
//! [`handle_request`] RETURNS a [`Response`], [`handle_notification`] RETURNS
//! the `textDocument/publishDiagnostics` notifications it wants sent, and
//! `main.rs` is what puts them on the channel.
//!
//! `lsp_server::Response` and `Notification` come with it — they are wire
//! VALUES, plain serde structs with no channel in them, and building one costs
//! what the server really pays to answer.
//!
//! This is the shape `fossil-wasm`'s `lsp_worker::dispatch` already had
//! (`DispatchOutput { response, diagnostics }`), and it is why that side was
//! callable in-process from `tests/transport_parity.rs` while this one had to be
//! spawned as a subprocess.
//!
//! # The server
//!
//! Fossil wired the LSP from day 1, so it avoided the parser-and-typecheck
//! first, integration-last trap that killed the predecessor project. It started
//! as a transport-only stub and is now the full capability set, each handler
//! being a thin adapter over a `fossil-ide` free function:
//!
//! - `initialize` → [`server_capabilities`], advertising `TextDocumentSync::FULL`,
//!   `hover_provider`, `definition_provider`, `completion_provider` (trigger
//!   characters `.` and `:`), `document_symbol_provider`, `code_action_provider`,
//!   and `semantic_tokens_provider` (full-document, legend from
//!   [`fossil_ide::semantic_legend`]).
//! - `textDocument/didOpen` → records `(uri, SourceFile)` and publishes
//!   [`fossil_ide::lsp_diagnostics`].
//! - `textDocument/didChange` → mutates the SAME `SourceFile` via the Salsa
//!   `Setter` (`set_text`), which BUMPS THE REVISION — the real cancellation
//!   trigger (NOT a fictional `db.cancel_pending()`) — then republishes.
//! - `textDocument/didClose` → forgets the buffer and publishes an empty list.
//! - `textDocument/hover` → [`fossil_ide::hover_bidirectional`] (the
//!   target-side `ShEx` type is reachable whenever the program names its output
//!   document — the editor supplies a filesystem, not a contract).
//! - `textDocument/definition` → [`fossil_ide::goto_definition`] → `Location`s.
//! - `textDocument/completion` → [`fossil_ide::completions`].
//! - `textDocument/documentSymbol` → [`fossil_ide::document_symbols`].
//! - `textDocument/semanticTokens/full` → [`fossil_ide::semantic_tokens`].
//! - `textDocument/codeAction` → [`fossil_ide::code_actions`].
//!
//! `shutdown` / `exit` are NOT here: `Connection::handle_shutdown` is the
//! transport's own state machine over its own channel, so it stays in `main.rs`
//! with the socket it needs.
//!
//! The transport is `lsp-server`, NOT `tower-lsp`, per CLAUDE.md "Hard Rules",
//! and this crate is native-only. The dispatch loop pattern is derived from
//! `rust-analyzer/lsp-server/examples/goto_def.rs`.
//!
//! # The other transport
//!
//! `fossil-wasm`'s `lsp_worker` serves the same LSP over `postMessage` to a Web
//! Worker. Its module docs used to call the handlers here «the 1:1 model; the
//! only difference is the wire channel», and that was not so — the two dropped
//! `d.labels` separately, got the shape-document guard a day apart, and
//! published diagnostics in two different JSON shapes.
//!
//! Neither can be deleted: two transports (stdio versus `postMessage`), two
//! hosts (a filesystem versus buffers only), two `#[salsa::db]` structs. What
//! IS one thing is the ANSWERS, and they are `fossil-ide` free functions that
//! both call. `crates/fossil-lsp/tests/transport_parity.rs` drives both with the
//! same buffers and compares the JSON, and holds the whole list of ways they are
//! still allowed to differ. Three things remain outside it, and all three are
//! about the HOST rather than the wire:
//!
//! - **The filesystem.** This server reads an unopened shape document off disk
//!   ([`LspState::register_named_documents`]); the worker has no disk, so there
//!   the only copy of a document is a buffer somebody opened.
//! - **Introspected descriptors.** The worker answers
//!   `fossil/registerInferredDescriptor`, because the browser has to push in
//!   what a `DESCRIBE` found. This server has a filesystem and goes and looks
//!   itself ([`LspState::introspect`]) — but only at sources it can `stat`. A
//!   program whose CSV lives on `s3://` is therefore still checked here without
//!   its columns, so `fossil check` reports things this editor does not; that
//!   gap is deliberate, it is the price of never blocking the message loop on
//!   the network, and
//!   `tests/introspected_diagnostics.rs::a_remote_source_is_not_introspected_by_the_editor`
//!   is what keeps it from going quiet.
//! - **`fossil/checkAll`.** A workspace-wide drain for a playground panel. An
//!   editor already receives one `publishDiagnostics` per file.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-lsp is native-only (lsp-server uses crossbeam-channel + stdio); \
     the playground exposes LSP features via fossil-wasm directly, not through this binary"
);

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::{
    Catalogue, Files, FsError, Provider, SourceAnchor, SourceFile, System, register_file,
};
use fossil_descriptors_input::DescriptorCache;
use lsp_server::{ErrorCode, Notification, Request, Response, ResponseError};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest,
    Request as _, SemanticTokensFullRequest,
};
use lsp_types::{
    CodeActionProviderCapability, CompletionOptions, Diagnostic as LspDiagnostic,
    DocumentSymbolResponse, GotoDefinitionResponse, Hover, HoverContents, HoverProviderCapability,
    Location, MarkupContent, MarkupKind, OneOf, PublishDiagnosticsParams, Range, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkDoneProgressOptions,
};

/// The editor's [`System`].
///
/// A filesystem, a clock, the decoder rows for the shape documents a program can
/// name, and the table of introspected input schemas. The LSP COMPILES
/// programs, so it installs the same rows the native engine does: without them
/// every program checks against no output contract, and the target-side halves
/// of [`fossil_ide::hover_bidirectional`] / [`fossil_ide::completions`] go
/// quietly empty for exactly the programs that declare a shape.
///
/// It replaced `fossil_base::test_support::NativeSystem`, whose decoder table is the trait
/// default — `&[]`, correct for a host that decodes nothing and wrong for this
/// one.
///
/// # The descriptor table was `None`, and that was a silent editor
///
/// It inherited the trait default and the docblock called it deliberate: «the
/// LSP introspects no sources yet, and `None` is a real answer rather than an
/// empty table pretending to be one». That was honest about a host that did
/// nothing with sources, and it stopped being harmless the moment the checker
/// started reporting on their columns. Measured 2026-08-23 over the real stdio
/// transport: `fossil check apps/docs/programs/errors/unknown-field/program.fossil`
/// reports ``nmae` is not a field of `User`` and exits 1, and the editor
/// published **zero** diagnostics for the same bytes. Every diagnostic whose
/// evidence is a source's schema was absent from the editor and present in the
/// CLI and in the browser playground.
///
/// The table is `Mutex`-guarded and reached through `&self`, so this stays
/// behind the `Arc<dyn System>` on [`LspDb`] and needs no mutable path.
/// Reading it registers no Salsa dependency — see
/// [`fossil_base::System::descriptors`] — which is why
/// [`LspState::introspect`] runs BEFORE anything queries the db.
#[derive(Debug, Default)]
struct LspSystem {
    descriptors: DescriptorCache,
}

impl System for LspSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(path.display().to_string()),
            _ => FsError::Io(e.to_string()),
        })
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }

    fn descriptors(&self) -> Option<&DescriptorCache> {
        Some(&self.descriptors)
    }
}

/// The local file a registry key names, or `None` for a key no filesystem can
/// answer.
///
/// The keys are the program's own path with the document joined onto it, so in
/// this host they are `file://` URIs. Anything with another scheme belongs to a
/// client we cannot read for (a `untitled:` buffer, a remote workspace); the
/// answer there is `None`, and the document arrives — if it arrives — when the
/// client opens it.
///
/// Percent-escapes are NOT decoded. A workspace path containing one is read
/// wrong today; the fix is a URI type at this seam, not a hand-rolled decoder.
fn local_path(key: &str) -> Option<PathBuf> {
    if let Some(rest) = key.strip_prefix("file://") {
        return Some(PathBuf::from(rest));
    }
    if key.contains("://") {
        return None;
    }
    Some(PathBuf::from(key))
}

/// The LSP database.
///
/// A `#[salsa::db]` struct carrying the Salsa runtime, the host [`System`] and
/// the file registry. The target shape reaches
/// [`fossil_ide::hover_bidirectional`] / [`fossil_ide::completions`] because
/// the PROGRAM names its output document and this host REGISTERS it — as a
/// Salsa input, so an edit to the document re-checks the programs that read it.
#[salsa::db]
#[derive(Clone)]
pub struct LspDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    catalogue: Catalogue,
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

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }
}

impl LspDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            system: Arc::new(LspSystem::default()),
            files: Files::default(),
            catalogue: Catalogue::default(),
        }
    }
}

/// Per-process server state. Holds the Salsa db + the open-file table
/// (URI → `SourceFile` interned id) needed by every feature handler to look
/// up the file's CST / `MappingLoc`s.
pub struct LspState {
    db: LspDb,
    /// Open-document table. URIs are owned by `lsp_types::Uri`; we key by the
    /// wrapped string representation.
    files: HashMap<String, SourceFile>,
}

impl std::fmt::Debug for LspState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspState")
            .field("open", &self.files.len())
            .finish_non_exhaustive()
    }
}

impl Default for LspState {
    fn default() -> Self {
        Self::new()
    }
}

impl LspState {
    /// A server with an empty db and no open buffers.
    pub fn new() -> Self {
        Self {
            db: LspDb::new(),
            files: HashMap::new(),
        }
    }

    /// The Salsa db, for a caller that wants to ask the compiler something
    /// directly.
    ///
    /// It is here for `tests/didchange_budget.rs`, which asserts the fixture
    /// really resolves its output contract BEFORE it starts the clock — a
    /// budget over a program that checks nothing is a flattering number, and
    /// that assertion is what stops it being one.
    pub const fn db(&self) -> &LspDb {
        &self.db
    }

    /// Record a newly-opened file: intern a fresh `SourceFile`. The path is
    /// kept because the shape documents the program names are read relative to
    /// it.
    ///
    /// Two registrations, and they are different things. The buffer goes into
    /// the file registry under its own URI, so opening a `.shex` makes the OPEN
    /// COPY the document every program naming it reads — that is how an unsaved
    /// edit reaches the checker, and the disk cannot express it. Then the
    /// documents THIS file names are read from disk if nobody has them yet.
    fn open(&mut self, uri: &Uri, text: String, path: String) -> SourceFile {
        self.introspect(&text, &path);
        let file = SourceFile::new(&self.db, text, path.clone());
        register_file(&mut self.db, path, file);
        self.files.insert(uri.as_str().to_string(), file);
        self.register_named_documents(file);
        file
    }

    /// Read the columns of every source the buffer names that this host can
    /// `stat`, and register them on [`LspSystem`]'s descriptor table.
    ///
    /// **The buffer, not the file on disk.** `fossil_introspect::introspect_program`
    /// is the sibling that opens a path, and it is the wrong one here: a source
    /// line the user has typed and not saved is exactly the line whose columns
    /// the editor needs, and the disk does not have it.
    ///
    /// # Two things this must not become
    ///
    /// **It must not go on the network.** The message loop is one sequential
    /// loop over one channel: a `didOpen` blocked on an `s3://` `DESCRIBE` is
    /// not one slow file, it is hover and completion dead in every other buffer
    /// until the read returns. `fossil_introspect::Reach::Local` is the whole of
    /// that promise — a source this host cannot `stat` is skipped, no connection
    /// opened, and the diagnostics that needed its columns stay absent. That
    /// gap is pinned by
    /// `tests/introspected_diagnostics.rs::a_remote_source_is_not_introspected_by_the_editor`.
    ///
    /// **It must not run per keystroke.** It is called on every `didOpen` and
    /// `didChange`, and what makes that affordable is the freshness token: a
    /// local source whose `mtime` and size have not moved is a hash lookup, and
    /// no `DuckDB` connection is opened at all on a call where every source is
    /// fresh or unreachable. `tests/didchange_budget.rs` is inside this call
    /// now, so a change that breaks the freshness token shows up as a budget
    /// failure rather than as a slow editor.
    ///
    /// # Ordering, and why it is before the db
    ///
    /// The descriptor table is ambient — reading it inside a tracked query
    /// registers no Salsa dependency, so a write that lands after a query has
    /// memoised its answer is invisible until something else invalidates it.
    /// Introspecting first, before the text is interned or set, means no query
    /// in this revision has looked yet.
    ///
    /// A URI this host cannot turn into a local path (an `untitled:` buffer, a
    /// remote workspace) resolves nothing relative and is skipped whole.
    fn introspect(&self, text: &str, path: &str) {
        let Some(program) = local_path(path) else {
            return;
        };
        let dir = program
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf);
        fossil_introspect::pre_introspect_and_register(
            &*self.db.system,
            text,
            SourceAnchor::beside(&dir),
            &HashMap::new(),
            fossil_introspect::Reach::Local,
        );
    }

    /// Register every shape document `file` names that the database does not
    /// hold yet, reading it from the local filesystem.
    ///
    /// Unopened documents are read ONCE, here. There is no
    /// `workspace/didChangeWatchedFiles` handling in this server, so a document
    /// nobody opened that changes on disk afterwards is stale until the program
    /// naming it is reopened. Opening the document fixes it for good: from then
    /// on it is a buffer, and every keystroke in it is a `set_text` the checker
    /// sees.
    fn register_named_documents(&mut self, file: SourceFile) {
        let registered = fossil_ide::register_missing_documents(&mut self.db, file, &|key| {
            let path = local_path(key)?;
            match std::fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(e) => {
                    tracing::debug!("shape document {} was not read: {e}", path.display());
                    None
                }
            }
        });
        if registered > 0 {
            tracing::debug!("registered {registered} shape document(s) from disk");
        }
    }

    /// Apply a `didChange` to an already-open file. Mutates the SAME
    /// `SourceFile` via the Salsa [`salsa::Setter`] (`set_text`) — this BUMPS
    /// THE REVISION, the real cancellation trigger: any in-flight
    /// analysis from the previous keystroke observes the new revision at its
    /// next cooperative checkpoint. Falls back to a fresh intern if the URI was
    /// never opened (a `didChange` before `didOpen` — tolerated, not an error).
    fn change(&mut self, uri: &Uri, text: String, path: String) -> SourceFile {
        use salsa::Setter as _;
        if let Some(&file) = self.files.get(uri.as_str()) {
            // Before the revision bump, for the reason in `introspect`: the
            // descriptor table is ambient, so it has to be right before any
            // query in this revision looks at it. A keystroke that changed no
            // source line costs a `stat` per source and no read.
            self.introspect(&text, &path);
            file.set_text(&mut self.db).to(text);
            // The keystroke may have just written the `type { … } =
            // io.shex("…")` line that names a document. A no-op once the
            // document is in — the loop skips what the registry already holds.
            self.register_named_documents(file);
            file
        } else {
            self.open(uri, text, path)
        }
    }

    /// Look up a previously-opened file. Returns `None` if the client requests
    /// a feature before sending `didOpen`.
    pub fn get(&self, uri: &Uri) -> Option<SourceFile> {
        self.files.get(uri.as_str()).copied()
    }

    /// Forget a closed buffer.
    ///
    /// **This did not exist, and the worker had it from the day it was
    /// written.** A file the user closed stayed in this table forever: still in
    /// [`Self::open_files`], which IS the workspace goto-def and completion
    /// resolve against, so a name from a closed buffer kept resolving; and still
    /// carrying whatever diagnostics were last published for it, with no
    /// notification to clear them.
    ///
    /// It does NOT deregister the file from `fossil_base`'s registry, and that
    /// is the same choice the worker makes. Closing the `.shex` a program names
    /// must not silently re-check the program against nothing — the last text
    /// the user had is a better answer than no contract at all, and the
    /// registry is the only place holding it.
    fn close(&mut self, uri: &Uri) {
        self.files.remove(uri.as_str());
    }

    /// The open-file set as a slice — the cross-file workspace for goto-def /
    /// completion — the workspace IS the open files.
    fn open_files(&self) -> Vec<SourceFile> {
        self.files.values().copied().collect()
    }
}

/// The full capability set the server advertises. Each provider is backed by a
/// thin `fossil-ide` adapter in [`handle_request`].
pub fn server_capabilities() -> ServerCapabilities {
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

/// Answer one request. Each arm decodes its params, calls the corresponding
/// `fossil-ide` free function over `state.db` + the open-file set, and encodes
/// the `lsp_types` result. Unhandled methods fall through to
/// [`method_not_found`].
///
/// `shutdown` never reaches here: `Connection::handle_shutdown` owns that
/// exchange and it owns a channel, so it stays in `main.rs`.
pub fn handle_request(state: &LspState, req: Request) -> Response {
    match req.method.as_str() {
        HoverRequest::METHOD => handle_hover(state, req),
        GotoDefinition::METHOD => handle_definition(state, req),
        Completion::METHOD => handle_completion(state, req),
        DocumentSymbolRequest::METHOD => handle_document_symbol(state, req),
        SemanticTokensFullRequest::METHOD => handle_semantic_tokens(state, req),
        CodeActionRequest::METHOD => handle_code_action(state, req),
        _ => method_not_found(req),
    }
}

/// `textDocument/hover` → [`fossil_ide::hover_bidirectional`] (target-aware
/// whenever the program names an output document). Renders Markdown with a
/// UTF-16 range.
fn handle_hover(state: &LspState, req: Request) -> Response {
    let req_id = req.id.clone();
    let params = match extract::<HoverRequest, _>(req) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let Some(file) = state.get(uri) else {
        return null_response(req_id);
    };
    let info = fossil_ide::hover_bidirectional(&state.db, file, pos.line, pos.character);
    let payload = info.map(|hi| Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hi.markdown,
        }),
        range: Some(byte_range_to_lsp_range(&state.db, file, hi.range)),
    });
    result_response(req_id, &payload)
}

/// `textDocument/definition` → [`fossil_ide::goto_definition`]; each
/// `NavigationTarget` (file + byte range) becomes a UTF-16 [`Location`].
fn handle_definition(state: &LspState, req: Request) -> Response {
    let req_id = req.id.clone();
    let params = match extract::<GotoDefinition, _>(req) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let Some(file) = state.get(uri) else {
        return null_response(req_id);
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
    result_response(req_id, &payload)
}

/// `textDocument/completion` → [`fossil_ide::completions`] (already returns
/// `lsp_types::CompletionItem`, no translation).
fn handle_completion(state: &LspState, req: Request) -> Response {
    let req_id = req.id.clone();
    let params = match extract::<Completion, _>(req) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let Some(file) = state.get(uri) else {
        return null_response(req_id);
    };
    let files = state.open_files();
    let items = fossil_ide::completions(&state.db, &files, file, pos.line, pos.character);
    result_response(req_id, &items)
}

/// `textDocument/documentSymbol` → [`fossil_ide::document_symbols`] (already
/// `lsp_types::DocumentSymbol`).
fn handle_document_symbol(state: &LspState, req: Request) -> Response {
    let req_id = req.id.clone();
    let params = match extract::<DocumentSymbolRequest, _>(req) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let Some(file) = state.get(&params.text_document.uri) else {
        return null_response(req_id);
    };
    let symbols = fossil_ide::document_symbols(&state.db, file);
    result_response(req_id, &DocumentSymbolResponse::Nested(symbols))
}

/// `textDocument/semanticTokens/full` → [`fossil_ide::semantic_tokens`]
/// (the spec-mandated flat `Vec<u32>` delta stream).
fn handle_semantic_tokens(state: &LspState, req: Request) -> Response {
    let req_id = req.id.clone();
    let params = match extract::<SemanticTokensFullRequest, _>(req) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let Some(file) = state.get(&params.text_document.uri) else {
        return null_response(req_id);
    };
    let data = fossil_ide::semantic_tokens(&state.db, file);
    let payload = SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: decode_to_lsp_tokens(&data),
    });
    result_response(req_id, &payload)
}

/// `textDocument/codeAction` → [`fossil_ide::code_actions`]. The request's own
/// diagnostics are passed through (the actions key off the structured
/// `did_you_mean` / `suggestion_source` fields — but the client sends the
/// published diagnostics back, so we re-derive them from the current document
/// to recover the structured carriers the LSP wire form drops).
fn handle_code_action(state: &LspState, req: Request) -> Response {
    let req_id = req.id.clone();
    let params = match extract::<CodeActionRequest, _>(req) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let Some(file) = state.get(&params.text_document.uri) else {
        return null_response(req_id);
    };
    // The wire `CodeActionParams.context.diagnostics` are stripped of our
    // structured carriers; re-drain to recover the full `Diagnostic`s (with
    // `did_you_mean` / `suggestion_source`). The SAME drain the publish path
    // uses, so a shape document offers no quick-fixes either — see
    // `fossil_ide::diagnostics`.
    let diagnostics = fossil_ide::diagnostics(&state.db, file);
    let actions = fossil_ide::code_actions(&state.db, file, params.range, &diagnostics);
    let payload: Vec<lsp_types::CodeActionOrCommand> = actions
        .into_iter()
        .map(lsp_types::CodeActionOrCommand::CodeAction)
        .collect();
    result_response(req_id, &payload)
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

/// Decode the params of a request typed by `R`. On a decode failure the `Err`
/// side is the null response to send back, per the LSP "MAY return null"
/// contract for providers.
fn extract<R, P>(req: Request) -> Result<P, Response>
where
    R: lsp_types::request::Request<Params = P>,
    P: serde::de::DeserializeOwned,
{
    let req_id = req.id.clone();
    match req.extract::<P>(R::METHOD) {
        Ok((_, p)) => Ok(p),
        Err(e) => {
            tracing::warn!("{} params decode failed: {e:?}", R::METHOD);
            Err(null_response(req_id))
        }
    }
}

/// `Response { result: <serde(payload)> }`.
///
/// A serialization failure becomes a JSON-RPC `InternalError` addressed to the
/// same id. It used to be a `?` out of the handler and up through `main_loop`,
/// which ENDED THE SERVER — an editor reads that as a clean exit and simply
/// stops having a language server.
fn result_response<T: serde::Serialize>(req_id: lsp_server::RequestId, payload: &T) -> Response {
    match serde_json::to_value(payload) {
        Ok(result) => Response {
            id: req_id,
            result: Some(result),
            error: None,
        },
        Err(e) => Response {
            id: req_id,
            result: None,
            error: Some(ResponseError {
                code: ErrorCode::InternalError as i32,
                message: format!("failed to serialise the response: {e}"),
                data: None,
            }),
        },
    }
}

/// `Response { result: null }` for a Request we couldn't fulfil but don't want
/// to error on (per LSP spec — providers MAY return null).
fn null_response(req_id: lsp_server::RequestId) -> Response {
    result_response(req_id, &serde_json::Value::Null)
}

/// `MethodNotFound` for any request not implemented above.
fn method_not_found(req: Request) -> Response {
    tracing::debug!("unhandled request: {}", req.method);
    Response {
        id: req.id,
        result: None,
        error: Some(ResponseError {
            code: ErrorCode::MethodNotFound as i32,
            message: format!("not implemented yet: {}", req.method),
            data: None,
        }),
    }
}

/// Apply one notification and return the `textDocument/publishDiagnostics`
/// notifications the caller must send.
///
/// `didOpen` / `didChange` record the buffer and publish the REAL
/// `parse → def_map → typecheck → lower` diagnostics. `didChange` mutates via
/// the Salsa `Setter` (revision bump → cancellation trigger). Anything else is
/// ignored and publishes nothing.
///
/// **This is what `tests/didchange_budget.rs` times.** The whole per-keystroke
/// cost is inside this call — the params decode, the source introspection, the
/// revision bump, the document registration, the drain and the LSP rendering —
/// so a step added anywhere in it is inside the gate's clock by construction
/// rather than by a test author noticing.
///
/// # Errors
///
/// A notification whose params do not decode to the type its method declares.
/// The caller ends the session on it, which is what this did before the move
/// and is deliberately unchanged here — the seam is the subject of this split,
/// not the error policy.
pub fn handle_notification(
    state: &mut LspState,
    notif: Notification,
) -> Result<Vec<Notification>, lsp_server::ExtractError<Notification>> {
    let published = match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = cast_notif::<DidOpenTextDocument>(notif)?;
            let uri = params.text_document.uri.clone();
            let path = uri.as_str().to_string();
            tracing::debug!("didOpen: {}", uri.as_str());
            let file = state.open(&uri, params.text_document.text, path);
            vec![publish_diagnostics(&state.db, &uri, file)]
        }
        DidChangeTextDocument::METHOD => {
            let params = cast_notif::<DidChangeTextDocument>(notif)?;
            let uri = params.text_document.uri.clone();
            // Full-sync model (TextDocumentSyncKind::FULL): the last change
            // carries the entire new document text.
            match params.content_changes.into_iter().last() {
                Some(last) => {
                    let path = uri.as_str().to_string();
                    // `change` mutates via `set_text` (Setter) → revision bump →
                    // cancels in-flight analysis from the previous keystroke
                    // (the real cancellation trigger).
                    let file = state.change(&uri, last.text, path);
                    tracing::debug!("didChange: {}", uri.as_str());
                    vec![publish_diagnostics(&state.db, &uri, file)]
                }
                None => Vec::new(),
            }
        }
        DidCloseTextDocument::METHOD => {
            let params = cast_notif::<DidCloseTextDocument>(notif)?;
            let uri = params.text_document.uri;
            tracing::debug!("didClose: {}", uri.as_str());
            state.close(&uri);
            // The spec's way of saying «nothing here any more». Without it the
            // squiggles from the last `didChange` stay on a file nobody has
            // open, and there is no longer anything to drain them from.
            vec![publish(&uri, Vec::new())]
        }
        m => {
            tracing::debug!("ignoring notification: {m}");
            Vec::new()
        }
    };
    Ok(published)
}

/// The `textDocument/publishDiagnostics` notification for `file`.
///
/// The payload is [`fossil_ide::lsp_diagnostics`] and nothing else — the drain,
/// the `claimed` guard that keeps a shape document from being checked as a
/// program, and the whole LSP rendering are one function shared with the browser
/// worker.
///
/// **All three of those lived here**, in a twin of `fossil-wasm`'s, and each was
/// got wrong independently on one side or the other: `d.labels` dropped on the
/// floor by both, the `claimed` guard added to the two a day apart, and the
/// worker publishing `related` where LSP says `relatedInformation`.
/// `crates/fossil-ide/src/diagnostics.rs` records which and when, and
/// `tests/transport_parity.rs` is what notices next time.
fn publish_diagnostics(db: &LspDb, uri: &Uri, file: SourceFile) -> Notification {
    publish(uri, fossil_ide::lsp_diagnostics(db, file))
}

/// One `textDocument/publishDiagnostics` notification.
///
/// Separate from [`publish_diagnostics`] because `didClose` publishes an EMPTY
/// list for a buffer that is no longer open, and there is nothing left to drain
/// it from.
fn publish(uri: &Uri, diagnostics: Vec<LspDiagnostic>) -> Notification {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    Notification {
        method: PublishDiagnostics::METHOD.to_string(),
        // `PublishDiagnosticsParams` is plain data with string keys throughout:
        // `to_value` fails only on a `Serialize` impl that errors or a
        // non-string map key, and there is neither. The `?` this replaced sent
        // the same impossible failure up through `main_loop`, ending the server.
        params: serde_json::to_value(&params).expect("PublishDiagnosticsParams serialises"),
    }
}

/// Translate a byte-offset range (from a `fossil-ide` feature) to a UTF-16 LSP
/// `Range`, resolving the file's memoised `LineIndex` first.
fn byte_range_to_lsp_range(db: &LspDb, file: SourceFile, range: std::ops::Range<u32>) -> Range {
    fossil_ide::byte_range_to_range(&fossil_ide::line_index(db, file), range)
}

/// Thin wrapper around [`Notification::extract`] — typed by `N`'s associated
/// `METHOD` constant so a method/params-type mismatch becomes a compile error.
fn cast_notif<N>(notif: Notification) -> Result<N::Params, lsp_server::ExtractError<Notification>>
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
