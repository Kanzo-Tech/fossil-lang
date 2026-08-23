//! `fossil-wasm` — WASM host shim exposing [`FossilPlayground`] to JS.
//!
//! ## The surface
//!
//! A single `compile(source: &str)` came first; the surface is now the full
//! `ty_wasm`-shaped `Workspace` lifecycle the playground and the WASM LSP
//! Worker consume, because an editor edits files and re-checks them, and a
//! one-shot compile has nowhere to put the file identity that requires:
//!
//! | Method                          | Returns                              | Use site            |
//! |---------------------------------|--------------------------------------|---------------------|
//! | [`FossilPlayground::open_file`] | [`FileHandle`]                       | `textDocument/didOpen`     |
//! | [`FossilPlayground::update_file`]| `()`                                 | `textDocument/didChange`   |
//! | [`FossilPlayground::close_file`]| `()`                                 | `textDocument/didClose`    |
//! | [`FossilPlayground::check`]     | `Array<{ uri, range, severity, message }>` | LSP `publishDiagnostics` (workspace-wide) |
//! | [`FossilPlayground::diagnostics_for`] | `Array<{ uri, range, severity, message }>` | per-file `publishDiagnostics` (the worker's drain) |
//!
//! The single-shot `compile(&str)` is RETAINED beside it: a playground with one
//! buffer and no LSP client should not have to open and close a file to
//! type-check it, and the node smoke test calls exactly that entry point.
//!
//! ## Architecture
//!
//! `fossil-wasm` is the LSP server-side IN THE BROWSER — NOT a recompiled
//! `fossil-lsp` (which has a `compile_error!` cfg-tripwire because
//! `lsp-server` uses crossbeam + stdio). Both `fossil-lsp` (native, stdio)
//! and `fossil-wasm` (WASM, postMessage) are thin transport adapters over
//! the same `fossil-ide` free functions — "one crate, two hosts".
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

pub(crate) mod lsp_worker;
pub mod tokenize;
mod wasm_system;
mod workspace;

pub use crate::lsp_worker::start_lsp_worker;
pub use crate::tokenize::{TokenRow, tokenize_native};
// The #[wasm_bindgen] `tokenize` and `semantic_legend` functions are exposed
// to JS by virtue of their attribute. The `tokenize` module is `pub` so the
// `#[wasm_bindgen]` items are reachable (the unreachable_pub lint would
// otherwise flag them — they ARE reachable, just via wasm-bindgen-generated
// glue, not via Rust callers).

use std::sync::Arc;

use fossil_base::{Catalogue, Diagnostic, Files, Severity, SourceFile, Span, System};
use fossil_ide::{LineIndex, Utf16Position};
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
/// [`FossilPlayground::open_file_native`]) — the playground supplies documents,
/// not a contract.
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
/// Hosts construct a single playground per browser tab / Node process and
/// reuse it across all method calls to amortise the Salsa interning +
/// memoisation overhead.
#[wasm_bindgen]
pub struct FossilPlayground {
    db: WasmDb,
    /// The system handle is owned by `db` via `Arc<dyn System>`; we retain a
    /// typed `Arc<WasmSystem>` here to reach the in-memory filesystem — the
    /// shape-document loop reads through it, and future host wiring (e.g. a
    /// `VirtualFS` import) writes to it — without round-tripping through the
    /// trait object.
    system: Arc<WasmSystem>,
    /// The open-file lifecycle map (handle → `SourceFile` + URI index).
    /// Mutated by `open_file` / `update_file` / `close_file`; iterated by
    /// `check` / `diagnostics_for`.
    files: OpenFiles,
}

impl std::fmt::Debug for FossilPlayground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FossilPlayground")
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

#[wasm_bindgen]
impl FossilPlayground {
    /// Construct a new playground.
    ///
    /// Installs `console_error_panic_hook` (idempotent —
    /// Don't-Hand-Roll #8) so any panic inside compiler-core surfaces as a
    /// `console.error` stack trace in the host (browser `DevTools` or Node).
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        let system = Arc::new(WasmSystem::default());
        let db = WasmDb::new(system.clone() as Arc<dyn System>);
        Self {
            db,
            system,
            files: OpenFiles::default(),
        }
    }

    // A `classification()` method sat here, returning the `{ name, wasm_class }`
    // manifest for the playground to gray out the native-only functions. There
    // are none: see the tombstone below `inferred_descriptor_native`.

    // ----- Workspace lifecycle (the ty_wasm pattern) -----

    /// Open a file in the workspace. Returns a [`FileHandle`] the JS side
    /// keys subsequent `update_file` / `close_file` / `compile_file` calls
    /// on. `path` is the URI / virtual path the diagnostics carry back to
    /// the LSP client.
    ///
    /// Mirrors `ty_wasm::Workspace::open_file` (Astral). Interns a fresh
    /// [`fossil_base::SourceFile`] under the current Salsa revision.
    ///
    /// # Errors
    ///
    /// Returns a JS error only if the internal counter is exhausted
    /// (`u32::MAX` files — would require a runaway loop in JS, not a normal
    /// failure mode).
    pub fn open_file(&mut self, path: String, contents: String) -> Result<FileHandle, JsError> {
        Ok(self.open_file_native(path, contents))
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
    /// Returns a JS error if `handle` was never opened or was already closed.
    pub fn update_file(&mut self, handle: FileHandle, contents: String) -> Result<(), JsError> {
        self.update_file_native(handle, contents)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Close a file in the workspace. Idempotent in spirit but strict in
    /// signal: closing an unknown / already-closed handle is an error so
    /// JS-side bugs surface loudly (mirrors `ty_wasm`).
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` was never opened or was already closed.
    pub fn close_file(&mut self, handle: FileHandle) -> Result<(), JsError> {
        self.close_file_native(handle)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Run `parse → def_map → typecheck_mapping` across every open **program**
    /// and return a flat JS array of `{ uri, range, severity, message }` rows
    /// keyed by file URI. The LSP Worker republishes these grouped
    /// by URI as `textDocument/publishDiagnostics` notifications.
    ///
    /// A buffer the installed provider catalogue claims — the `.shex` the
    /// playground had to open to give the compiler a document, a `.csv`, a
    /// `.parquet` — is an INPUT and is not parsed as fossil, and
    /// `diagnostics_for_file` in this file is where that is said and measured.
    ///
    /// `range` is the UTF-16 LSP range (via `fossil_ide::LineIndex` — the
    /// rust-analyzer model). `severity` is the LSP integer
    /// constant (1 = error, 2 = warning, 3 = info). `message` carries any
    /// `suggestion_source` as a `\nhelp: ...` suffix, mirroring
    /// `fossil-lsp`'s `to_lsp_diagnostic`.
    ///
    /// # Errors
    ///
    /// Returns a JS error only if the result fails to serialize to `JsValue`.
    pub fn check(&self) -> Result<JsValue, JsError> {
        // Native-side tests reach the pure-Rust core via `check_rows()`;
        // the wasm-bindgen wrapper just serializes. Separating the two
        // halves keeps `cargo test -p fossil-wasm` runnable without a JS
        // runtime (the `to_value` call panics on native targets — the
        // wasm-bindgen library's deliberate guard).
        serde_wasm_bindgen::to_value(&self.check_rows()).map_err(JsError::from)
    }

    /// Per-file diagnostic drain — the accessor the LSP Worker consumes for its
    /// per-file `publishDiagnostics` notifications.
    /// `check()` returns the workspace-wide flat array; `diagnostics_for`
    /// returns just one file's rows so the worker can dispatch one notification
    /// per affected URI without partitioning the workspace array on the JS
    /// side.
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, or if serialization fails.
    pub fn diagnostics_for(&self, handle: FileHandle) -> Result<JsValue, JsError> {
        let rows = self
            .diagnostics_for_rows(handle)
            .ok_or_else(|| JsError::new(&WorkspaceError::UnknownHandle.to_string()))?;
        serde_wasm_bindgen::to_value(&rows).map_err(JsError::from)
    }

    // ----- Register a host-introspected descriptor -----

    /// Register an [`fossil_descriptors_input::InferredDescriptor`] for a
    /// source binding name BEFORE invoking `Self::compile` /
    /// `Self::compile_file`. The Rust compiler reads from this registration
    /// during forward type propagation — the browser has no filesystem to
    /// introspect a CSV from, so the column types must arrive from the host.
    ///
    /// `descriptor_json` is the JSON serialisation of `InferredDescriptor`;
    /// the canonical shape is exposed in `packages/wasm/src/index.ts` as
    /// `InferredDescriptorJson`:
    ///
    /// ```json
    /// {
    ///   "uri": "examples/users.csv",
    ///   "columns": [
    ///     { "name": "id", "primitive": "integer" },
    ///     { "name": "name", "primitive": "string" }
    ///   ],
    ///   "freshness_token": ""
    /// }
    /// ```
    ///
    /// Called by the browser-side playground orchestration AFTER running
    /// DuckDB-WASM `DESCRIBE read_csv_auto('<resolved-url>')` and BEFORE
    /// invoking `compile()` / `compile_file()`. Keyed by the source URI as the
    /// program writes it (`"examples/users.csv"` for
    /// `users := io.csv("examples/users.csv")`), NOT by the binding name and
    /// NOT by the resolved URL the host fetched.
    ///
    /// Idempotent: re-registering the same `uri` OVERWRITES the previous entry
    /// — intentional, since the host re-introspects when the source changes.
    /// A host that can tell whether it changed puts a token in
    /// `freshness_token` and skips the `DESCRIBE` when the cache agrees.
    ///
    /// # Errors
    ///
    /// - Malformed JSON / missing required fields → JS `Error` with the
    ///   underlying `serde_json` message.
    ///
    /// Implementation: thin shim over the pure-Rust
    /// [`Self::register_inferred_descriptor_native`] helper.
    #[wasm_bindgen(js_name = registerInferredDescriptor)]
    pub fn register_inferred_descriptor(&self, descriptor_json: &str) -> Result<(), JsError> {
        self.register_inferred_descriptor_native(descriptor_json)
            .map_err(|e| JsError::new(&e.to_string()))
    }
}

// ----- Pure-Rust core (test-reachable; no wasm-bindgen serialization) -----
//
// The `#[wasm_bindgen]` methods above (`check`, `diagnostics_for`,
// `compile_file`) call `serde_wasm_bindgen::to_value` and construct
// `JsError`s, both of which call wasm-bindgen extern intrinsics that panic
// on native targets ("cannot call wasm-bindgen imported functions on
// non-wasm targets" — wasm-bindgen 0.2 lib.rs:101). Splitting the
// pure-Rust half out into a separate non-#[wasm_bindgen] impl block lets
// `cargo test -p fossil-wasm --test workspace` exercise the full lifecycle
// natively (the wasm-bindgen attribute layer is a transparent pass-through
// over these helpers — a passing native test guarantees the wire-side
// methods compile + dispatch correctly). Mirrors the `classification()` ↔
// `stdlib_classification()` split that established the convention.

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
    /// The file parsed to zero mappings — `compile_file` has nothing to
    /// compile. The `check` path is fine with this case (it returns an
    /// empty diagnostic stream).
    NoMappingInFile,
    /// The `register_inferred_descriptor` JSON payload did not deserialise
    /// into an [`fossil_descriptors_input::InferredDescriptor`]. Carries the
    /// underlying `serde_json` error message.
    MalformedDescriptor(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownHandle => f.write_str("unknown file handle"),
            Self::NoMappingInFile => f.write_str("no mapping found in file"),
            Self::MalformedDescriptor(msg) => {
                write!(f, "malformed InferredDescriptor JSON: {msg}")
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl FossilPlayground {
    /// Native-reachable workspace-wide diagnostic drain. Returns the same
    /// row structures `check()` serializes, without going through
    /// `serde_wasm_bindgen`.
    #[must_use]
    pub fn check_rows(&self) -> Vec<CheckRow> {
        let mut all: Vec<CheckRow> = Vec::new();
        for (h, file) in self.files.iter() {
            let uri = self.files.path_for(h, &self.db).unwrap_or_default();
            let index = fossil_ide::line_index(&self.db, file);
            for d in diagnostics_for_file(&self.db, file) {
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
            diagnostics_for_file(&self.db, file)
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
        // The edit may have just added the `type { … } = io.shex("…")` line
        // that names a document. A no-op once the document is in (the loop
        // skips what the registry already holds), and the `def_map` it consults
        // is the one `check` is about to run anyway.
        self.register_named_documents(file);
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
    /// Two registrations happen here, and they are different things. The buffer
    /// goes into the file registry under its own path, so that opening a
    /// `.shex` makes the OPEN COPY the document every program naming it reads —
    /// the editor's buffer is the truth, not whatever the host staged. Then the
    /// documents THIS file names are registered if nobody has them yet.
    pub fn open_file_native(&mut self, path: String, contents: String) -> FileHandle {
        let file = fossil_base::SourceFile::new(&self.db, contents, path.clone());
        fossil_base::register_file(&mut self.db, path.clone(), file);
        let handle = self.files.insert(path, file);
        self.register_named_documents(file);
        handle
    }

    /// Register every shape document `file` names that is not in the database
    /// already — see [`fossil_ide::register_missing_documents`].
    ///
    /// The playground's filesystem is [`WasmSystem`]'s in-memory map, which is
    /// empty unless a host staged something in it. So in the browser this
    /// normally registers NOTHING, and that is the honest answer: a document
    /// the host never opened is not there. It is not a dead end either — the
    /// registry is a Salsa input, so `open_file`ing that document later
    /// re-executes every query that missed it. The playground's way to give the
    /// compiler a shape document is to open it.
    fn register_named_documents(&mut self, file: fossil_base::SourceFile) {
        let system = &self.system;
        fossil_ide::register_missing_documents(&mut self.db, file, &|key| {
            let bytes = system.read_file(std::path::Path::new(key)).ok()?;
            String::from_utf8(bytes).ok()
        });
    }

    // ----- LSP-worker dispatch helpers (pub(crate)) -----
    //
    // These accessors are consumed by `lsp_worker::dispatch` to route LSP
    // requests / notifications onto the existing fossil-ide free functions
    // without leaking the `WasmDb` type or duplicating bookkeeping. They
    // mirror the analogous `LspState` accessors in `fossil-lsp`.

    /// URI → `FileHandle` lookup used by `textDocument/didChange` /
    /// `didClose` / custom `fossil/compileFile` dispatch.
    pub(crate) fn lookup_handle_by_uri(&self, uri: &str) -> Option<FileHandle> {
        self.files.lookup_uri(uri)
    }

    /// URI → `SourceFile` lookup used by request handlers (hover, completion,
    /// goto-def, etc.) that consume Salsa inputs directly.
    pub(crate) fn lookup_file_by_uri(&self, uri: &str) -> Option<SourceFile> {
        let handle = self.files.lookup_uri(uri)?;
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

    /// Drain Salsa `Diagnostic` accumulators for `file` in their structured
    /// form (still carrying `did_you_mean` / `suggestion_source`). Used by
    /// `textDocument/codeAction` to re-derive the carriers the wire form
    /// drops — mirrors fossil-lsp's `diagnostics_for`.
    pub(crate) fn drain_diagnostics_for_file(&self, file: SourceFile) -> Vec<Diagnostic> {
        diagnostics_for_file(&self.db, file)
    }

    // ----- Inferred-descriptor registration -----

    /// Pure-Rust mirror of [`Self::register_inferred_descriptor`] (the
    /// `#[wasm_bindgen]` wrapper).
    ///
    /// Cargo-tests call THIS function — the wasm-bindgen wrapper panics on
    /// the native test target (wasm-bindgen 0.2 lib.rs:101). Mirrors the
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

// `FnClassification` and `stdlib_classification()` lived here — one row per
// stdlib function, carrying `"pure_sql"` or `"native_udf_only"`, so the browser
// playground could render the native-only functions as disabled.
//
// Ruling 15 of `SURFACE-PLAN.md` deleted the `Udf` lowering kind, and with it
// the `WasmClass` concept the manifest projected. Every catalogued function is
// a pure SQL expression template now, so there is nothing to disable and a
// manifest saying `"pure_sql"` fifty-one times says nothing at all. **The
// language runs entirely in the browser**, which is what the manifest existed
// to deny.

impl Default for FossilPlayground {
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
// delegate to the shared, WASM-clean `fossil_registry` implementation — the SAME
// code `fossil-engine` runs natively — so the browser and the CLI can never
// diverge.
//
// Free functions (not `FossilPlayground` methods): they are stateless and
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
pub fn refs_native(program: &str) -> Vec<fossil_run_status::SourceRefInfo> {
    let system = Arc::new(WasmSystem::default()) as Arc<dyn System>;
    let db = WasmDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), "<refs>".to_string());
    fossil_lineage::source_refs(&db, file)
}

/// One diagnostic row in the [`FossilPlayground::check`] return array.
///
/// Mirrors the LSP `Diagnostic` shape exactly so the LSP Worker can
/// republish each row as-is inside a `PublishDiagnosticsParams` payload
/// without a second translation step. UTF-16 ranges; integer LSP severities.
///
/// Pub-visible for the native cargo-test path (`check_rows`,
/// `diagnostics_for_rows`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckRow {
    pub uri: String,
    pub range: CheckRange,
    pub severity: u8,
    pub message: String,
    /// The other places this one diagnostic points at — LSP's
    /// `relatedInformation`, which is the concept for a report whose content is
    /// a RELATION between two places.
    ///
    /// Empty for nearly every diagnostic. Non-empty when the mistake needs a
    /// second underline: two mappings minting two identities for one type, the
    /// binding a row came from, and the line of the `.shex` a violated
    /// constraint is declared on — which is in ANOTHER FILE, and is why each
    /// entry carries its own `uri`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<CheckRelated>,
}

/// One entry of [`CheckRow::related`] — a place, and what is there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckRelated {
    pub uri: String,
    pub range: CheckRange,
    pub message: String,
}

/// LSP-shaped range (zero-based line + UTF-16 column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CheckRange {
    pub start: CheckPosition,
    pub end: CheckPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CheckPosition {
    pub line: u32,
    pub character: u32,
}

/// Every diagnostic `file` produces, from the one implementation of that
/// question — [`fossil_mir::program_diagnostics`], which `fossil-engine` and
/// `fossil-lsp` also call.
///
/// **This used to be a per-mapping loop of its own**, byte-identical to
/// `fossil-lsp`'s and three drains short of `fossil check`'s. What the browser
/// did not show, for as long as that was true: a file the parser recovered no
/// mapping from produced NO rows at all (the parse errors were present and
/// unreachable), a top-level binding's provider errors vanished, two mappings
/// minting two identities for one type was never checked, and one top-level
/// mistake was reported once per mapping.
///
/// # A file the catalogue reads is not drained as a program
///
/// [`FossilPlayground::check_rows`] loops over the whole workspace and calls
/// this for each handle, and in the playground the only way to hand the
/// compiler a shape document is to open it. So this used to parse `person.shex`
/// as fossil and attribute its errors to it — **twenty-one rows** for the
/// `ShExJ` document of `tests/shape_document.rs`: `unexpected token` eleven
/// times, `expected IDENT, found STRING` eight, `expected IDENT, found INDENT`
/// once and `expected DEFINE, found DEDENT` at the closing brace, for a file
/// that is not wrong. In an editor that is squiggles down the length of the
/// user's `ShEx`. The `ShExC` document in the same file measured fourteen, and
/// **three of those claimed an internal compiler error** (`mapping has no HIR
/// at its DefMap index`) — a shape declaration parses far enough to look like a
/// mapping header and then has no HIR.
///
/// It was invisible while the browser's drain was the per-mapping loop, because
/// a `.shex` produces no mappings. Reading the file-level accumulators, which
/// is where `parse` lives, is what surfaced it.
///
/// **The notion of «which open files are programs» was already in the tree**,
/// and the note that stood here saying it was not is what took the longest to
/// disprove. It is the provider catalogue: a row declares the extensions it
/// accepts, [`fossil_base::claimed`] asks all of them, and a URI some row reads
/// is an INPUT to a program rather than a program. The host installs
/// `fossil_descriptors_output::PROVIDERS` (`wasm_system.rs`), so `.shex` /
/// `.shexj` / `.shexc` / `.ttl` / `.shacl` are claimed alongside `.csv` /
/// `.json` / `.parquet`, and nothing about fossil's syntax is decided here —
/// the question is «does something read this», and only the catalogue answers
/// it.
///
/// Two things this deliberately does not do. It does not consult a **program**
/// extension: `.fossil` is a convention, a URI with no extension is claimed by
/// nobody, and the default is to check, so the failure mode is the old
/// behaviour rather than silence. And it does not deregister the document — the
/// buffer is still the text the checker decodes, so a broken `ShEx` is still
/// reported, on the program that names it and with a label pointing into the
/// document (`tests/shape_document.rs`). What has no home is a document nobody
/// names: it is not checked, because there is nothing to check it against.
fn diagnostics_for_file(db: &WasmDb, file: SourceFile) -> Vec<Diagnostic> {
    if fossil_base::claimed(fossil_base::installed(db), file.path(db)) {
        return Vec::new();
    }
    fossil_mir::program_diagnostics(db, file)
}

/// Convert one `fossil_base::Diagnostic` to the JS-side row shape. UTF-16
/// range conversion via [`fossil_ide::LineIndex`]; `suggestion_source` folded
/// into the message as a `help:` suffix (mirroring `fossil-lsp::
/// to_lsp_diagnostic` — keep the structured carriers reachable by
/// re-draining the accumulator on the consumer side).
fn to_check_row(
    db: &WasmDb,
    file: SourceFile,
    uri: &str,
    index: &LineIndex,
    d: &Diagnostic,
) -> CheckRow {
    let range = span_to_range(index, d.span);
    let message = d.suggestion_source.as_ref().map_or_else(
        || d.message.clone(),
        |s| format!("{}\nhelp: {s}", d.message),
    );
    // **The labels reach the browser now, and none of them did.** This dropped
    // `d.labels` exactly as `fossil-lsp`'s twin did, so a report naming two
    // mappings arrived as one squiggle. `fossil_ide::related_locations` answers
    // which file each label is in — one answer, rendered here into the JS row
    // shape and there into `DiagnosticRelatedInformation`.
    let related = fossil_ide::related_locations(db, file, d)
        .into_iter()
        .map(|r| CheckRelated {
            // A `LineIndex` per file: a UTF-16 column is a fact about the text
            // the range is in, and it is memoised, so the labels that are in
            // the program cost nothing extra.
            range: span_to_range(&fossil_ide::line_index(db, r.file), r.span),
            uri: r.file.path(db).clone(),
            message: r.text,
        })
        .collect();
    CheckRow {
        uri: uri.to_string(),
        range,
        severity: severity_to_lsp_int(d.severity),
        message,
        related,
    }
}

/// Map `fossil_base::Severity` to the LSP integer constant the playground /
/// LSP Worker expects (1 = error, 2 = warning, 3 = info).
const fn severity_to_lsp_int(s: Severity) -> u8 {
    match s {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
    }
}

/// Translate a byte-offset [`Span`] to a UTF-16 LSP-shaped range.
fn span_to_range(index: &LineIndex, span: Span) -> CheckRange {
    CheckRange {
        start: utf16_to_pos(fossil_ide::offset_to_lsp_position(index, span.start)),
        end: utf16_to_pos(fossil_ide::offset_to_lsp_position(index, span.end)),
    }
}

const fn utf16_to_pos(p: Utf16Position) -> CheckPosition {
    CheckPosition {
        line: p.line,
        character: p.character,
    }
}

// ----- LSP dispatch test hook -----
//
// The LSP-worker `dispatch` function is `pub(crate)`; native integration
// tests in `crates/fossil-wasm/tests/lsp_worker.rs` reach it through this
// `#[doc(hidden)]` shim. The shim deserializes a `serde_json::Value` (the
// shape every test builds) into the typed `LspRequest` and forwards.

/// Dispatch test hook — not part of the published JS surface.
///
/// # Panics
///
/// Panics if `req` is not a valid LSP JSON-RPC payload (intentional — tests
/// must not feed it malformed JSON).
#[doc(hidden)]
#[must_use]
pub fn __dispatch_for_test(
    pg: &mut FossilPlayground,
    req: serde_json::Value,
) -> DispatchTestOutput {
    let parsed: lsp_worker::LspRequest = serde_json::from_value(req).expect("malformed test req");
    let out = lsp_worker::dispatch(pg, parsed);
    DispatchTestOutput {
        response: out.response.map(|r| DispatchTestResponse {
            id: r.id,
            result: r.result,
            error: r.error.map(|e| DispatchTestError {
                code: e.code,
                message: e.message,
            }),
        }),
        diagnostics: out.diagnostics,
    }
}

/// Native-test-only response wrapper. The `lsp_worker::LspResponse` /
/// `LspError` types are `pub(crate)`; this mirrors them so the test surface
/// has stable field access.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct DispatchTestOutput {
    pub response: Option<DispatchTestResponse>,
    pub diagnostics: Vec<serde_json::Value>,
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct DispatchTestResponse {
    pub id: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<DispatchTestError>,
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct DispatchTestError {
    pub code: i32,
    pub message: String,
}
