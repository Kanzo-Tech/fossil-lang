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
//! | [`FossilPlayground::compile_file`] | `{ sql, manifest_yaml }`           | run-button → DuckDB-WASM (07-04) |
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

mod wasm_system;
mod workspace;

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

    // ----- Phase 1 legacy surface (retained verbatim) -----

    /// Compile a Fossil source string. On success returns
    /// `{ sql: String, manifest_yaml: String }` as a JS object via
    /// `serde-wasm-bindgen`. On error throws a JS `Error` carrying the
    /// diagnostic message.
    ///
    /// Phase 1 wires the same pipeline as `fossil-cli`'s `compile`
    /// subcommand minus the native `DuckDB` execution step (`fossil-runtime`
    /// is intentionally NOT a dependency of this crate per RESEARCH.md
    /// Pattern 5). Phase 7 PLAY-02 adds in-browser execution via
    /// `DuckDB`-WASM.
    ///
    /// Prefer [`Self::compile_file`] for the lifecycle path (it consumes an
    /// open `FileHandle` instead of a freshly-interned ad-hoc `SourceFile`).
    pub fn compile(&self, source: &str) -> Result<JsValue, JsError> {
        let file = fossil_base::SourceFile::new(
            &self.db,
            source.to_string(),
            "playground.fossil".to_string(),
        );

        let dm = fossil_hir::def_map::def_map(&self.db, file);
        let mappings = dm.mappings(&self.db);
        let mapping = mappings
            .first()
            .copied()
            .ok_or_else(|| JsError::new("no mapping found in source"))?;

        let plan = fossil_codegen::codegen_sql(&self.db, mapping);
        let result = CompileResult {
            sql: plan.sql(&self.db).clone(),
            manifest_yaml: plan.manifest_yaml(&self.db).clone(),
        };
        serde_wasm_bindgen::to_value(&result).map_err(JsError::from)
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
    /// `fossil_registry::FunctionRegistry` — no `DuckDB`, no native UDF code,
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
        let file = fossil_base::SourceFile::new(&self.db, contents, path.clone());
        Ok(self.files.insert(path, file))
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
        use salsa::Setter as _;
        let file = self
            .files
            .get(handle)
            .ok_or_else(|| JsError::new("unknown file handle"))?;
        file.set_text(&mut self.db).to(contents);
        Ok(())
    }

    /// Close a file in the workspace. Idempotent in spirit but strict in
    /// signal: closing an unknown / already-closed handle is an error so
    /// JS-side bugs surface loudly (mirrors `ty_wasm`).
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` was never opened or was already closed.
    pub fn close_file(&mut self, handle: FileHandle) -> Result<(), JsError> {
        self.files
            .remove(handle)
            .ok_or_else(|| JsError::new("unknown file handle"))?;
        Ok(())
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
        let mut all: Vec<JsCheckDiagnostic> = Vec::new();
        for (h, file) in self.files.iter() {
            let uri = self.files.path_for(h, &self.db).unwrap_or_default();
            let index = fossil_ide::line_index(&self.db, file);
            for d in diagnostics_for_file(&self.db, file) {
                all.push(to_js_diagnostic(&uri, &index, &d));
            }
        }
        serde_wasm_bindgen::to_value(&all).map_err(JsError::from)
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
        let file = self
            .files
            .get(handle)
            .ok_or_else(|| JsError::new("unknown file handle"))?;
        let uri = self.files.path_for(handle, &self.db).unwrap_or_default();
        let index = fossil_ide::line_index(&self.db, file);
        let rows: Vec<JsCheckDiagnostic> = diagnostics_for_file(&self.db, file)
            .into_iter()
            .map(|d| to_js_diagnostic(&uri, &index, &d))
            .collect();
        serde_wasm_bindgen::to_value(&rows).map_err(JsError::from)
    }

    /// Compile one open file. Returns `{ sql, manifest_yaml }` — the same
    /// shape [`Self::compile`] returns. Preferred over `compile(&str)` for
    /// the playground run path because it consumes the file's stable Salsa
    /// `SourceFile` identity (so subsequent edits benefit from incremental
    /// memoisation).
    ///
    /// # Errors
    ///
    /// Returns a JS error if `handle` is unknown, the file has no mapping,
    /// or serialization fails.
    pub fn compile_file(&self, handle: FileHandle) -> Result<JsValue, JsError> {
        let file = self
            .files
            .get(handle)
            .ok_or_else(|| JsError::new("unknown file handle"))?;
        let dm = fossil_hir::def_map::def_map(&self.db, file);
        let mapping = dm
            .mappings(&self.db)
            .first()
            .copied()
            .ok_or_else(|| JsError::new("no mapping found in file"))?;
        let plan = fossil_codegen::codegen_sql(&self.db, mapping);
        let result = CompileResult {
            sql: plan.sql(&self.db).clone(),
            manifest_yaml: plan.manifest_yaml(&self.db).clone(),
        };
        serde_wasm_bindgen::to_value(&result).map_err(JsError::from)
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
        match ShExDescriptor::from_reader(text.as_bytes()) {
            Ok(d) => {
                self.db.set_descriptor(OutputDescriptorKind::ShEx(d));
                Ok(())
            }
            Err(e) => Err(JsError::new(&format!("ShEx parse error: {e:?}"))),
        }
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
/// Read directly from `fossil_registry::FunctionRegistry::stdlib_default()`,
/// the single source of truth (SC#1 — the playground and the native UDFs agree
/// on which functions are `native_udf_only`).
#[must_use]
pub fn stdlib_classification() -> Vec<FnClassification> {
    use fossil_registry::WasmClass;

    let registry = fossil_registry::FunctionRegistry::stdlib_default();
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

/// Phase 1 [`FossilPlayground::compile`] return shape — flat object with two
/// `String` fields. Phase 7 `compile_file` reuses the same shape (additive
/// to the JS-side contract).
#[derive(serde::Serialize)]
struct CompileResult {
    sql: String,
    manifest_yaml: String,
}

/// One diagnostic row in the [`FossilPlayground::check`] return array.
///
/// Mirrors the LSP `Diagnostic` shape exactly so the LSP Worker (07-03) can
/// republish each row as-is inside a `PublishDiagnosticsParams` payload
/// without a second translation step. UTF-16 ranges; integer LSP severities.
#[derive(serde::Serialize)]
struct JsCheckDiagnostic {
    uri: String,
    range: JsRange,
    severity: u8,
    message: String,
}

/// LSP-shaped range (zero-based line + UTF-16 column).
#[derive(serde::Serialize)]
struct JsRange {
    start: JsPosition,
    end: JsPosition,
}

#[derive(serde::Serialize)]
struct JsPosition {
    line: u32,
    character: u32,
}

/// Drain the Salsa `Diagnostic` accumulator across every mapping in `file`
/// (the same pattern as `fossil-lsp::diagnostics_for` in 06-09). Forces
/// `def_map` + `typecheck_mapping` for each mapping so the accumulator is
/// populated before we read it.
fn diagnostics_for_file(db: &WasmDb, file: SourceFile) -> Vec<Diagnostic> {
    let dm = fossil_hir::def_map::def_map(db, file);
    let mut out = Vec::new();
    for mapping in dm.mappings(db) {
        let _ = fossil_hir::typecheck_mapping(db, *mapping);
        let diags = fossil_hir::typecheck_mapping::accumulated::<Diagnostic>(db, *mapping);
        out.extend(diags.into_iter().cloned());
    }
    out
}

/// Convert one `fossil_base::Diagnostic` to the JS-side row shape. UTF-16
/// range conversion via [`fossil_ide::LineIndex`]; `suggestion_source` folded
/// into the message as a `help:` suffix (mirroring `fossil-lsp::
/// to_lsp_diagnostic` in 06-09 — keep the structured carriers reachable by
/// re-draining the accumulator on the consumer side).
fn to_js_diagnostic(uri: &str, index: &LineIndex, d: &Diagnostic) -> JsCheckDiagnostic {
    let range = span_to_js_range(index, d.span);
    let message = d.suggestion_source.as_ref().map_or_else(
        || d.message.clone(),
        |s| format!("{}\nhelp: {s}", d.message),
    );
    JsCheckDiagnostic {
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
fn span_to_js_range(index: &LineIndex, span: Span) -> JsRange {
    JsRange {
        start: utf16_to_js(fossil_ide::offset_to_lsp_position(index, span.start)),
        end: utf16_to_js(fossil_ide::offset_to_lsp_position(index, span.end)),
    }
}

const fn utf16_to_js(p: Utf16Position) -> JsPosition {
    JsPosition {
        line: p.line,
        character: p.character,
    }
}
