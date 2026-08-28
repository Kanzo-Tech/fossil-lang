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
use serde::Serialize;
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
    // **This binding grants raw SQL, and it is one call that says so.**
    //
    // `Operation` has no `Deserialize`: `from_wire`'s second argument is the
    // permission that `execute_sql`'s `sql` and `read`'s `where` share, so a
    // binding cannot open one and close the other (`fossil_graph::raw_sql`).
    // The browser grants it because there is nothing here to withhold it from:
    // the engine is DuckDB-WASM inside the caller's own tab, over a corpus the
    // caller already holds, and `/docs/format/reading/without-fossil`
    // documents opening that corpus with any Parquet reader. A gate here would
    // guard a door in a field. The binding with a policy worth having is
    // `fossil-mcp`, where the operator holds the files and the caller does not.
    let op: serde_json::Value = serde_wasm_bindgen::from_value(op).map_err(JsError::from)?;
    let op = Operation::from_wire(&op, Some(fossil_graph::RawSqlAccess::granted()))
        .map_err(|e| to_js_error(&e))?;
    let files: HashMap<String, String> =
        serde_wasm_bindgen::from_value(manifest_files).map_err(JsError::from)?;
    let manifest = Manifest::load(&MapSource(files)).map_err(|e| to_js_error(&e))?;
    let exec = JsExecutor { query };
    let result = fossil_graph::dispatch(&op, &manifest, &exec)
        .await
        .map_err(|e| to_js_error(&e))?;
    // `json_compatible()` serialises structs/maps as plain JS objects (not the
    // default `Map`), so the TS side reads `result.types` etc. matching the
    // codegen'd interfaces. Large ints surface as JS numbers (counts fit).
    result
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(JsError::from)
}

// ── the addressing, which opens no byte ───────────────────────────────────────
//
// [`dispatch_graph`] above is the VERB surface, and it needs a `query` callback
// because every verb ends in SQL. This is the other half, and it needs nothing: a
// tile is a fixed range of `dense_id`, its address is a shift, and every URL a
// reader wants is a function of the manifest bytes the host already holds. No
// engine appears anywhere below, which is why a host swapping the one behind
// `query` changes nothing here.
//
// It is exposed here rather than only in `@fossil-lang/corpus` because this is the
// leg that makes the conformance diff three-sided.
// `apps/corpus/conformance/expected.json` is executed by plain Node
// (`conformance/verify.mjs`), by the published TypeScript
// (`packages/corpus/tests/conformance.test.ts`) and by [`fossil_graph::address`] —
// natively in `crates/fossil-graph/tests/conformance.rs` and, through this
// binding, as the wasm32 build that actually ships. The native run and the wasm
// run are not the same claim: `usize` is 64 bits there and 32 here, and 2^53 is
// where a port that went through a double stops being exact.

use fossil_graph::address::{Direction, ResolvedCorpus, resolve};

/// A corpus resolved to addresses — synchronous, and it opens no byte.
///
/// The counterpart of `resolveCorpus` in `@fossil-lang/corpus/address`, and the
/// same arithmetic the native reader runs. A host holds one of these for as long
/// as it holds the manifest.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Corpus {
    inner: ResolvedCorpus,
}

// `wasm_bindgen` owns every argument that crosses the boundary — `Option<&String>`
// and `&[String]` are not shapes it can generate glue for — so pedantic's
// `needless_pass_by_value` fires on parameters that cannot be references.
#[allow(clippy::needless_pass_by_value)]
#[wasm_bindgen]
impl Corpus {
    /// Resolve `{ rel_path: yaml }` — the manifest set the host pre-fetched —
    /// against `base`, which is prepended to every URL and nothing else happens
    /// to it.
    ///
    /// # Errors
    ///
    /// A `JsError` when the manifest cannot address itself: a missing file, a
    /// `chunk_size` no shift addresses, an endpoint type the index does not
    /// declare, or an edge whose declared tile size disagrees with the vertex
    /// type that addresses it. **Not** for an orientation the corpus does not
    /// publish — that is a legitimate corpus, reported by
    /// [`Corpus::adjacency_tile_url`] as an address that does not exist rather
    /// than one that 404s.
    #[wasm_bindgen(constructor)]
    pub fn new(manifest_files: JsValue, base: Option<String>) -> Result<Self, JsError> {
        let files: std::collections::BTreeMap<String, String> =
            serde_wasm_bindgen::from_value(manifest_files).map_err(JsError::from)?;
        let inner =
            resolve(&files, base.as_deref().unwrap_or_default()).map_err(|e| to_js_error(&e))?;
        Ok(Self { inner })
    }

    /// The whole resolution as plain JS data: the container, every vertex type
    /// with its prefix, tile size, shift, count and index, and every edge type
    /// with the orientations it publishes.
    ///
    /// # Errors
    ///
    /// A `JsError` if the resolution cannot be serialised.
    pub fn snapshot(&self) -> Result<JsValue, JsError> {
        self.inner
            .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
            .map_err(JsError::from)
    }

    /// The tile a `dense_id` lives in — the whole of the addressing scheme.
    ///
    /// **A decimal string in and out.** A `dense_id` may carry more than 53 bits,
    /// and JavaScript's `>>` truncates to 32 *before* it shifts, so the same three
    /// characters mean something different on each side of this boundary. The
    /// published border vectors (`apps/corpus/guards/vectors.json`) are 2^31,
    /// where a port that took the shift as signed gives a negative tile, and 2^53,
    /// where one that went through a `Number` stops being exact.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest, or `dense_id` is not a
    /// decimal integer.
    #[wasm_bindgen(js_name = tileOf)]
    pub fn tile_of(&self, vertex_type: Option<String>, dense_id: &str) -> Result<String, JsError> {
        let vertex = self
            .inner
            .vertex_type(vertex_type.as_deref())
            .map_err(|e| to_js_error(&e))?;
        let id: u64 = dense_id
            .parse()
            .map_err(|_| JsError::new(&format!("dense_id {dense_id} is not a decimal integer")))?;
        Ok(vertex.tile_of(id).to_string())
    }

    /// The file tile `k` of a vertex type is in.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest.
    #[wasm_bindgen(js_name = vertexTileUrl)]
    pub fn vertex_tile_url(
        &self,
        vertex_type: Option<String>,
        tile: u32,
    ) -> Result<String, JsError> {
        self.inner
            .vertex_type(vertex_type.as_deref())
            .map(|v| v.tile_url(u64::from(tile)))
            .map_err(|e| to_js_error(&e))
    }

    /// The file tile `k` of one orientation of one edge type is in, or `null`
    /// when the corpus does not publish that orientation.
    ///
    /// `null` rather than a string is the point: an orientation the manifest does
    /// not declare has no address, and handing back a URL that 404s is the failure
    /// this whole seam exists to prevent.
    ///
    /// # Errors
    ///
    /// A `JsError` when the edge type is not in the manifest, or `direction` is
    /// neither `src` nor `dst`.
    #[wasm_bindgen(js_name = adjacencyTileUrl)]
    pub fn adjacency_tile_url(
        &self,
        edge_type: &str,
        direction: &str,
        tile: u32,
    ) -> Result<Option<String>, JsError> {
        let direction = Direction::parse(direction).ok_or_else(|| {
            JsError::new(&format!(
                "{direction} is not an orientation; it is src or dst"
            ))
        })?;
        let edge = self
            .inner
            .edges
            .iter()
            .find(|e| e.edge_type == edge_type)
            .ok_or_else(|| JsError::new(&format!("no edge type {edge_type} in the manifest")))?;
        Ok(edge
            .adjacency(direction)
            .map(|a| a.tile_url(u64::from(tile))))
    }

    /// The URLs a set of vertex tiles addresses, and what that set is complete
    /// for. `directions` of `["src"]` is the drawing read; both orientations is
    /// the incident set.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest, or a direction is neither
    /// `src` nor `dst`.
    pub fn window(
        &self,
        vertex_type: Option<String>,
        tiles: Vec<u32>,
        directions: Vec<String>,
    ) -> Result<JsValue, JsError> {
        let tiles: Vec<u64> = tiles.into_iter().map(u64::from).collect();
        let directions: Vec<Direction> = directions
            .iter()
            .map(|d| {
                Direction::parse(d).ok_or_else(|| {
                    JsError::new(&format!("{d} is not an orientation; it is src or dst"))
                })
            })
            .collect::<Result<_, _>>()?;
        self.inner
            .window(vertex_type.as_deref(), &tiles, &directions)
            .map_err(|e| to_js_error(&e))?
            .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
            .map_err(JsError::from)
    }
}
