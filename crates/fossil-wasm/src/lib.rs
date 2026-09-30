//! `fossil-wasm` — WASM host shim exposing [`FossilWorkspace`] to JS.
//!
//! ## The surface
//!
//! The surface is the `ty_wasm`-shaped `Workspace` lifecycle an editor host
//! consumes, because an editor edits files and re-checks
//! them, and a one-shot compile has nowhere to put the file identity that
//! requires:
//!
//! | Method                          | Returns                              | Use site            |
//! |---------------------------------|--------------------------------------|---------------------|
//! | [`WasmWorkspace::open_file`]   | [`FileHandle`]                       | `textDocument/didOpen`     |
//! | [`WasmWorkspace::update_file`] | `()`                                 | `textDocument/didChange`   |
//! | [`WasmWorkspace::close_file`]  | `()`                                 | `textDocument/didClose`    |
//! | [`WasmWorkspace::set_connections`] | `()`                             | the host's `Host.connections()` |
//! | [`WasmWorkspace::missing_documents`] | `Array<{ key, locator, connection? }>` | `resolveDocuments` |
//! | [`WasmWorkspace::register_document`] | `()`                           | `resolveDocuments` |
//! | [`WasmWorkspace::sources`]     | `Array<ProgramSource>`               | introspection |
//! | [`WasmWorkspace::check`]       | `Array<{ uri, range, severity, message }>` | a diagnostics panel, workspace-wide |
//! | [`WasmWorkspace::diagnostics_for`] | `Array<{ uri, range, severity, message }>` | the same rows scoped to one file |
//! | [`WasmWorkspace::hover`]       | `{ markdown, range } \| null`        | `textDocument/hover`       |
//! | [`WasmWorkspace::completions`] | `Array<{ label, kind, detail }>`     | `textDocument/completion`  |
//! | [`WasmWorkspace::goto_definition`] | `Array<{ uri, range }>`          | `textDocument/definition`  |
//!
//! The last three are the [`ide`] module: the `fossil-ide` answers the native
//! LSP serves, as ordinary method calls, because a tab that already
//! calls `check()` in-process should not have to stand up an LSP client to ask
//! what type is under a cursor. That module's header is the whole argument,
//! including why all three take a SHARED borrow and what a caller owes in
//! return.
//!
//! ## Architecture
//!
//! `fossil-wasm` is the language service IN THE BROWSER — NOT a recompiled
//! `fossil-lsp` (which has a `compile_error!` cfg-tripwire because
//! `lsp-server` uses crossbeam + stdio). Both `fossil-lsp` (native, JSON-RPC
//! over stdio) and `fossil-wasm` (WASM, plain method calls) are thin transport
//! adapters over the same `fossil-ide` free functions — "one crate, two hosts".
//!
//! ## Why the Workspace lifecycle is fan-out-safe
//!
//! `update_file` mutates the SAME [`fossil_base::SourceFile`] input via the
//! Salsa [`salsa::Setter`] (`set_text`) — the EXACT mechanism the native LSP's
//! `didChange` path uses (the revision bump is the cancellation
//! trigger). NO new tracked queries land in the lifecycle path, so
//! `MAX_PER_MAPPING_FAN_OUT` stays at 1, which
//! `fossil-hir`'s `invalidation_regression` test is what proves.
//!
//! WASM-first is load-bearing, not a port: everything above `fossil-ide` had to
//! build for `wasm32` before this shim could exist at all, which is why the
//! `compile_error!` tripwires live on the native-only crates rather than here.

pub mod ide;
pub mod tokenize;
mod wasm_system;
mod workspace;

pub use crate::ide::{CompletionRow, DefinitionRow, HoverRow};
pub use crate::tokenize::{TokenRow, token_kinds_native, tokenize_native};
// The #[wasm_bindgen] `tokenize` and `semantic_legend` functions are exposed
// to JS by virtue of their attribute. The `tokenize` module is `pub` so the
// `#[wasm_bindgen]` items are reachable (the unreachable_pub lint would
// otherwise flag them — they ARE reachable, just via wasm-bindgen-generated
// glue, not via Rust callers).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use fossil_base::{Catalogue, Diagnostic, Files, SourceFile, System};
use fossil_ide::LineIndex;
use lsp_types::{DiagnosticSeverity, Range};
use wasm_bindgen::prelude::*;

use crate::wasm_system::WasmSystem;
pub use crate::workspace::FileHandle;
use crate::workspace::OpenFiles;

/// The Salsa database the WASM host owns.
///
/// Mirrors `fossil-lsp::LspDb`: the Salsa runtime + the host
/// [`System`] (here [`WasmSystem`]) + the file registry. The target-side `ShEx`
/// type/properties `fossil-ide` surfaces are reachable because the PROGRAM
/// names its output document and the host REGISTERS it (see
/// [`FossilWorkspace::register_document_native`]) — the host supplies
/// documents, not a contract.
#[salsa::db]
#[derive(Clone)]
struct WasmDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    catalogue: Catalogue,
}

impl std::fmt::Debug for WasmDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmDb").finish_non_exhaustive()
    }
}

#[salsa::db]
impl salsa::Database for WasmDb {}

#[salsa::db]
impl fossil_base::Db for WasmDb {
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

impl WasmDb {
    fn new(system: Arc<dyn System>) -> Self {
        Self {
            storage: salsa::Storage::default(),
            system,
            files: Files::default(),
            catalogue: Catalogue::default(),
        }
    }
}

/// JS-facing handle for the Fossil compiler running inside a WASM module.
///
/// One instance owns one [`WasmDb`] (Salsa store + injected [`WasmSystem`]).
/// Hosts construct a single workspace per browser tab / Node process and
/// reuse it across all method calls to amortise the Salsa interning +
/// memoisation overhead.
///
/// This is the RUST-side workspace and it is not the exported class: the JS
/// boundary is [`WasmWorkspace`], which holds one of these in a `RefCell` and
/// exports it under the name `FossilWorkspace`. That indirection is the fix
/// for a real defect — the type-level note on [`WasmWorkspace`] says which.
pub struct FossilWorkspace {
    db: WasmDb,
    /// The system handle is owned by `db` via `Arc<dyn System>`; we retain a
    /// typed `Arc<WasmSystem>` here so descriptor registration reaches its
    /// cache without round-tripping through the trait object.
    system: Arc<WasmSystem>,
    /// Connection name → base, as `@name/…` expands against it. Outside Salsa:
    /// it reaches locators only, never a registry key, so setting it
    /// invalidates nothing.
    connections: HashMap<String, String>,
    /// The open-file lifecycle map (handle → `SourceFile` + URI index).
    /// Mutated by `open_file` / `update_file` / `close_file`; iterated by
    /// `check` / `diagnostics_for`.
    files: OpenFiles,
}

impl std::fmt::Debug for FossilWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FossilWorkspace")
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

impl FossilWorkspace {
    /// Construct a new workspace.
    ///
    /// Installs `console_error_panic_hook` (idempotent) so any panic inside
    /// compiler-core surfaces as a `console.error` stack trace in the host
    /// (browser `DevTools` or Node).
    #[must_use]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        let system = Arc::new(WasmSystem::default());
        let db = WasmDb::new(system.clone() as Arc<dyn System>);
        Self {
            db,
            system,
            connections: HashMap::new(),
            files: OpenFiles::default(),
        }
    }

    // A `classification()` method sat here, returning the `{ name, wasm_class }`
    // manifest for an editor to gray out the native-only stdlib functions.
    // There are none: `crates/fossil-hir/src/stdlib.rs` records the removal.
}

/// The JS-facing workspace — `FossilWorkspace` on the JS side, and a
/// `RefCell` around the Rust one.
///
/// # Why the interior `RefCell`, and it is not a style choice
///
/// `wasm-bindgen` wraps every exported struct in its own `WasmRefCell`. A
/// method taking `&mut self` borrows that cell EXCLUSIVELY for the call, and a
/// failed borrow does not return — it panics, with
/// *"recursive use of an object detected which would lead to unsafe aliasing in
/// rust"*. On `wasm32-unknown-unknown` a panic is an abort: the trap unwinds
/// nothing, so the borrow flag it was holding is never cleared, and **every
/// later call on that object fails the same way for the life of the module.**
/// One bad call poisons the workspace permanently.
///
/// That is a real defect and it was reachable from correct host code: an
/// editor reproduced it by typing sixteen characters faster than its
/// debounce, and had to carry a `busy` flag and a 120 ms coalesce to stay
/// out of it. A mitigation a host has to remember is not a fix — and an editor
/// is precisely the workload that forgets.
///
/// So no exported method takes `&mut self`. Every one takes `&self`, which
/// wasm-bindgen borrows SHARED — shared borrows nest, so re-entry cannot fail
/// there — and the mutation goes through this `RefCell` with
/// `try_borrow_mut`. Genuine re-entry (a JS callback that calls back in while a
/// call is live) now returns a catchable `Error` naming what happened, and
/// **the workspace is still usable afterwards**, because a returned `Err` drops
/// its guard where a panic did not.
///
/// The Rust-side [`FossilWorkspace`] keeps its `&mut self` signatures
/// untouched: the native tests drive it directly. Only the JS boundary changed.
#[wasm_bindgen(js_name = FossilWorkspace)]
pub struct WasmWorkspace {
    inner: RefCell<FossilWorkspace>,
}

impl std::fmt::Debug for WasmWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmWorkspace")
            .field("inner", &self.inner)
            .finish()
    }
}

impl Default for WasmWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

/// The message a refused re-entrant call carries.
///
/// It names the method and says what to do, because the host that hits this is
/// an editor and the fix is always the same shape: coalesce the edit stream.
fn busy_error(method: &str) -> JsError {
    JsError::new(&format!(
        "fossil workspace is busy: `{method}` was called while another call on \
         the same workspace was still running. Coalesce edits (an LSP client \
         debounces `didChange` rather than sending one per keystroke) and retry \
         — the workspace is still usable."
    ))
}

#[wasm_bindgen(js_class = FossilWorkspace)]
impl WasmWorkspace {
    /// Construct a new workspace.
    ///
    /// Installs `console_error_panic_hook` (idempotent) so any panic inside
    /// compiler-core surfaces as a `console.error` stack trace in the host
    /// (browser `DevTools` or Node).
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(FossilWorkspace::new()),
        }
    }

    // ----- Workspace lifecycle (the ty_wasm pattern) -----

    /// Open a file in the workspace. Returns a [`FileHandle`] the JS side
    /// keys subsequent `update_file` / `close_file` calls on. `path` is the
    /// URI / virtual path the diagnostics carry back to the LSP client.
    ///
    /// Mirrors `ty_wasm::Workspace::open_file` (Astral). Interns a fresh
    /// [`fossil_base::SourceFile`] under the current Salsa revision.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is already inside another call —
    /// see the type-level note on [`WasmWorkspace`].
    pub fn open_file(&self, path: String, contents: String) -> Result<FileHandle, JsError> {
        let mut ws = self
            .inner
            .try_borrow_mut()
            .map_err(|_| busy_error("open_file"))?;
        Ok(ws.open_file_native(path, contents))
    }

    /// Apply an edit to an open file. Mutates the SAME `SourceFile` via the
    /// Salsa [`salsa::Setter`] (`set_text`) — this BUMPS THE REVISION, the
    /// real cancellation trigger: any in-flight analysis from the
    /// previous keystroke observes the new revision at its next cooperative
    /// checkpoint. NO new `SourceFile` is interned — the Salsa input
    /// identity stays stable across the file's lifetime, so memoised
    /// downstream queries (`def_map`, `typecheck_mapping`) invalidate
    /// incrementally instead of falling off a cliff.
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` was never opened or was already closed,
    /// or if the workspace is already inside another call — see the type-level
    /// note on [`WasmWorkspace`]. **Neither leaves the workspace unusable**,
    /// which is the whole reason this method takes `&self`.
    pub fn update_file(&self, handle: &FileHandle, contents: String) -> Result<(), JsError> {
        let mut ws = self
            .inner
            .try_borrow_mut()
            .map_err(|_| busy_error("update_file"))?;
        ws.update_file_native(*handle, contents)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Close a file in the workspace. Idempotent in spirit but strict in
    /// signal: closing an unknown / already-closed handle is an error so
    /// JS-side bugs surface loudly (mirrors `ty_wasm`).
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` was never opened or was already closed,
    /// or if the workspace is busy.
    pub fn close_file(&self, handle: &FileHandle) -> Result<(), JsError> {
        let mut ws = self
            .inner
            .try_borrow_mut()
            .map_err(|_| busy_error("close_file"))?;
        ws.close_file_native(*handle)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Every open **program**'s diagnostics as a flat JS array of
    /// `{ uri, range, severity, message }` rows keyed by file URI — the
    /// diagnostics panel's view. It is NOT the LSP wire's shape; see
    /// [`CheckRow`].
    ///
    /// A buffer the installed provider catalogue claims — a `.shex` being
    /// edited, a `.csv`, a `.parquet` — is an INPUT and is not parsed as fossil.
    /// [`fossil_ide::diagnostics()`] is where that is said and measured, for both
    /// hosts.
    ///
    /// `range` is the UTF-16 LSP range (via `fossil_ide::LineIndex` — the
    /// rust-analyzer model). `severity` is the LSP integer constant
    /// (1 = error, 2 = warning, 3 = info). `message` carries any
    /// `suggestion_source` as a `\nhelp: ...` suffix. All three come from
    /// [`fossil_ide::lsp_diagnostic`], the rendering an editor is shown.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`.
    pub fn check(&self) -> Result<JsValue, JsError> {
        // Native-side tests reach the pure-Rust core via `check_rows()`;
        // the wasm-bindgen wrapper just serializes. Separating the two
        // halves keeps `cargo test -p fossil-wasm` runnable without a JS
        // runtime (the `to_value` call panics on native targets — the
        // wasm-bindgen library's deliberate guard).
        let ws = self.inner.try_borrow().map_err(|_| busy_error("check"))?;
        serde_wasm_bindgen::to_value(&ws.check_rows()).map_err(JsError::from)
    }

    /// Per-file diagnostic drain: `check()` returns the workspace-wide flat
    /// array, this returns one file's rows, so a host can refresh one buffer's
    /// panel without partitioning the workspace array on the JS side. It is NOT
    /// the LSP wire's shape — that is [`fossil_ide::lsp_diagnostics`].
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, if the workspace is busy, or
    /// if serialization fails.
    pub fn diagnostics_for(&self, handle: &FileHandle) -> Result<JsValue, JsError> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy_error("diagnostics_for"))?;
        let rows = ws
            .diagnostics_for_rows(*handle)
            .ok_or_else(|| JsError::new(&WorkspaceError::UnknownHandle.to_string()))?;
        serde_wasm_bindgen::to_value(&rows).map_err(JsError::from)
    }

    // ----- Documents and sources: fossil resolves, the host reads -----

    /// Replace the connection map `@name/…` expands against, as
    /// `Host.connections()` answers it.
    ///
    /// # Errors
    ///
    /// Returns a JS error if `connections` is not a string-to-string record, or
    /// if the workspace is busy.
    #[wasm_bindgen(js_name = setConnections)]
    pub fn set_connections(&self, connections: JsValue) -> Result<(), JsError> {
        let connections: HashMap<String, String> =
            serde_wasm_bindgen::from_value(connections).map_err(JsError::from)?;
        self.inner
            .try_borrow_mut()
            .map_err(|_| busy_error("setConnections"))?
            .set_connections_native(connections);
        Ok(())
    }

    /// The documents the file at `handle` names that nothing has registered:
    /// `{ key, locator }` rows, the locator expanded through the connection map.
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, if the workspace is busy, or
    /// if serialization fails.
    #[wasm_bindgen(js_name = missingDocuments)]
    pub fn missing_documents(&self, handle: &FileHandle) -> Result<JsValue, JsError> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy_error("missingDocuments"))?;
        let rows = ws
            .missing_documents_native(*handle)
            .ok_or_else(|| JsError::new(&WorkspaceError::UnknownHandle.to_string()))?;
        serde_wasm_bindgen::to_value(&rows).map_err(JsError::from)
    }

    /// Register a fetched document's `text` under the `key` `missingDocuments`
    /// reported.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy.
    #[wasm_bindgen(js_name = registerDocument)]
    pub fn register_document(&self, key: &str, text: &str) -> Result<(), JsError> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| busy_error("registerDocument"))?
            .register_document_native(key, text);
        Ok(())
    }

    /// The data sources the file at `handle` reads — see
    /// [`fossil_lineage::program_sources`].
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, if the workspace is busy, or
    /// if serialization fails.
    pub fn sources(&self, handle: &FileHandle) -> Result<JsValue, JsError> {
        let ws = self.inner.try_borrow().map_err(|_| busy_error("sources"))?;
        let rows = ws
            .sources_native(*handle)
            .ok_or_else(|| JsError::new(&WorkspaceError::UnknownHandle.to_string()))?;
        serde_wasm_bindgen::to_value(&rows).map_err(JsError::from)
    }

    // ----- The main-thread IDE surface (see the `ide` module) -----
    //
    // All three take `&FileHandle` — wasm-bindgen CONSUMES an exported struct
    // passed by value, so a by-value handle is good for exactly one call and
    // the second throws "null pointer passed to rust" (the note on
    // `FileHandle` has the whole defect). Hover fires on mouse-move, so this
    // is the surface where that bug would be found again in one second rather
    // than in one keystroke.
    //
    // And all three take a SHARED borrow, because none of them mutates. That
    // is what lets an editor ask at three different rates against one
    // workspace without the coalescing `update_file` needs.

    /// What is under the cursor: `{ markdown, range }`, or `null` when the
    /// cursor is on whitespace, on an expression the checker inferred no type
    /// for, or outside any mapping.
    ///
    /// `line` / `character` are LSP — zero-based, and `character` in UTF-16
    /// code units, which is what a JS host counts in anyway. The `range` comes
    /// back in the same units.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`. An unknown handle is `null`, not an error —
    /// see [`FossilWorkspace::hover_row`].
    pub fn hover(
        &self,
        handle: &FileHandle,
        line: u32,
        character: u32,
    ) -> Result<JsValue, JsError> {
        let ws = self.inner.try_borrow().map_err(|_| busy_error("hover"))?;
        serde_wasm_bindgen::to_value(&ws.hover_row(*handle, line, character)).map_err(JsError::from)
    }

    /// The completion candidates at a position: `{ label, kind, detail }` rows,
    /// already narrowed by the receiver — `str.` offers string members and no
    /// reader, a property key position offers the target shape's predicates and
    /// no catalogue row.
    ///
    /// `kind` is the LSP `CompletionItemKind` **by name** (`"function"`,
    /// `"field"`). See [`ide::CompletionRow`] for why a number does not cross
    /// this boundary.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`. An unknown handle is an empty array.
    pub fn completions(
        &self,
        handle: &FileHandle,
        line: u32,
        character: u32,
    ) -> Result<JsValue, JsError> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy_error("completions"))?;
        serde_wasm_bindgen::to_value(&ws.completion_rows(*handle, line, character))
            .map_err(JsError::from)
    }

    /// Where the name under the cursor is defined: `{ uri, range }` rows, empty
    /// when nothing there has a definition.
    ///
    /// `uri` is the key the host opened the buffer under, verbatim — and two of
    /// the three positions goto-def recognises resolve into the shape document,
    /// so a host with one pane has to read it before moving a cursor.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`.
    #[wasm_bindgen(js_name = gotoDefinition)]
    pub fn goto_definition(
        &self,
        handle: &FileHandle,
        line: u32,
        character: u32,
    ) -> Result<JsValue, JsError> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy_error("gotoDefinition"))?;
        serde_wasm_bindgen::to_value(&ws.definition_rows(*handle, line, character))
            .map_err(JsError::from)
    }

    // ----- Register a host-introspected descriptor -----

    /// Register an [`fossil_descriptors_input::InferredDescriptor`] for a
    /// source binding name BEFORE invoking `check`. The Rust compiler reads
    /// from this registration during forward type propagation — the browser has
    /// no filesystem to introspect a CSV from, so the column types must arrive
    /// from the host.
    ///
    /// Registering the same URI twice REPLACES the previous descriptor
    /// — intentional, since the host re-introspects when the source changes.
    ///
    /// # Errors
    ///
    /// - Malformed JSON / missing required fields → JS `Error` with the
    ///   underlying `serde_json` message.
    /// - The workspace is busy.
    #[wasm_bindgen(js_name = registerInferredDescriptor)]
    pub fn register_inferred_descriptor(&self, descriptor_json: &str) -> Result<(), JsError> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy_error("registerInferredDescriptor"))?;
        ws.register_inferred_descriptor_native(descriptor_json)
            .map_err(|e| JsError::new(&e.to_string()))
    }
}

// ----- Pure-Rust core (test-reachable; no wasm-bindgen serialization) -----
//
// The `#[wasm_bindgen]` methods above (`check`, `diagnostics_for`)
// call `serde_wasm_bindgen::to_value` and construct
// `JsError`s, both of which call wasm-bindgen extern intrinsics that panic
// on native targets ("cannot call wasm-bindgen imported functions on
// non-wasm targets" — wasm-bindgen 0.2). Splitting the
// pure-Rust half out into a separate non-#[wasm_bindgen] impl block lets
// `cargo test -p fossil-wasm --test workspace` exercise the full lifecycle
// natively (the wasm-bindgen attribute layer is a transparent pass-through
// over these helpers — a passing native test guarantees the wire-side
// methods compile + dispatch correctly).

/// Pure-Rust error returned by the `*_native` / `*_rows` / `*_result`
/// helpers.
///
/// The `#[wasm_bindgen]` wrappers translate this to `JsError` (the
/// JS-facing error type) so native tests never construct a `JsError`
/// directly — wasm-bindgen's intrinsic-construction routines panic on
/// non-wasm32 targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// The handle was never opened, or was already closed.
    UnknownHandle,
    /// The `register_inferred_descriptor` JSON payload did not deserialise
    /// into an [`fossil_descriptors_input::InferredDescriptor`]. Carries the
    /// underlying `serde_json` error message.
    MalformedDescriptor(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownHandle => f.write_str("unknown file handle"),
            Self::MalformedDescriptor(msg) => {
                write!(f, "malformed InferredDescriptor JSON: {msg}")
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl FossilWorkspace {
    /// Native-reachable workspace-wide diagnostic drain. Returns the same
    /// row structures `check()` serializes, without going through
    /// `serde_wasm_bindgen`.
    #[must_use]
    pub fn check_rows(&self) -> Vec<CheckRow> {
        let mut all: Vec<CheckRow> = Vec::new();
        for (h, file) in self.files.iter() {
            let uri = self.files.path_for(h, &self.db).unwrap_or_default();
            let index = fossil_ide::line_index(&self.db, file);
            for d in fossil_ide::diagnostics(&self.db, file) {
                all.push(to_check_row(&self.db, file, &uri, &index, &d));
            }
        }
        all
    }

    /// Native-reachable per-file diagnostic drain. Returns `None` when
    /// `handle` is unknown / closed (the wasm-bindgen wrapper translates
    /// `None` to a `JsError`).
    #[must_use]
    pub fn diagnostics_for_rows(&self, handle: FileHandle) -> Option<Vec<CheckRow>> {
        let file = self.files.get(handle)?;
        let uri = self.files.path_for(handle, &self.db).unwrap_or_default();
        let index = fossil_ide::line_index(&self.db, file);
        Some(
            fossil_ide::diagnostics(&self.db, file)
                .into_iter()
                .map(|d| to_check_row(&self.db, file, &uri, &index, &d))
                .collect(),
        )
    }

    /// Native-reachable update — pure-Rust mirror of `update_file`.
    ///
    /// # Errors
    ///
    /// Returns `Err(WorkspaceError::UnknownHandle)` if `handle` was never
    /// opened or was already closed.
    pub fn update_file_native(
        &mut self,
        handle: FileHandle,
        contents: String,
    ) -> Result<(), WorkspaceError> {
        use salsa::Setter as _;
        let file = self
            .files
            .get(handle)
            .ok_or(WorkspaceError::UnknownHandle)?;
        file.set_text(&mut self.db).to(contents);
        Ok(())
    }

    /// Native-reachable close — pure-Rust mirror of `close_file`.
    ///
    /// The file leaves the workspace but STAYS in the registry. Deregistering
    /// would be right if there were a disk to fall back to; here there is not,
    /// so dropping a closed `.shex` would delete the only copy of it the
    /// compiler has and silently turn off the output contract of every program
    /// naming it.
    ///
    /// # Errors
    ///
    /// Returns `Err(WorkspaceError::UnknownHandle)` if `handle` was never
    /// opened or was already closed.
    pub fn close_file_native(&mut self, handle: FileHandle) -> Result<(), WorkspaceError> {
        self.files
            .remove(handle)
            .ok_or(WorkspaceError::UnknownHandle)?;
        Ok(())
    }

    /// Native-reachable open — pure-Rust mirror of `open_file`. Returns the
    /// fresh handle directly (panics on `u32::MAX` counter overflow, same
    /// as the wasm-bindgen wrapper).
    ///
    /// The buffer goes into the file registry under its own path, so an open
    /// `.shex` is the document every program naming it reads: the editor's
    /// buffer is the truth, not a copy the host fetched.
    pub fn open_file_native(&mut self, path: String, contents: String) -> FileHandle {
        let file = fossil_base::SourceFile::new(&self.db, contents, path.clone());
        fossil_base::register_file(&mut self.db, path, file);
        self.files.insert(file)
    }

    /// Pure-Rust mirror of [`WasmWorkspace::set_connections`].
    pub fn set_connections_native(&mut self, connections: HashMap<String, String>) {
        self.connections = connections;
    }

    /// Pure-Rust mirror of [`WasmWorkspace::missing_documents`]; `None` for an
    /// unknown handle.
    #[must_use]
    pub fn missing_documents_native(&self, handle: FileHandle) -> Option<Vec<MissingDocumentRow>> {
        let file = self.files.get(handle)?;
        Some(
            fossil_hir::documents::missing_documents(&self.db, file, &self.connections)
                .into_iter()
                .map(|d| MissingDocumentRow {
                    key: d.key,
                    locator: d.locator,
                    connection: d.connection,
                })
                .collect(),
        )
    }

    /// Pure-Rust mirror of [`WasmWorkspace::register_document`].
    pub fn register_document_native(&mut self, key: &str, text: &str) {
        fossil_base::register_document(&mut self.db, key, text);
    }

    /// Pure-Rust mirror of [`WasmWorkspace::sources`]; `None` for an unknown
    /// handle.
    #[must_use]
    pub fn sources_native(&self, handle: FileHandle) -> Option<Vec<fossil_lineage::ProgramSource>> {
        let file = self.files.get(handle)?;
        Some(fossil_lineage::program_sources(
            &self.db,
            file,
            &self.connections,
        ))
    }

    // ----- Accessors for the editor surface in [`ide`] (pub(crate)) -----

    /// Handle → `SourceFile` lookup, for the main-thread surface in [`ide`].
    ///
    /// A caller already holds the handle `open_file` gave it, and making it
    /// re-derive a URI to get back to the file it just opened would be the sort
    /// of round trip [`CheckRow::uri`]'s note exists to avoid.
    pub(crate) fn file_by_handle(&self, handle: FileHandle) -> Option<SourceFile> {
        self.files.get(handle)
    }

    /// Base `fossil_base::Db` accessor for every `fossil-ide` free function
    /// (hover, completion, goto-def, document-symbol, semantic-tokens,
    /// code-action — they all take `&dyn fossil_base::Db`).
    pub(crate) fn base_db(&self) -> &dyn fossil_base::Db {
        &self.db
    }

    /// Snapshot of currently-open `SourceFile`s — fed to `goto_definition` /
    /// `completions` for their workspace-wide name resolution pass.
    pub(crate) fn open_source_files(&self) -> Vec<SourceFile> {
        self.files.iter().map(|(_, f)| f).collect()
    }

    // ----- Inferred-descriptor registration -----

    /// Pure-Rust mirror of [`WasmWorkspace::register_inferred_descriptor`] (the
    /// `#[wasm_bindgen]` wrapper).
    ///
    /// Cargo-tests call THIS function — the wasm-bindgen wrapper panics on
    /// the native test target (wasm-bindgen 0.2). Mirrors the
    /// `*_native` / `*_result` / `*_rows` convention documented in
    /// `tests/workspace.rs`.
    ///
    /// `descriptor_json` is the JSON serialisation of
    /// [`fossil_descriptors_input::InferredDescriptor`] — see
    /// `packages/wasm/src/index.ts` `InferredDescriptorJson` for the
    /// canonical shape.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::MalformedDescriptor`] if the JSON fails to
    /// deserialise. The descriptor table is not modified on error.
    pub fn register_inferred_descriptor_native(
        &self,
        descriptor_json: &str,
    ) -> Result<(), WorkspaceError> {
        let descriptor: fossil_descriptors_input::InferredDescriptor =
            serde_json::from_str(descriptor_json)
                .map_err(|e| WorkspaceError::MalformedDescriptor(e.to_string()))?;
        if let Some(cache) = self.system.descriptors() {
            cache.insert(descriptor);
        }
        Ok(())
    }

    /// Native-reachable lookup into the descriptor cache, by source URI.
    /// Lets cargo-tests verify the registration round-trips without going
    /// through the wasm-bindgen wrapper.
    #[must_use]
    pub fn inferred_descriptor_native(
        &self,
        uri: &str,
    ) -> Option<fossil_descriptors_input::InferredDescriptor> {
        self.system.descriptors()?.get(uri)
    }
}

impl Default for FossilWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

// ----- Parse-only lineage + providers (one crate, two hosts) -----
//
// `refs` and `providers` mirror the native `fossil refs` / `fossil providers`
// CLI commands for the BROWSER host: keasy's client-compute job runner reads a
// program's typed lineage (which `@conn`s + `schema =` it references) and the
// supported source providers WITHOUT subprocessing the `fossil` binary. Both
// delegate to `fossil_lineage` (WASM-clean) over
// `fossil_descriptors_output::PROVIDERS` — the same calls
// `fossil-df/tests/provider_registry.rs` makes natively.
//
// Free functions (not `FossilWorkspace` methods): they are stateless and
// program-text-driven (the job runner has the script string, not the editor's
// open-file workspace), mirroring the existing free `tokenize` export.

/// The providers this host installs (`io.csv`, `io.rdf`, `io.shex`, …) as a JS
/// array of `{ name, extensions, kind }`. Pure projection of the provider
/// registry — no parsing, no db. `kind` is read off each row's capabilities, so
/// the two shape-document rows come back as `Schema`.
///
/// # Errors
/// Returns a JS error only if the result fails to serialize to `JsValue`.
#[wasm_bindgen]
pub fn providers() -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(&fossil_lineage::providers(
        fossil_descriptors_output::PROVIDERS,
    ))
    .map_err(JsError::from)
}

/// Parse `program` and return its external references — every data URI +
/// `schema =` argument, each tagged with its `@conn` alias (`null` for a direct
/// path) and role (`Data` / `Schema`) — as a JS array of
/// `{ connection, path, role }`. Parse-only; the host resolves `@conn` →
/// `{base}/path` itself (its data-plane job, kept out of fossil's semantics).
///
/// # Errors
/// Returns a JS error only if the result fails to serialize to `JsValue`.
#[wasm_bindgen]
pub fn refs(program: &str) -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(&refs_native(program)).map_err(JsError::from)
}

/// Native-reachable core of [`refs`] — builds a transient single-file db and
/// runs the shared [`fossil_lineage::source_refs`]. Cargo-tests call THIS: the
/// `#[wasm_bindgen]` wrapper's `serde_wasm_bindgen` / `JsError` calls panic on
/// native targets (same split as `check` ↔ `check_rows`). The db is throwaway
/// (refs is parse-only and called once per job launch, not per keystroke), so
/// it never touches the editor's persistent workspace.
#[must_use]
pub fn refs_native(program: &str) -> Vec<fossil_lineage::SourceRefInfo> {
    let system = Arc::new(WasmSystem::default()) as Arc<dyn System>;
    let db = WasmDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), "<refs>".to_string());
    fossil_lineage::source_refs(&db, file)
}

/// `fossil_hir::documents::MissingDocument` in the shape it crosses to JS —
/// `MissingDocument` in `@fossil-lang/types`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MissingDocumentRow {
    pub key: String,
    pub locator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
}

/// One diagnostic row in the [`WasmWorkspace::check`] return array.
///
/// The workspace's own shape, and **not the LSP wire's**, which is
/// `lsp_types::Diagnostic` ([`fossil_ide::lsp_diagnostics`]). This type's
/// docblock once said it «mirrors the LSP `Diagnostic` shape exactly so the LSP
/// Worker can republish each row as-is», and the worker did, so an extra `uri`
/// and a `related` spelled nothing like `relatedInformation` went out on the
/// wire.
///
/// What it is for is `check()`: one flat array across every open file, which
/// needs a `uri` per row precisely because it is not per-file. The rows are
/// keyed by the path the HOST opened the buffer under, which in the browser is
/// often a bare name and not a URI — see [`Self::related`].
///
/// Everything but those keys is projected from the shared rendering, so the
/// message, the severity and the ranges here cannot disagree with what an editor
/// is shown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckRow {
    pub uri: String,
    pub range: Range,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// The other places this one diagnostic points at — LSP's
    /// `relatedInformation`, in this array's own shape.
    ///
    /// Empty for nearly every diagnostic. Non-empty when the mistake needs a
    /// second underline: two mappings minting two identities for one type, the
    /// binding a row came from, and the line of the `.shex` a violated
    /// constraint is declared on — which is in ANOTHER FILE, and is why each
    /// entry carries its own key.
    ///
    /// `uri` here is the registry key verbatim, matching [`Self::uri`], and NOT
    /// put through `fossil_ide::file_uri`: a caller filtering this array by the
    /// path it opened a file under has to find the same string in both fields.
    /// The LSP wire does convert, because LSP will not accept anything else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<CheckRelated>,
}

/// One entry of [`CheckRow::related`] — a place, and what is there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckRelated {
    pub uri: String,
    pub range: Range,
    pub message: String,
}

/// Project one diagnostic onto the JS-side row shape.
///
/// The range, the severity and the `help:`-suffixed message come from
/// [`fossil_ide::lsp_diagnostic`] — the same rendering both LSP transports
/// publish — so this cannot drift from what an editor shows. Only the two keys
/// are this array's own; [`CheckRow::related`] says why.
fn to_check_row(
    db: &WasmDb,
    file: SourceFile,
    uri: &str,
    index: &LineIndex,
    d: &Diagnostic,
) -> CheckRow {
    let rendered = fossil_ide::lsp_diagnostic(db, file, index, d);
    CheckRow {
        uri: uri.to_string(),
        range: rendered.range,
        severity: rendered.severity.unwrap_or(DiagnosticSeverity::ERROR),
        message: rendered.message,
        related: fossil_ide::related_locations(db, file, d)
            .into_iter()
            .map(|r| CheckRelated {
                // A `LineIndex` per file: a UTF-16 column is a fact about the
                // text the range is in, and it is memoised, so the labels that
                // are in the program cost nothing extra.
                range: fossil_ide::span_to_range(&fossil_ide::line_index(db, r.file), r.span),
                uri: r.file.path(db).clone(),
                message: r.text,
            })
            .collect(),
    }
}

// `diagnostics_for_file`, `severity_to_lsp_int`, `span_to_range` and
// `utf16_to_pos` lived here, and every one of them had a twin in
// `fossil-lsp/src/main.rs`. They are `crates/fossil-ide/src/diagnostics.rs` now.
// The measurements this file used to carry — what a shape document produced
// before the `claimed` guard — are in
// `crates/fossil-wasm/tests/documents_are_not_programs.rs`.
