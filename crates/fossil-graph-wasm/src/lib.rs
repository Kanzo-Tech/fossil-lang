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
// **This is the only reader.** `@fossil-lang/corpus` had a second one — 1,058
// lines of TypeScript composing the same URLs from the same manifest, agreeing
// with [`fossil_graph::plan`] because people kept making it agree — and it is
// gone. What is left below is the binding it reaches this one through, so
// `apps/corpus/conformance/expected.json` is now executed by exactly two
// implementations that mean it (plain Node in `conformance/verify.mjs`, and
// `fossil_graph::plan` natively in `crates/fossil-graph/tests/conformance.rs`)
// plus one that re-enters this one from JavaScript
// (`packages/corpus/tests/conformance.test.ts`, through the wasm32 build that
// actually ships). The native run and the wasm run are not the same claim:
// `usize` is 64 bits there and 32 here, and 2^53 is where a port that went
// through a double stops being exact.
//
// **Every number of `dense_id` width crosses as a `BigInt`**, in both
// directions. JavaScript's `>>` truncates to 32 bits *before* it shifts and its
// `Number` stops being exact at 2^53, so a `u64` that arrived as a double would
// have lost the two borders the published vectors
// (`apps/corpus/guards/vectors.json`) exist to pin.

use fossil_graph::plan::{
    Direction, EdgeAddress, ProjectionAddress, ReadPlan, VertexAddress, resolve,
};
use fossil_sinks::manifest::VertexLevels;

/// How many `dense_id`s one row of level `k` stands for — `4^k`.
///
/// The pyramid's base, and JS reads it here rather than respelling it: the
/// decimation level *k* means the vertices whose `dense_id` is a multiple of
/// this, so a `2^k` on the far side of the boundary is a different corpus.
#[must_use]
// `wasm_bindgen` cannot generate glue for a `const fn`, so this cannot be one.
#[allow(clippy::missing_const_for_fn)]
#[wasm_bindgen(js_name = strideOf)]
pub fn stride_of(level: u32) -> u64 {
    VertexLevels::stride(level)
}

/// How many bits of `dense_id` level `k` drops — `2k`, and the number a reader
/// ADDS to its payload tile shift to address a level tile.
#[must_use]
// `wasm_bindgen` cannot generate glue for a `const fn`, so this cannot be one.
#[allow(clippy::missing_const_for_fn)]
#[wasm_bindgen(js_name = strideBits)]
pub fn stride_bits(level: u32) -> u32 {
    VertexLevels::stride_bits(level)
}

/// How many rows level `k` of a type of `count` rows holds — `ceil(count / 4^k)`.
///
/// A level is a predicate, so this answers for every `k` and not only for the
/// ones a writer spent bytes on. [`ProjectionAddress::rows`] is the same number
/// for a level the manifest declares — it is a field there, computed once at
/// resolve time, and not a question asked again per call.
#[must_use]
// `wasm_bindgen` cannot generate glue for a `const fn`, so this cannot be one.
#[allow(clippy::missing_const_for_fn)]
#[wasm_bindgen(js_name = rowsAt)]
pub fn rows_at(count: u64, level: u32) -> u64 {
    VertexLevels::rows_at(count, level)
}

/// A corpus resolved into a [`ReadPlan`] — synchronous, and it opens no byte.
///
/// A host holds one of these for as long as it holds the manifest.
/// [`Corpus::snapshot`] is how most of it crosses: what a resolved corpus IS is
/// data, and data crosses once. The methods below are the part that is not — a
/// question answered per call, because its answer is a function of an argument
/// the manifest does not contain.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Corpus {
    inner: ReadPlan,
}

impl Corpus {
    /// One vertex type, or the first the index names. The error is
    /// [`ReadPlan::vertex_type`]'s, which names what the manifest does declare.
    fn vertex(&self, name: Option<&str>) -> Result<&VertexAddress, JsError> {
        self.inner.vertex_type(name).map_err(|e| to_js_error(&e))
    }

    /// One projection of a vertex type, or `None` when the manifest writes none
    /// at that scale — which is a cost and not a refusal, the predicate over the
    /// payload answering every scale a writer skipped.
    fn projection(
        &self,
        name: Option<&str>,
        scale: u64,
    ) -> Result<Option<&ProjectionAddress>, JsError> {
        Ok(self.vertex(name)?.projection(scale))
    }

    fn edge(&self, edge_type: &str) -> Result<&EdgeAddress, JsError> {
        self.inner
            .edges
            .iter()
            .find(|e| e.edge_type == edge_type)
            .ok_or_else(|| JsError::new(&format!("no edge type {edge_type} in the manifest")))
    }

    /// One projection of a relation, by scale and orientation. `None` for an
    /// orientation the corpus does not publish as well as for a scale it did not
    /// write: neither has an address, and both are legitimate corpora.
    fn edge_projection(
        &self,
        edge_type: &str,
        direction: &str,
        scale: u64,
    ) -> Result<Option<&ProjectionAddress>, JsError> {
        let direction = parse_direction(direction)?;
        Ok(self.edge(edge_type)?.projection(scale, direction))
    }
}

fn parse_direction(value: &str) -> Result<Direction, JsError> {
    Direction::parse(value)
        .ok_or_else(|| JsError::new(&format!("{value} is not an orientation; it is src or dst")))
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
    /// [`Corpus::edge_projection_tile_url`] as an address that does not exist
    /// rather than one that 404s.
    #[wasm_bindgen(constructor)]
    pub fn new(manifest_files: JsValue, base: Option<String>) -> Result<Self, JsError> {
        let files: std::collections::BTreeMap<String, String> =
            serde_wasm_bindgen::from_value(manifest_files).map_err(JsError::from)?;
        let inner =
            resolve(&files, base.as_deref().unwrap_or_default()).map_err(|e| to_js_error(&e))?;
        Ok(Self { inner })
    }

    /// The whole resolution as plain JS data: the container, every vertex type
    /// with its prefix, tile size, shift, count, index and declared levels, and
    /// every edge type with the orientations it publishes.
    ///
    /// **This is most of the surface**, and deliberately: what a resolved corpus
    /// is, is data, and data crosses a boundary once rather than a field at a
    /// time. Every row-count-width field arrives as a `BigInt`
    /// (`serialize_large_number_types_as_bigints`), because a `vertex_count`
    /// above 2^53 through a double comes out one tile short and takes the tail
    /// tile with it.
    ///
    /// # Errors
    ///
    /// A `JsError` if the resolution cannot be serialised.
    pub fn snapshot(&self) -> Result<JsValue, JsError> {
        self.inner
            .serialize(
                &serde_wasm_bindgen::Serializer::json_compatible()
                    .serialize_large_number_types_as_bigints(true),
            )
            .map_err(JsError::from)
    }

    /// The vertex type a name resolves to — the name itself when the manifest
    /// declares it, the first type the index names when none is given.
    ///
    /// The lookup exists as a call so the refusal does: a caller naming a type
    /// this corpus has never heard of gets the diagnosis that names the ones it
    /// has, from the reader rather than from a second copy of the list.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest.
    #[wasm_bindgen(js_name = vertexTypeName)]
    pub fn vertex_type_name(&self, vertex_type: Option<String>) -> Result<String, JsError> {
        Ok(self.vertex(vertex_type.as_deref())?.vertex_type.clone())
    }

    /// The tiles a batch of `dense_id`s live in — the whole of the addressing
    /// scheme, and batched because a reader asks it of a frontier and not of an id.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest.
    #[wasm_bindgen(js_name = tilesOf)]
    pub fn tiles_of(
        &self,
        vertex_type: Option<String>,
        dense_ids: Vec<u64>,
    ) -> Result<Vec<u64>, JsError> {
        let vertex = self.vertex(vertex_type.as_deref())?;
        Ok(dense_ids.into_iter().map(|id| vertex.tile_of(id)).collect())
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
        tile: u64,
    ) -> Result<String, JsError> {
        Ok(self.vertex(vertex_type.as_deref())?.tile_url(tile))
    }

    /// Every payload FILE of a vertex type, in order and distinct.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest, or when it declares no
    /// `vertex_count` — tiles are addressed and never listed, and HTTP gives no
    /// directory to fall back on.
    #[wasm_bindgen(js_name = vertexFiles)]
    pub fn vertex_files(&self, vertex_type: Option<String>) -> Result<Vec<String>, JsError> {
        self.vertex(vertex_type.as_deref())?
            .files()
            .map_err(|e| to_js_error(&e))
    }

    /// Every file the corpus can address, distinct and in declaration order —
    /// `fossil_graph::plan::ReadPlan::files`, the list a host signs or
    /// registers whole.
    #[must_use]
    pub fn files(&self) -> Vec<String> {
        self.inner.files()
    }

    /// Every file of a vertex type's identity index, in order and distinct.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest, declares no index, or
    /// declares no `vertex_count`.
    #[wasm_bindgen(js_name = indexFiles)]
    pub fn index_files(&self, vertex_type: Option<String>) -> Result<Vec<String>, JsError> {
        self.vertex(vertex_type.as_deref())?
            .index_files()
            .map_err(|e| to_js_error(&e))
    }

    /// The tile of one projection holding `dense_id`, or `null` when the corpus
    /// writes none at that scale.
    ///
    /// **The scale and never the exponent.** Tile `j` covers
    /// `[j · chunk_size · scale, (j+1) · chunk_size · scale)`, so the shift is
    /// the type's own plus `log2(scale)` — a product the manifest carries, which
    /// is why no `4` and no `2k` cross this boundary in either direction.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest.
    #[wasm_bindgen(js_name = projectionTileOf)]
    pub fn projection_tile_of(
        &self,
        vertex_type: Option<String>,
        scale: u64,
        dense_id: u64,
    ) -> Result<Option<u64>, JsError> {
        Ok(self
            .projection(vertex_type.as_deref(), scale)?
            .map(|p| p.tile_of(dense_id)))
    }

    /// The file tile `j` of one projection is in, spelled by the corpus's
    /// container, or `null` when the corpus writes none at that scale.
    ///
    /// `null` rather than a string is the point, and it is the point the
    /// adjacency made when it was a vocabulary of its own: a projection nobody
    /// wrote has no address, and handing back a URL that 404s is the failure
    /// this whole seam exists to prevent.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest.
    #[wasm_bindgen(js_name = projectionTileUrl)]
    pub fn projection_tile_url(
        &self,
        vertex_type: Option<String>,
        scale: u64,
        tile: u64,
    ) -> Result<Option<String>, JsError> {
        Ok(self
            .projection(vertex_type.as_deref(), scale)?
            .map(|p| p.tile_url(tile)))
    }

    /// Every file of one projection of a vertex type, in order and distinct.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest, declares no
    /// `vertex_count`, or wrote no projection at `scale` — the last one naming
    /// the scales it did write, because a URL under an `l{k}/` nobody wrote is
    /// the one failure a reader cannot tell from an empty level.
    #[wasm_bindgen(js_name = projectionFiles)]
    pub fn projection_files(
        &self,
        vertex_type: Option<String>,
        scale: u64,
    ) -> Result<Vec<String>, JsError> {
        self.vertex(vertex_type.as_deref())?
            .projection_files(scale)
            .map_err(|e| to_js_error(&e))
    }

    /// The tile of one projection of a RELATION holding `dense_id`, or `null`
    /// when the corpus publishes neither that orientation nor that scale.
    ///
    /// The id is the ALIGNED endpoint's, which is what lets a coarse camera draw
    /// a line without opening the vertex payload for its far end: the tiles
    /// already selected address these with no new arithmetic.
    ///
    /// # Errors
    ///
    /// A `JsError` when the edge type is not in the manifest, or `direction` is
    /// neither `src` nor `dst`.
    #[wasm_bindgen(js_name = edgeProjectionTileOf)]
    pub fn edge_projection_tile_of(
        &self,
        edge_type: &str,
        direction: &str,
        scale: u64,
        dense_id: u64,
    ) -> Result<Option<u64>, JsError> {
        Ok(self
            .edge_projection(edge_type, direction, scale)?
            .map(|p| p.tile_of(dense_id)))
    }

    /// The file tile `j` of one projection of a RELATION is in, or `null` when
    /// the corpus publishes neither that orientation nor that scale.
    ///
    /// # Errors
    ///
    /// A `JsError` when the edge type is not in the manifest, or `direction` is
    /// neither `src` nor `dst`.
    #[wasm_bindgen(js_name = edgeProjectionTileUrl)]
    pub fn edge_projection_tile_url(
        &self,
        edge_type: &str,
        direction: &str,
        scale: u64,
        tile: u64,
    ) -> Result<Option<String>, JsError> {
        Ok(self
            .edge_projection(edge_type, direction, scale)?
            .map(|p| p.tile_url(tile)))
    }

    /// Every file of one projection of a RELATION, in order and distinct.
    ///
    /// # Errors
    ///
    /// A `JsError` when the edge type is not in the manifest, `direction` is
    /// neither `src` nor `dst`, the aligned endpoint declares no `vertex_count`,
    /// or the corpus wrote no projection at `scale` in that orientation.
    #[wasm_bindgen(js_name = edgeProjectionFiles)]
    pub fn edge_projection_files(
        &self,
        edge_type: &str,
        direction: &str,
        scale: u64,
    ) -> Result<Vec<String>, JsError> {
        let direction = parse_direction(direction)?;
        self.edge(edge_type)?
            .projection_files(scale, direction)
            .map_err(|e| to_js_error(&e))
    }

    /// Which relations a picture of one vertex type may draw — both endpoints in
    /// its own `dense_id` space — and which of the incident ones it leaves out,
    /// with the reason. `relations` are indices into the plan's `edges`.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest.
    pub fn drawing(&self, vertex_type: Option<String>) -> Result<JsValue, JsError> {
        self.inner
            .drawing(vertex_type.as_deref())
            .map_err(|e| to_js_error(&e))?
            .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
            .map_err(JsError::from)
    }

    /// The URLs a set of vertex tiles addresses, and what that set is complete
    /// for. `directions` of `["src"]` is the out-edge read and both orientations
    /// is the incident set — neither is the set a picture may DRAW, which is
    /// `drawing` above.
    ///
    /// # Errors
    ///
    /// A `JsError` when the type is not in the manifest, or a direction is neither
    /// `src` nor `dst`.
    pub fn window(
        &self,
        vertex_type: Option<String>,
        tiles: Vec<u64>,
        directions: Vec<String>,
    ) -> Result<JsValue, JsError> {
        let directions: Vec<Direction> = directions
            .iter()
            .map(|d| parse_direction(d))
            .collect::<Result<_, _>>()?;
        self.inner
            .window(vertex_type.as_deref(), &tiles, &directions)
            .map_err(|e| to_js_error(&e))?
            .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
            .map_err(JsError::from)
    }
}
