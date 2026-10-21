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
//! | [`WasmWorkspace::missing_documents`] | `Array<{ key, location, connection? }>` | `resolveDocuments` |
//! | [`WasmWorkspace::register_document`] | `()`                           | `resolveDocuments` |
//! | [`WasmWorkspace::inputs`]      | `Array<Input>`                       | introspection, lineage |
//! | [`WasmWorkspace::mapping`]     | `string` (Turtle)                    | the graph's RML mapping before a run |
//! | [`WasmWorkspace::register_inferred_descriptor`] | `()`                | introspection, a source described |
//! | [`WasmWorkspace::register_undescribed`] | `()`                        | introspection, a source that was not |
//! | [`WasmWorkspace::diagnostics`] | `Array<Diagnostic>`                  | `textDocument/diagnostic`, workspace-wide |
//! | [`WasmWorkspace::hover`]       | `{ markdown, range } \| null`        | `textDocument/hover`       |
//! | [`WasmWorkspace::completion`]  | `Array<CompletionItem>`              | `textDocument/completion`  |
//! | [`WasmWorkspace::definition`]  | `Array<Location>`                    | `textDocument/definition`  |
//! | [`WasmWorkspace::semantic_tokens`] | `Array<{ range, kind, modifiers }>` | `textDocument/semanticTokens/full` |
//!
//! Beside the workspace, [`profile::JsProfiler`] — `Profiler` to JS — is
//! `fossil-profile`'s plan, whose queries a host runs on its own `DuckDB`-WASM.
//!
//! The last four are the [`ide`] module: the `fossil-ide` answers the native
//! LSP serves, as ordinary method calls, because a tab that already
//! calls `diagnostics()` in-process should not have to stand up an LSP client to ask
//! what type is under a cursor. That module's header is the whole argument,
//! including why all four take a SHARED borrow and what a caller owes in
//! return.
//!
//! ## Architecture
//!
//! `fossil-wasm` is the language service in the browser: a thin transport
//! adapter, plain method calls, over the `fossil-ide` free functions.
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
//! build for `wasm32` before this shim could exist at all.

// `result_large_err`: the pure-Rust half refuses with
// `fossil_graph_schema::Failure` (144 bytes, over clippy's 128) on the path that
// ends the call, where the copy costs nothing measurable — `fossil-exec`'s crate
// root says the same.
#![allow(clippy::result_large_err)]

pub mod ide;
pub mod profile;
pub mod tokenize;
mod wasm_system;
mod workspace;

pub use crate::ide::{CompletionItem, CompletionKind, Hover, Location, SemanticToken};
pub use crate::tokenize::{Token, tokenize_native};

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use fossil_base::{FossilDb, SourceFile, System};
use fossil_graph_schema::js::{bug, invalid_argument};
use fossil_graph_schema::{Failure, Problem};
use fossil_hir::documents::MissingDocument;
use fossil_ide::LineIndex;
use lsp_types::{DiagnosticSeverity, Range};
use wasm_bindgen::prelude::*;

use crate::wasm_system::WasmSystem;
pub use crate::workspace::FileHandle;
use crate::workspace::OpenFiles;

/// JS-facing handle for the Fossil compiler running inside a WASM module.
///
/// One instance owns one [`FossilDb`] (Salsa store + injected [`WasmSystem`]).
/// Hosts construct a single workspace per browser tab / Node process and
/// reuse it across all method calls to amortise the Salsa interning +
/// memoisation overhead.
///
/// This is the RUST-side workspace and it is not the exported class: the JS
/// boundary is [`WasmWorkspace`], which holds one of these in a `RefCell` and
/// exports it under the name `FossilWorkspace`. That indirection is the fix
/// for a real defect — the type-level note on [`WasmWorkspace`] says which.
pub struct FossilWorkspace {
    db: FossilDb,
    /// The open-file lifecycle map (handle → `SourceFile` + URI index).
    /// Mutated by `open_file` / `update_file` / `close_file`; iterated by
    /// `diagnostics` / `diagnostics_for`.
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
        let db = FossilDb::new(Arc::new(WasmSystem) as Arc<dyn System>);
        Self {
            db,
            files: OpenFiles::default(),
        }
    }
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

/// A refused re-entrant call, `api/busy`.
///
/// Its help says what to do, because the host that hits this is an editor and
/// the fix is always the same shape: coalesce the edit stream.
fn busy(method: &str) -> Failure {
    Failure::new(Problem::Busy {
        call: method.to_string(),
    })
    .with_help(
        "coalesce edits (an LSP client debounces `didChange` rather than sending one per \
         keystroke) and retry — the workspace is still usable",
    )
}

// The types the signatures above name, from the one place they are declared.
#[wasm_bindgen(typescript_custom_section)]
const WIRE_TYPES: &str = "import type { Diagnostic, CompletionItem, Format, Hover, Input, \
    Location, Model, ModelType, Profile, ProfileOptions, ProfileQuery, ProfileSource, SemanticToken, \
    SourceVocabulary, Token, Vocabulary } \
    from '@fossil-lang/types';\n\
    import type { MissingDocument } from '@fossil-lang/types/internal';";

/// A value this crate built, as plain JS data. A serialiser refusing one is
/// fossil's fault, `internal/bug`.
pub(crate) fn to_value<T: serde::Serialize + ?Sized>(
    what: &str,
    value: &T,
) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(|e| bug(format!("serialising {what}"), e).into())
}

/// Diagnostic rows as plain JS data, through JSON rather than
/// [`to_value`]: a row's `data` is a `serde_json::Value`, and
/// `serde_wasm_bindgen` writes a map as an ES `Map`, which a host reading
/// `row.data.field` finds nothing on.
fn rows_to_value(rows: &[Diagnostic]) -> Result<JsValue, JsValue> {
    let text = serde_json::to_string(rows).map_err(|e| {
        bug(
            "serialising the check rows",
            JsValue::from_str(&e.to_string()),
        )
    })?;
    js_sys::JSON::parse(&text).map_err(|e| bug("parsing the check rows", e).into())
}

/// The failure an unknown or closed handle is.
const fn unknown_handle() -> Failure {
    Failure::new(Problem::UnknownHandle {})
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
    #[wasm_bindgen(js_name = openFile)]
    pub fn open_file(&self, path: String, contents: String) -> Result<FileHandle, JsValue> {
        let mut ws = self.inner.try_borrow_mut().map_err(|_| busy("openFile"))?;
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
    #[wasm_bindgen(js_name = updateFile)]
    pub fn update_file(&self, handle: &FileHandle, contents: String) -> Result<(), JsValue> {
        let mut ws = self
            .inner
            .try_borrow_mut()
            .map_err(|_| busy("updateFile"))?;
        Ok(ws.update_file_native(*handle, contents)?)
    }

    /// Close a file in the workspace. Idempotent in spirit but strict in
    /// signal: closing an unknown / already-closed handle is an error so
    /// JS-side bugs surface loudly (mirrors `ty_wasm`).
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` was never opened or was already closed,
    /// or if the workspace is busy.
    #[wasm_bindgen(js_name = closeFile)]
    pub fn close_file(&self, handle: &FileHandle) -> Result<(), JsValue> {
        let mut ws = self.inner.try_borrow_mut().map_err(|_| busy("closeFile"))?;
        Ok(ws.close_file_native(*handle)?)
    }

    /// Every open **program**'s diagnostics as a flat JS array of
    /// [`Diagnostic`]s keyed by file URI — the diagnostics panel's view. It is
    /// NOT the LSP wire's shape; see [`Diagnostic`].
    ///
    /// A buffer the installed provider catalogue claims — a `.shex` being
    /// edited, a `.csv`, a `.parquet` — is an INPUT and is not parsed as fossil.
    /// [`fossil_ide::diagnostics()`] is where that is said and measured, for both
    /// hosts.
    ///
    /// `range` is the UTF-16 LSP range (via `fossil_ide::LineIndex` — the
    /// rust-analyzer model). `severity` is the LSP integer constant
    /// (1 = error, 2 = warning, 3 = info). `message` is the problem rendered
    /// and nothing else; `help` is its own field.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`.
    #[wasm_bindgen(unchecked_return_type = "Diagnostic[]")]
    pub fn diagnostics(&self) -> Result<JsValue, JsValue> {
        // Native-side tests reach the pure-Rust core via `diagnostic_rows()`;
        // the wasm-bindgen wrapper just serializes. Separating the two
        // halves keeps `cargo test -p fossil-wasm` runnable without a JS
        // runtime (the `to_value` call panics on native targets — the
        // wasm-bindgen library's deliberate guard).
        let ws = self.inner.try_borrow().map_err(|_| busy("diagnostics"))?;
        rows_to_value(&ws.diagnostic_rows())
    }

    // ----- Documents and sources: fossil resolves, the host reads -----

    /// Replace the connections `@name/…` expands against, as
    /// `Host.connections()` answers them — an input, so what expanded one
    /// re-runs ([`fossil_base::set_connections`]).
    ///
    /// # Errors
    ///
    /// Returns a JS error if `connections` is not a string-to-string record, or
    /// if the workspace is busy.
    #[wasm_bindgen(js_name = setConnections)]
    pub fn set_connections(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Record<string, string>")] connections: JsValue,
    ) -> Result<(), JsValue> {
        let connections: BTreeMap<String, String> = serde_wasm_bindgen::from_value(connections)
            .map_err(|e| {
                invalid_argument(
                    "connections",
                    "an object of { name: baseUrl }",
                    Some(e.into()),
                )
            })?;
        self.inner
            .try_borrow_mut()
            .map_err(|_| busy("setConnections"))?
            .set_connections_native(connections);
        Ok(())
    }

    /// The documents the file at `handle` names that nothing has registered:
    /// `{ key, location }` rows, the location expanded through the connections.
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, if the workspace is busy, or
    /// if serialization fails.
    #[wasm_bindgen(js_name = missingDocuments, unchecked_return_type = "MissingDocument[]")]
    pub fn missing_documents(&self, handle: &FileHandle) -> Result<JsValue, JsValue> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy("missingDocuments"))?;
        let rows = ws
            .missing_documents_native(*handle)
            .ok_or_else(unknown_handle)?;
        to_value("the missing documents", &rows)
    }

    /// Register a fetched document's `text` under the `key` `missingDocuments`
    /// reported.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy.
    #[wasm_bindgen(js_name = registerDocument)]
    pub fn register_document(&self, key: &str, text: &str) -> Result<(), JsValue> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| busy("registerDocument"))?
            .register_document_native(key, text);
        Ok(())
    }

    /// What the file at `handle` reads — see [`fossil_hir::lineage::inputs`].
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, if the workspace is busy, or
    /// if serialization fails.
    #[wasm_bindgen(unchecked_return_type = "Input[]")]
    pub fn inputs(&self, handle: &FileHandle) -> Result<JsValue, JsValue> {
        let ws = self.inner.try_borrow().map_err(|_| busy("inputs"))?;
        let rows = ws.inputs_native(*handle).ok_or_else(unknown_handle)?;
        to_value("the inputs", &rows)
    }

    /// The RML-Core mapping the program's types describe — what `mapping.ttl` is, as Turtle, before
    /// any run ([`fossil_mir::describe`]). The types are the program's declarations and the
    /// documents it imports; a program with none describes nothing, and the mapping is empty.
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, if the workspace is busy, or with
    /// `write/ambiguous-iri` when two types or two columns of one share an IRI, the refusal a run
    /// would meet.
    #[wasm_bindgen]
    pub fn mapping(&self, handle: &FileHandle) -> Result<String, JsValue> {
        let ws = self.inner.try_borrow().map_err(|_| busy("mapping"))?;
        Ok(ws.mapping_native(*handle).ok_or_else(unknown_handle)??)
    }

    // ----- The main-thread IDE surface (see the `ide` module) -----
    //
    // All four take `&FileHandle` — wasm-bindgen CONSUMES an exported struct
    // passed by value, so a by-value handle is good for exactly one call and
    // the second throws "null pointer passed to rust" (the note on
    // `FileHandle` has the whole defect). Hover fires on mouse-move, so this
    // is the surface where that bug would be found again in one second rather
    // than in one keystroke.
    //
    // And all four take a SHARED borrow, because none of them mutates. That
    // is what lets an editor ask at four different rates against one
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
    #[wasm_bindgen(unchecked_return_type = "Hover | undefined")]
    pub fn hover(
        &self,
        handle: &FileHandle,
        line: u32,
        character: u32,
    ) -> Result<JsValue, JsValue> {
        let ws = self.inner.try_borrow().map_err(|_| busy("hover"))?;
        to_value("the hover", &ws.hover_row(*handle, line, character))
    }

    /// The completion candidates at a position: `{ label, kind, detail, insertText }` rows,
    /// already narrowed by the receiver — `str.` offers string members and no
    /// reader, a property key position offers the target shape's predicates and
    /// no catalogue row.
    ///
    /// `kind` is the LSP `CompletionItemKind` **by name** (`"function"`,
    /// `"field"`). See [`ide::CompletionKind`] for why a number does not cross
    /// this boundary.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`. An unknown handle is an empty array.
    #[wasm_bindgen(unchecked_return_type = "CompletionItem[]")]
    pub fn completion(
        &self,
        handle: &FileHandle,
        line: u32,
        character: u32,
    ) -> Result<JsValue, JsValue> {
        let ws = self.inner.try_borrow().map_err(|_| busy("completion"))?;
        to_value(
            "the completions",
            &ws.completion_rows(*handle, line, character),
        )
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
    #[wasm_bindgen(unchecked_return_type = "Location[]")]
    pub fn definition(
        &self,
        handle: &FileHandle,
        line: u32,
        character: u32,
    ) -> Result<JsValue, JsValue> {
        let ws = self.inner.try_borrow().map_err(|_| busy("definition"))?;
        to_value(
            "the definitions",
            &ws.definition_rows(*handle, line, character),
        )
    }

    /// Every classified span of the file: `{ range, kind, modifiers }` rows in
    /// source order, `kind` and `modifiers` by legend NAME. An editor lays these
    /// over its lexical highlighting — see [`ide::SemanticToken`].
    ///
    /// # Errors
    ///
    /// Returns a JS error if the workspace is busy, or if the result fails to
    /// serialize to `JsValue`. An unknown handle is an empty array.
    #[wasm_bindgen(js_name = semanticTokens, unchecked_return_type = "SemanticToken[]")]
    pub fn semantic_tokens(&self, handle: &FileHandle) -> Result<JsValue, JsValue> {
        let ws = self
            .inner
            .try_borrow()
            .map_err(|_| busy("semanticTokens"))?;
        to_value("the semantic tokens", &ws.semantic_token_rows(*handle))
    }

    // ----- Register a host-introspected descriptor -----

    /// Register an [`fossil_base::sources::InferredDescriptor`] for a
    /// source binding name BEFORE invoking `diagnostics`. The Rust compiler reads
    /// from this registration during forward type propagation — the browser has
    /// no filesystem to introspect a CSV from, so the column types must arrive
    /// from the host.
    ///
    /// Registering the same URI twice REPLACES the previous descriptor
    /// — intentional, since the host re-introspects when the source changes.
    /// It also forgets a problem [`Self::register_undescribed`] recorded under
    /// that URI: the latest answer about a source is the one `diagnostics` reports.
    ///
    /// # Errors
    ///
    /// - Malformed JSON / missing required fields → JS `Error` with the
    ///   underlying `serde_json` message.
    /// - The workspace is busy.
    #[wasm_bindgen(js_name = registerInferredDescriptor)]
    pub fn register_inferred_descriptor(&self, descriptor_json: &str) -> Result<(), JsValue> {
        let mut ws = self
            .inner
            .try_borrow_mut()
            .map_err(|_| busy("registerInferredDescriptor"))?;
        Ok(ws.register_inferred_descriptor_native(descriptor_json)?)
    }

    /// Record that the host could not describe the source the program wrote as
    /// `key`, and why: `problem_json` is the `Problem` its introspection
    /// answered (`{ code, data, help?, … }`; the fields the code does not carry
    /// are ignored).
    ///
    /// `diagnostics` then reports it at the binding that reads `key` — one warning
    /// per key, under the problem's own code — until a descriptor is registered
    /// under the same key, which forgets it. Registering a key twice replaces
    /// the problem.
    ///
    /// # Errors
    ///
    /// - `problem_json` is not a `Problem` fossil knows → `api/invalid-argument`.
    /// - The workspace is busy.
    #[wasm_bindgen(js_name = registerUndescribed)]
    pub fn register_undescribed(&self, key: &str, problem_json: &str) -> Result<(), JsValue> {
        let mut ws = self
            .inner
            .try_borrow_mut()
            .map_err(|_| busy("registerUndescribed"))?;
        Ok(ws.register_undescribed_native(key, problem_json)?)
    }
}

// ----- Pure-Rust core (test-reachable; no wasm-bindgen serialization) -----
//
// The `#[wasm_bindgen]` methods above (`diagnostics`, `diagnostics_for`)
// call `serde_wasm_bindgen::to_value` and build a `FossilError` through
// `fossil_graph_schema::js`, both of which call wasm-bindgen extern intrinsics that panic
// on native targets ("cannot call wasm-bindgen imported functions on
// non-wasm targets" — wasm-bindgen 0.2). Splitting the
// pure-Rust half out into a separate non-#[wasm_bindgen] impl block lets
// `cargo test -p fossil-wasm --test workspace` exercise the full lifecycle
// natively (the wasm-bindgen attribute layer is a transparent pass-through
// over these helpers — a passing native test guarantees the wire-side
// methods compile + dispatch correctly). What the pure half refuses with is a
// `Failure`, which is plain Rust; the wrappers turn it into the thrown value.

impl FossilWorkspace {
    /// Native-reachable workspace-wide diagnostic drain. Returns the same
    /// row structures `diagnostics()` serializes, without going through
    /// `serde_wasm_bindgen`.
    #[must_use]
    pub fn diagnostic_rows(&self) -> Vec<Diagnostic> {
        let mut all: Vec<Diagnostic> = Vec::new();
        for (h, file) in self.files.iter() {
            let uri = self.files.path_for(h, &self.db).unwrap_or_default();
            let index = fossil_ide::line_index(&self.db, file);
            for d in self.file_diagnostics(file) {
                all.push(to_diagnostic(&self.db, file, &uri, index, &d));
            }
        }
        all
    }

    /// The checker's diagnostics for `file`, then the sources it reads that the
    /// host could not describe.
    fn file_diagnostics(&self, file: SourceFile) -> Vec<fossil_base::Diagnostic> {
        let mut all = fossil_ide::diagnostics(&self.db, file);
        all.extend(fossil_hir::infer::refused_sources(&self.db, file));
        all
    }

    /// Native-reachable per-file diagnostic drain. Returns `None` when
    /// `handle` is unknown / closed (the wasm-bindgen wrapper throws
    /// `None` as `api/unknown-handle`).
    #[must_use]
    pub fn diagnostics_for_rows(&self, handle: FileHandle) -> Option<Vec<Diagnostic>> {
        let file = self.files.get(handle)?;
        let uri = self.files.path_for(handle, &self.db).unwrap_or_default();
        let index = fossil_ide::line_index(&self.db, file);
        Some(
            self.file_diagnostics(file)
                .into_iter()
                .map(|d| to_diagnostic(&self.db, file, &uri, index, &d))
                .collect(),
        )
    }

    /// Native-reachable update — pure-Rust mirror of `update_file`.
    ///
    /// # Errors
    ///
    /// Returns `api/unknown-handle` if `handle` was never
    /// opened or was already closed.
    pub fn update_file_native(
        &mut self,
        handle: FileHandle,
        contents: String,
    ) -> Result<(), Failure> {
        use salsa::Setter as _;
        let file = self.files.get(handle).ok_or_else(unknown_handle)?;
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
    /// Returns `api/unknown-handle` if `handle` was never
    /// opened or was already closed.
    pub fn close_file_native(&mut self, handle: FileHandle) -> Result<(), Failure> {
        self.files.remove(handle).ok_or_else(unknown_handle)?;
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
    pub fn set_connections_native(&mut self, connections: BTreeMap<String, String>) {
        fossil_base::set_connections(&mut self.db, connections);
    }

    /// Pure-Rust mirror of [`WasmWorkspace::missing_documents`]; `None` for an
    /// unknown handle.
    #[must_use]
    pub fn missing_documents_native(&self, handle: FileHandle) -> Option<Vec<MissingDocument>> {
        let file = self.files.get(handle)?;
        Some(fossil_hir::documents::missing_documents(&self.db, file))
    }

    /// Pure-Rust mirror of [`WasmWorkspace::register_document`].
    pub fn register_document_native(&mut self, key: &str, text: &str) {
        fossil_base::register_document(&mut self.db, key, text);
    }

    /// Pure-Rust mirror of [`WasmWorkspace::inputs`]; `None` for an unknown
    /// handle.
    #[must_use]
    pub fn inputs_native(&self, handle: FileHandle) -> Option<Vec<fossil_hir::lineage::Input>> {
        let file = self.files.get(handle)?;
        Some(fossil_hir::lineage::inputs(&self.db, file))
    }

    /// Pure-Rust mirror of [`WasmWorkspace::mapping`]; `None` for an unknown handle.
    #[must_use]
    pub fn mapping_native(&self, handle: FileHandle) -> Option<Result<String, Failure>> {
        let file = self.files.get(handle)?;
        Some(self.mapping_of(file))
    }

    fn mapping_of(&self, file: SourceFile) -> Result<String, Failure> {
        let schema = fossil_hir::graph_type::graph_type(&self.db, file)?;
        let mapping =
            fossil_mir::describe::describe(&schema, &fossil_mir::describe::Natural::new())?;
        Ok(fossil_mir::describe::turtle(&mapping)?)
    }

    // ----- Accessors for the editor surface in [`ide`] (pub(crate)) -----

    /// Handle → `SourceFile` lookup, for the main-thread surface in [`ide`].
    ///
    /// A caller already holds the handle `open_file` gave it, and making it
    /// re-derive a URI to get back to the file it just opened would be the sort
    /// of round trip [`Diagnostic::uri`]'s note exists to avoid.
    pub(crate) fn file_by_handle(&self, handle: FileHandle) -> Option<SourceFile> {
        self.files.get(handle)
    }

    /// Base `fossil_base::Db` accessor for every `fossil-ide` free function
    /// (hover, completion, goto-def, document-symbol, semantic-tokens,
    /// code-action — they all take `&dyn fossil_base::Db`).
    pub(crate) fn base_db(&self) -> &dyn fossil_base::Db {
        &self.db
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
    /// [`fossil_base::sources::InferredDescriptor`].
    ///
    /// # Errors
    ///
    /// What [`fossil_base::columns_from_json`] refuses. Nothing is recorded on
    /// error.
    pub fn register_inferred_descriptor_native(
        &mut self,
        descriptor_json: &str,
    ) -> Result<(), Failure> {
        let descriptor = fossil_base::columns_from_json(descriptor_json)?;
        fossil_base::describe_columns(&mut self.db, descriptor);
        Ok(())
    }

    /// Pure-Rust mirror of [`WasmWorkspace::register_undescribed`].
    ///
    /// # Errors
    ///
    /// What [`fossil_base::refusal_from_json`] refuses. Nothing is recorded on
    /// error.
    pub fn register_undescribed_native(
        &mut self,
        key: &str,
        problem_json: &str,
    ) -> Result<(), Failure> {
        let refused = fossil_base::refusal_from_json(problem_json)?;
        fossil_base::describe(&mut self.db, key, refused);
        Ok(())
    }

    /// The columns registered for a source, by the location the program writes.
    /// Lets cargo-tests verify the registration round-trips without going
    /// through the wasm-bindgen wrapper.
    #[must_use]
    pub fn inferred_descriptor_native(
        &self,
        uri: &str,
    ) -> Option<fossil_base::sources::InferredDescriptor> {
        fossil_base::columns(&self.db, uri)
    }
}

impl Default for FossilWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

/// The formats this host reads (`csv`, `rdf`, `shex`, …) as a JS array of
/// `{ name, extensions, kind }`. Pure projection of the provider registry — no
/// parsing, no db. `kind` is read off each row's capabilities, so the two
/// shape-document rows come back as `schema`.
///
/// # Errors
/// Returns a JS error only if the result fails to serialize to `JsValue`.
#[wasm_bindgen(unchecked_return_type = "Format[]")]
pub fn formats() -> Result<JsValue, JsValue> {
    to_value(
        "the formats",
        &fossil_hir::lineage::formats(fossil_hir::providers::PROVIDERS),
    )
}

/// One diagnostic row in the [`WasmWorkspace::diagnostics`] return array: the
/// workspace's own shape, not the LSP wire's (`lsp_types::Diagnostic`).
///
/// `diagnostics()` is one flat array across every open file, so each row carries the
/// `uri` the host opened its buffer under. Everything else is projected from the
/// rendering an editor is shown, so the two cannot disagree. `Diagnostic` in
/// `@fossil-lang/types` is this, with `data` typed by `code`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[schemars(rename = "DiagnosticBase")]
pub struct Diagnostic {
    pub uri: String,
    #[schemars(with = "fossil_ide::wire::RangeSchema")]
    pub range: Range,
    #[schemars(schema_with = "fossil_ide::wire::severity")]
    pub severity: DiagnosticSeverity,
    /// The problem's code — `area/kind`, what a host branches on.
    pub code: &'static str,
    /// The problem rendered, for a person. Nothing parses it.
    pub message: String,
    /// `title`, `data`, `help`, `didYouMean` and `suggestion`: LSP's `data`,
    /// flattened onto the row.
    #[serde(flatten)]
    pub detail: fossil_ide::DiagnosticData,
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
    pub related: Vec<DiagnosticRelatedInformation>,
}

/// One entry of [`Diagnostic::related`] — a place, and what is there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct DiagnosticRelatedInformation {
    pub uri: String,
    #[schemars(with = "fossil_ide::wire::RangeSchema")]
    pub range: Range,
    pub message: String,
}

/// Project one diagnostic onto the JS-side row shape.
///
/// The range, the severity and the message come from
/// [`fossil_ide::lsp_diagnostic`] and the rest from
/// [`fossil_ide::DiagnosticData::of`], which is what it sends as `data` — the
/// same rendering both LSP transports publish — so this cannot drift from what
/// an editor shows.
/// Only the two keys are this array's own; [`Diagnostic::related`] says why.
fn to_diagnostic(
    db: &FossilDb,
    file: SourceFile,
    uri: &str,
    index: &LineIndex,
    d: &fossil_base::Diagnostic,
) -> Diagnostic {
    let rendered = fossil_ide::lsp_diagnostic(db, file, index, d);
    Diagnostic {
        uri: uri.to_string(),
        range: rendered.range,
        severity: rendered.severity.unwrap_or(DiagnosticSeverity::ERROR),
        code: d.problem.code(),
        message: rendered.message,
        detail: fossil_ide::DiagnosticData::of(index, d),
        related: fossil_ide::related_locations(db, file, d)
            .into_iter()
            .map(|r| DiagnosticRelatedInformation {
                // A `LineIndex` per file: a UTF-16 column is a fact about the
                // text the range is in, and it is memoised, so the labels that
                // are in the program cost nothing extra.
                range: fossil_ide::range(fossil_ide::line_index(db, r.file), r.span),
                uri: r.file.path(db).clone(),
                message: r.text,
            })
            .collect(),
    }
}

/// The binding name a file becomes in a program — `people` for `people.csv`,
/// `_2024_sales` for `2024-sales.parquet` — always one `IDENT`.
/// `fossil_syntax::name::binding_name` is the rule, asked of the lexer.
#[wasm_bindgen(js_name = bindingName)]
#[must_use]
pub fn binding_name(file: &str) -> String {
    fossil_syntax::name::binding_name(file)
}

/// The `DuckDB` type a column of `primitive` has in a graph fossil wrote, before
/// anything has run: `fossil_graph_schema::Duckdb::from`, the writer's one type
/// per primitive.
///
/// # Errors
///
/// Returns a JS error if `primitive` is not a `Primitive`'s wire name.
#[wasm_bindgen(js_name = duckdbTypeOf)]
pub fn duckdb_type_of(
    #[wasm_bindgen(unchecked_param_type = "Primitive")] primitive: JsValue,
) -> Result<String, JsValue> {
    let primitive: fossil_graph_schema::Primitive = serde_wasm_bindgen::from_value(primitive)
        .map_err(|e| invalid_argument("primitive", "a Primitive", Some(e.into())))?;
    Ok(fossil_graph_schema::Duckdb::from(primitive)
        .as_str()
        .to_owned())
}
