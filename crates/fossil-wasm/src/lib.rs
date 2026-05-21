//! `fossil-wasm` — WASM host shim exposing [`FossilPlayground`] to JS.
//!
//! Phase 1 surface (this commit): a single [`FossilPlayground::compile`]
//! method that runs the full compile pipeline (parse → `def_map` →
//! `lower_to_hir` → `lower_to_mir` → `codegen_sql`) on a source string and
//! returns `{ sql, manifest_yaml }` as a JS object via `serde-wasm-bindgen`.
//!
//! Phase 7 PLAY-01 expands the surface to the full `open_file` /
//! `update_file` / `close_file` lifecycle plus a `check()` method.
//! Phase 7 PLAY-02 wires DuckDB-WASM execution in the browser.
//!
//! See `decisions/rudof-wasm.md` for the Phase 0 spike that validated the
//! WASM-first architecture, and RESEARCH.md §"WASM API scope" / Example 17
//! for the verbatim API contract this file implements.

mod wasm_system;

use std::sync::Arc;

use wasm_bindgen::prelude::*;

use crate::wasm_system::WasmSystem;

/// JS-facing handle for the Fossil compiler running inside a WASM module.
///
/// One instance owns one [`fossil_base::FossilDb`] (Salsa store + injected
/// [`fossil_base::System`]). Hosts construct a single playground per browser
/// tab / Node process and reuse it across [`FossilPlayground::compile`]
/// invocations to amortise the Salsa interning + memoisation overhead.
#[wasm_bindgen]
#[derive(Debug)]
pub struct FossilPlayground {
    db: fossil_base::FossilDb,
    // The system handle is owned by `db` via `Arc<dyn System>`; we retain a
    // typed `Arc<WasmSystem>` here so Phase 7 PLAY-01's `open_file` /
    // `update_file` lifecycle can call `WasmSystem::write` without
    // round-tripping through the trait object.
    #[allow(dead_code)]
    system: Arc<WasmSystem>,
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
        let db = fossil_base::FossilDb::new(system.clone() as Arc<dyn fossil_base::System>);
        Self { db, system }
    }

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
/// `String` fields. Phase 7 PLAY-01 may extend this with diagnostic arrays
/// once `check()` lands; keeping the shape additive preserves the JS-side
/// contract.
#[derive(serde::Serialize)]
struct CompileResult {
    sql: String,
    manifest_yaml: String,
}
