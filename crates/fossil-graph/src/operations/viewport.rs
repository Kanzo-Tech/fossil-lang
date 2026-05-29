//! Viewport verbs — `viewport`, `set_selection`.
//!
//! The viewport verb is the **larger-than-RAM-friendly** read path: it
//! returns at most `limit` vertices intersecting the bbox + LOD threshold,
//! emitted as typed-array-ready dense indices so the consumer can ship the
//! result straight to a GPU buffer. The implementation MUST translate to
//! SQL with a `WHERE x BETWEEN … AND y BETWEEN …` clause so `DuckDB`'s
//! predicate-pushdown skips non-matching morton-sorted Parquet row groups.
//!
//! `set_selection` is the crossfilter bridge: it accepts a Mosaic-style
//! filter expression and stashes it in the executor so subsequent verb
//! calls inherit the filter. The shape mirrors `@uwdata/mosaic-core::Selection`
//! so the keasy viewer can forward its current state 1-for-1.

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// viewport
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewportParams {
    pub bbox: BoundingBox,
    /// Current zoom level. Above [`Self::lod_threshold`] the executor swaps
    /// to aggregate mode (`GROUP BY cluster_id`) returning ≤ 10k super-nodes
    /// regardless of total N.
    pub zoom: f32,
    #[serde(default = "default_lod_threshold")]
    pub lod_threshold: f32,
    #[serde(default = "default_viewport_limit")]
    pub limit: u32,
    /// Restrict to a subset of vertex types. Empty = all.
    #[serde(default)]
    pub vertex_types: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BoundingBox {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

const fn default_lod_threshold() -> f32 {
    0.5
}

const fn default_viewport_limit() -> u32 {
    500_000
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewportResult {
    pub mode: ViewportMode,
    pub n: u32,
    /// Always present; in aggregate mode the entries are super-nodes
    /// (cluster centroids) with `cluster_id` populated.
    pub vertices: Vec<ViewportVertex>,
    pub edges: Vec<ViewportEdge>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViewportMode {
    Detail,
    Aggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewportVertex {
    pub dense_id: u32,
    pub x: f32,
    pub y: f32,
    pub type_idx: u8,
    /// Aggregate mode only: cluster size + `cluster_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewportEdge {
    pub src_dense: u32,
    pub dst_dense: u32,
}

// ──────────────────────────────────────────────────────────────────────────
// set_selection
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSelectionParams {
    /// Mosaic-Selection-compatible filter expression. `None` clears the
    /// current selection so the next viewport call returns the full bbox.
    pub selection: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SetSelectionResult {
    /// Echo of how many vertices the new selection would yield against the
    /// last viewport bbox. Lets the caller decide whether to repaint.
    pub matching_count: u32,
}
