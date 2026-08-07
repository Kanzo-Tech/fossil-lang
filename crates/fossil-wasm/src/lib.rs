//! `fossil-wasm` — WASM host shim exposing [`FossilPlayground`] to JS.
//!
//! ## Phase 7 surface (this commit — WASM-02 / plan 07-02)
//!
//! Phase 7 grows the surface from the original Phase-1 single
//! `compile(source: &str)` into the full `ty_wasm`-shaped `Workspace`
//! lifecycle the playground and the WASM LSP Worker (07-03) consume:
//!
//! | Method                          | Returns                              | Use site            |
//! |---------------------------------|--------------------------------------|---------------------|
//! | [`FossilPlayground::open_file`] | [`FileHandle`]                       | `textDocument/didOpen`     |
//! | [`FossilPlayground::update_file`]| `()`                                 | `textDocument/didChange`   |
//! | [`FossilPlayground::close_file`]| `()`                                 | `textDocument/didClose`    |
//! | [`FossilPlayground::check`]     | `Array<{ uri, range, severity, message }>` | LSP `publishDiagnostics` (workspace-wide) |
//! | [`FossilPlayground::diagnostics_for`] | `Array<{ uri, range, severity, message }>` | per-file `publishDiagnostics` (07-03 drain) |
//! | [`FossilPlayground::set_target_shex`] | `()`                              | Schema panel install |
//!
//! The Phase-1 `compile(&str)` + `classification()` methods are RETAINED
//! verbatim — the Phase-1 smoke test and the STDL-07 classification path
//! both keep working without change.
//!
//! ## Architecture (ADR-0024)
//!
//! `fossil-wasm` is the LSP server-side IN THE BROWSER — NOT a recompiled
//! `fossil-lsp` (which has a `compile_error!` cfg-tripwire because
//! `lsp-server` uses crossbeam + stdio). Both `fossil-lsp` (native, stdio)
//! and `fossil-wasm` (WASM, postMessage) are thin transport adapters over
//! the same `fossil-ide` free functions — "one crate, two hosts"
//! (PROJECT.md EXT-01).
//!
//! ## Why the Workspace lifecycle is fan-out-safe
//!
//! `update_file` mutates the SAME [`fossil_base::SourceFile`] input via the
//! Salsa [`salsa::Setter`] (`set_text`) — the EXACT mechanism Phase 6's LSP
//! `didChange` path uses (ADR-0022 — the revision bump is the cancellation
//! trigger). NO new tracked queries land in the lifecycle path, so
//! `MAX_PER_MAPPING_FAN_OUT` stays at 1 (verified by
//! `fossil-hir::tests::invalidation_regression`, 3/3). The descriptor
//! storage on [`WasmDb`] mirrors `LspDb` byte-for-byte (06-09):
//! `Arc<OutputDescriptorKind>`, read once per query through the
//! [`fossil_hir::HirDb`] accessor, never interned, never a Salsa key
//! (ADR-0020).
//!
//! See `decisions/rudof-wasm.md` for the Phase 0 spike that validated the
//! WASM-first architecture, and RESEARCH.md §"WASM API scope" / Example 17
//! for the verbatim API contract this file implements.

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

use fossil_base::{Diagnostic, Files, Severity, SourceFile, Span, System};
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_hir::HirDb;
use fossil_ide::{LineIndex, Utf16Position};
use wasm_bindgen::prelude::*;

use crate::wasm_system::WasmSystem;
pub use crate::workspace::FileHandle;
use crate::workspace::OpenFiles;

/// The Salsa database the WASM host owns.
///
/// Mirrors `fossil-lsp::LspDb` byte-for-byte (06-09): a fresh `#[salsa::db]`
/// struct (cannot be `fossil_base::FossilDb` because the
/// `impl HirDb for FossilDb` is `#[cfg(test)]`-only — production hosts must
/// own their own db). Carries the Salsa runtime + the host [`System`] (here
/// [`WasmSystem`]) + an `Arc<OutputDescriptorKind>` it returns from the
/// [`HirDb`] override. Lets `fossil-ide` features (hover, completion,
/// goto-def) read the host's output descriptor so the target-side `ShEx`
/// type/properties are reachable once the user installs a schema via
/// [`FossilPlayground::set_target_shex`].
#[salsa::db]
#[derive(Clone)]
struct WasmDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    /// The user-installed output descriptor. Starts at the degraded
    /// `AcceptAll` default; replaced atomically on each
    /// [`FossilPlayground::set_target_shex`] call. Stored behind `Arc` so
    /// `Clone` of `WasmDb` (Salsa's `Snapshot` mechanism) shares the
    /// schema by reference, not by deep clone.
    descriptor: Arc<OutputDescriptorKind>,
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
}

impl HirDb for WasmDb {
    fn output_descriptor_kind(&self) -> &OutputDescriptorKind {
        &self.descriptor
    }
}

impl WasmDb {
    fn new(system: Arc<dyn System>) -> Self {
        Self {
            storage: salsa::Storage::default(),
            system,
            files: Files::default(),
            descriptor: Arc::new(OutputDescriptorKind::ACCEPT_ALL_DEFAULT),
        }
    }

    /// Replace the host output descriptor (e.g. after the user pastes a
    /// `ShEx` schema in the Schema panel). Re-`Arc`s a new descriptor;
    /// cheap and rare (per-install).
    fn set_descriptor(&mut self, kind: OutputDescriptorKind) {
        self.descriptor = Arc::new(kind);
    }
}

/// JS-facing handle for the Fossil compiler running inside a WASM module.
///
/// One instance owns one [`WasmDb`] (Salsa store + injected [`WasmSystem`]
/// + `Arc<OutputDescriptorKind>`). Hosts construct a single playground per
/// browser tab / Node process and reuse it across all method calls to
/// amortise the Salsa interning + memoisation overhead.
#[wasm_bindgen]
pub struct FossilPlayground {
    db: WasmDb,
    /// The system handle is owned by `db` via `Arc<dyn System>`; we retain a
    /// typed `Arc<WasmSystem>` here so future host wiring (e.g. an in-memory
    /// `VirtualFS` import in plan 07-10) can call `WasmSystem::write`
    /// without round-tripping through the trait object.
    #[allow(dead_code)]
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
    /// Installs `console_error_panic_hook` (idempotent — RESEARCH.md
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

    /// Return the stdlib classification manifest as a JS array of
    /// `{ name, wasm_class }` objects (STDL-07).
    ///
    /// `wasm_class` is the string `"pure_sql"` or `"native_udf_only"`. The
    /// playground reads this once at startup to render `native_udf_only`
    /// functions as disabled with a "native-only — unavailable in the browser"
    /// tooltip (SC#1 playground half). `DuckDB`-WASM cannot register the Rust
    /// UDFs those functions need (Pitfall 3), so the classification is the
    /// authority on what is runnable in-browser.
    ///
    /// This is pure read-only data projected from the `&'static`-ready
    /// `fossil_hir::stdlib::FunctionRegistry` — no `DuckDB`, no native UDF code,
    /// WASM-clean.
    ///
    /// # Errors
    ///
    /// Returns a JS error only if the manifest fails to serialize to `JsValue`.
    pub fn classification(&self) -> Result<JsValue, JsError> {
        let manifest = stdlib_classification();
        serde_wasm_bindgen::to_value(&manifest).map_err(JsError::from)
    }

    // ----- Phase 7 Workspace lifecycle (ty_wasm pattern — WASM-02) -----

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
    /// real cancellation trigger (ADR-0022): any in-flight analysis from the
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

    /// Run `parse → def_map → typecheck_mapping` across every open file and
    /// return a flat JS array of `{ uri, range, severity, message }` rows
    /// keyed by file URI. The LSP Worker (07-03) republishes these grouped
    /// by URI as `textDocument/publishDiagnostics` notifications.
    ///
    /// `range` is the UTF-16 LSP range (via `fossil_ide::LineIndex` — the
    /// rust-analyzer model from 06-05). `severity` is the LSP integer
    /// constant (1 = error, 2 = warning, 3 = info). `message` carries any
    /// `suggestion_source` as a `\nhelp: ...` suffix (mirroring `fossil-lsp`
    /// `to_lsp_diagnostic` in 06-09).
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

    /// Per-file diagnostic drain — the B3 follow-up accessor the LSP Worker
    /// (07-03) consumes for its per-file `publishDiagnostics` notifications.
    /// `check()` returns the workspace-wide flat array; `diagnostics_for`
    /// returns just one file's rows so 07-03 can dispatch one notification
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

    /// Install a user-supplied `ShEx` schema as the active output descriptor.
    /// On parse failure the previously-installed descriptor is RETAINED (no
    /// half-applied state — a broken schema must never wedge the editor;
    /// same contract as `fossil-lsp`'s `load_sibling_shex` in 06-09).
    ///
    /// The schema is parsed via
    /// [`fossil_descriptors_output::ShExDescriptor::from_reader`] (no
    /// network access — Pitfall 1) and wrapped in
    /// [`OutputDescriptorKind::ShEx`]. The resulting `Arc` is swapped into
    /// the [`WasmDb`]'s descriptor slot atomically; future `fossil-ide`
    /// feature calls (hover, completion) reading
    /// [`HirDb::output_descriptor_kind`] see the new schema.
    ///
    /// # Errors
    ///
    /// Returns a JS error if the text is not a parseable `ShEx` schema.
    pub fn set_target_shex(&mut self, text: &str) -> Result<(), JsError> {
        self.set_target_shex_native(text)
            .map_err(|e| JsError::new(&e))
    }

    // ----- Phase 13 (ADR-0037) — register a host-introspected descriptor -----

    /// Register an [`fossil_descriptors_input::InferredDescriptor`] for a
    /// source binding name BEFORE invoking [`Self::compile`] /
    /// [`Self::compile_file`]. The Rust compiler reads from this registration
    /// during forward type propagation (Phase 3 CORE-05 rewired in plan
    /// 13-02).
    ///
    /// `descriptor_json` is the JSON serialisation of `InferredDescriptor`;
    /// the canonical shape is exposed in `packages/wasm/src/index.ts` as
    /// `InferredDescriptorJson`:
    ///
    /// ```json
    /// {
    ///   "source_name": "users",
    ///   "columns": [
    ///     { "name": "id", "primitive": "integer" },
    ///     { "name": "name", "primitive": "string" }
    ///   ],
    ///   "content_hash": ""
    /// }
    /// ```
    ///
    /// Called by the browser-side playground orchestration AFTER running
    /// DuckDB-WASM `DESCRIBE read_csv_auto('<resolved-url>')` and BEFORE
    /// invoking `compile()` / `compile_file()`. Keyed by source-binding
    /// name (`"users"` for `users := io.csv("...")`), NOT by URL.
    ///
    /// Idempotent: re-registering with the same `source_name` OVERWRITES the
    /// previous entry — intentional, since the host may re-introspect when
    /// file content changes.
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
// `stdlib_classification()` split that has been the pattern since Phase 5.

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
    /// underlying `serde_json` error message. Phase 13 (ADR-0037).
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
                all.push(to_check_row(&uri, &index, &d));
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
                .map(|d| to_check_row(&uri, &index, &d))
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
    pub fn open_file_native(&mut self, path: String, contents: String) -> FileHandle {
        let file = fossil_base::SourceFile::new(&self.db, contents, path.clone());
        self.files.insert(path, file)
    }

    // ----- LSP-worker dispatch helpers (07-03 — pub(crate)) -----
    //
    // These accessors are consumed by `lsp_worker::dispatch` to route LSP
    // requests / notifications onto the existing fossil-ide free functions
    // without leaking the `WasmDb` type or duplicating bookkeeping. They
    // mirror the analogous `LspState` accessors in `fossil-lsp` 06-09.

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

    /// `HirDb` accessor for hover / completion (they take `&dyn HirDb` for
    /// the target-aware descriptor read — ADR-0020).
    pub(crate) fn hir_db(&self) -> &dyn HirDb {
        &self.db
    }

    /// Base `fossil_base::Db` accessor for goto-def / document-symbol /
    /// semantic-tokens / code-action (they take `&dyn fossil_base::Db`).
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
    /// drops — mirrors fossil-lsp's `diagnostics_for` in 06-09.
    pub(crate) fn drain_diagnostics_for_file(&self, file: SourceFile) -> Vec<Diagnostic> {
        diagnostics_for_file(&self.db, file)
    }

    /// Native-reachable `ShEx` install — pure-Rust mirror of
    /// `set_target_shex`.
    ///
    /// # Errors
    ///
    /// Returns the lowering error string when the schema does not parse;
    /// the previously-installed descriptor is RETAINED (no half-applied
    /// state).
    pub fn set_target_shex_native(&mut self, text: &str) -> Result<(), String> {
        match ShExDescriptor::from_reader(text.as_bytes()) {
            Ok(d) => {
                self.db.set_descriptor(OutputDescriptorKind::ShEx(d));
                Ok(())
            }
            Err(e) => Err(format!("ShEx parse error: {e:?}")),
        }
    }

    // ----- Phase 13 (ADR-0037) — inferred-descriptor registration -----

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
        self.system.register_inferred_descriptor(descriptor);
        Ok(())
    }

    /// Native-reachable lookup mirroring `System::inferred_descriptor`.
    /// Lets cargo-tests verify the registration round-trips without going
    /// through the wasm-bindgen wrapper.
    #[must_use]
    pub fn inferred_descriptor_native(
        &self,
        source_name: &str,
    ) -> Option<fossil_descriptors_input::InferredDescriptor> {
        self.system.inferred_descriptor(source_name)
    }
}

/// One stdlib function's WASM classification (STDL-07). Serialized to a JS
/// object `{ name, wasm_class }` for the playground.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FnClassification {
    /// Fully-qualified dotted name (e.g. `"clean.slug"`, `"clean.trim"`).
    pub name: String,
    /// `"pure_sql"` (runs in the browser) or `"native_udf_only"` (disabled).
    pub wasm_class: String,
}

/// Project the full stdlib registry into the serializable classification
/// manifest: every function name + its `wasm_class` string.
///
/// Read directly from `fossil_hir::stdlib::FunctionRegistry::stdlib_default()`,
/// the single source of truth (SC#1 — the playground and the native UDFs agree
/// on which functions are `native_udf_only`).
#[must_use]
pub fn stdlib_classification() -> Vec<FnClassification> {
    use fossil_hir::stdlib::WasmClass;

    let registry = fossil_hir::stdlib::FunctionRegistry::stdlib_default();
    let mut manifest: Vec<FnClassification> = registry
        .iter()
        .map(|entry| FnClassification {
            name: entry.name.to_string(),
            wasm_class: match entry.wasm_class {
                WasmClass::PureSql => "pure_sql".to_string(),
                WasmClass::NativeUdfOnly => "native_udf_only".to_string(),
            },
        })
        .collect();
    // Stable order so the manifest (and any consumer snapshot) is deterministic;
    // the registry iterates a HashMap (unspecified order).
    manifest.sort_by(|a, b| a.name.cmp(&b.name));
    manifest
}

impl Default for FossilPlayground {
    fn default() -> Self {
        Self::new()
    }
}

// ----- Parse-only lineage + providers (one crate, two hosts — ADR-0024) -----
//
// `refs` and `providers` mirror the native `fossil refs` / `fossil providers`
// CLI commands for the BROWSER host: keasy's client-compute job runner reads a
// program's typed lineage (which `@conn`s + `schema =` it references) and the
// supported source providers WITHOUT subprocessing the `fossil` binary. Both
// delegate to the shared, WASM-clean `fossil_registry` implementation — the SAME
// code `fossil-engine` runs natively — so the browser and the CLI can never
// diverge ([[feedback_no_duplicate_logic_across_crates]]).
//
// Free functions (not `FossilPlayground` methods): they are stateless and
// program-text-driven (the job runner has the script string, not the editor's
// open-file workspace), mirroring the existing free `tokenize` export.

/// The data-source providers fossil supports (`io.csv`, `io.rdf`, …) as a JS
/// array of `{ name, extensions, kind }`. Pure projection of the stdlib source
/// registry — no parsing, no db.
///
/// # Errors
/// Returns a JS error only if the result fails to serialize to `JsValue`.
#[wasm_bindgen]
pub fn providers() -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(&fossil_lineage::providers()).map_err(JsError::from)
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
/// Mirrors the LSP `Diagnostic` shape exactly so the LSP Worker (07-03) can
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

/// Drain the Salsa `Diagnostic` accumulator across every mapping in `file`
/// (the same pattern as `fossil-lsp::diagnostics_for` in 06-09). Forces
/// `def_map` + `typecheck_mapping` for each mapping so the accumulator is
/// populated before we read it.
fn diagnostics_for_file(db: &WasmDb, file: SourceFile) -> Vec<Diagnostic> {
    let dm = fossil_hir::def_map::def_map(db, file);
    let mut out = Vec::new();
    for mapping in dm.mappings(db) {
        // Drain from the LOWERING, not the typechecker: Salsa accumulators are
        // transitive and lowering calls the typechecker, so this yields both
        // sets without duplicating either. Draining only the typechecker would
        // show the editor a clean file that `run` then refuses.
        let _ = fossil_mir::lower_to_mir_pg(db, *mapping);
        let diags = fossil_mir::lower_to_mir_pg::accumulated::<Diagnostic>(db, *mapping);
        // Spans are mapping-relative; the editor renders against the file.
        out.extend(fossil_hir::spans::rebase_to_file(
            db,
            *mapping,
            diags.into_iter().cloned(),
        ));
    }
    out
}

/// Convert one `fossil_base::Diagnostic` to the JS-side row shape. UTF-16
/// range conversion via [`fossil_ide::LineIndex`]; `suggestion_source` folded
/// into the message as a `help:` suffix (mirroring `fossil-lsp::
/// to_lsp_diagnostic` in 06-09 — keep the structured carriers reachable by
/// re-draining the accumulator on the consumer side).
fn to_check_row(uri: &str, index: &LineIndex, d: &Diagnostic) -> CheckRow {
    let range = span_to_range(index, d.span);
    let message = d.suggestion_source.as_ref().map_or_else(
        || d.message.clone(),
        |s| format!("{}\nhelp: {s}", d.message),
    );
    CheckRow {
        uri: uri.to_string(),
        range,
        severity: severity_to_lsp_int(d.severity),
        message,
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

// ----- LSP dispatch test hook (07-03 Task 2) -----
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
