//! wasm-bindgen binding for the fossil-graph verb surface.
//!
//! The verb→SQL logic and dispatch live in `fossil-graph` (WASM-clean, the same
//! code the native runtime drives). This crate adds only the browser glue:
//!
//! - [`dispatch_graph`] — the `#[wasm_bindgen]` entry point. The host (keasy)
//!   passes the [`Operation`] JSON, the (small, pre-fetched) manifest YAMLs, and
//!   a `query` callback that runs SQL on its DuckDB-WASM connection and returns a
//!   `Promise<rows>`. The verb logic runs in WASM and `await`s that callback.
//! - The manifest stays sync: the host pre-fetches the handful of YAML files and
//!   passes them by value, so the (httpfs/OPFS) async fetch happens in JS, not
//!   across the `ManifestSource` seam.
//!
//! The dispatch future is single-threaded (DuckDB-WASM lives on one browser
//! thread), so `future_not_send` is allowed here.
#![allow(clippy::future_not_send)]

use std::collections::HashMap;

use fossil_graph::manifest::{Manifest, ManifestSource};
use fossil_graph::{DuckExecutor, GraphError, Operation};
use serde_json::Value;
use wasm_bindgen::prelude::*;

/// In-memory manifest source: the host pre-fetches `{ rel_path: yaml }` and hands
/// it over, keeping [`ManifestSource`] synchronous in WASM.
struct MapSource(HashMap<String, String>);

impl ManifestSource for MapSource {
    fn fetch(&self, rel_path: &str) -> fossil_graph::Result<Vec<u8>> {
        self.0
            .get(rel_path)
            .map(|yaml| yaml.clone().into_bytes())
            .ok_or_else(|| GraphError::InvalidManifest(format!("missing {rel_path}")))
    }
}

/// [`DuckExecutor`] backed by a JS async callback `(sql: string) => Promise<rows>`
/// — the host's DuckDB-WASM query (e.g. keasy's Mosaic coordinator). The rows
/// must be a JSON array of objects (column → value).
struct JsExecutor {
    query: js_sys::Function,
}

impl DuckExecutor for JsExecutor {
    async fn query_json(&self, sql: &str) -> fossil_graph::Result<Vec<Value>> {
        let promise = self
            .query
            .call1(&JsValue::NULL, &JsValue::from_str(sql))
            .map_err(|e| js_err(&e))?;
        let promise: js_sys::Promise = promise.dyn_into().map_err(|_| {
            GraphError::Execution("query callback did not return a Promise".to_string())
        })?;
        let rows = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(|e| js_err(&e))?;
        serde_wasm_bindgen::from_value(rows).map_err(|e| GraphError::Execution(e.to_string()))
    }
}

fn js_err(value: &JsValue) -> GraphError {
    GraphError::Execution(format!("{value:?}"))
}

fn to_js_error(error: &GraphError) -> JsError {
    JsError::new(&error.to_string())
}

/// Dispatch one graph verb in the browser.
///
/// - `op` — the [`Operation`] as `{ verb, params }` JSON.
/// - `manifest_files` — `{ rel_path: yaml_string }` for the `GraphAr` manifest
///   (the host pre-fetches these; they're small).
/// - `query` — `(sql: string) => Promise<rows>` running on the host's DuckDB-WASM.
///
/// Returns the verb's `Result` as a JS value. The `#[wasm_bindgen] async fn`
/// surfaces to JS as a `Promise`.
///
/// # Errors
///
/// Returns a `JsError` if `op`/`manifest_files` fail to deserialise, the manifest
/// is invalid, or the verb's execution (including the JS `query` callback) fails.
#[wasm_bindgen]
pub async fn dispatch_graph(
    op: JsValue,
    manifest_files: JsValue,
    query: js_sys::Function,
) -> Result<JsValue, JsError> {
    let op: Operation = serde_wasm_bindgen::from_value(op).map_err(JsError::from)?;
    let files: HashMap<String, String> =
        serde_wasm_bindgen::from_value(manifest_files).map_err(JsError::from)?;
    let manifest = Manifest::load(&MapSource(files)).map_err(|e| to_js_error(&e))?;
    let exec = JsExecutor { query };
    let result = fossil_graph::dispatch(&op, &manifest, &exec)
        .await
        .map_err(|e| to_js_error(&e))?;
    serde_wasm_bindgen::to_value(&result).map_err(JsError::from)
}
