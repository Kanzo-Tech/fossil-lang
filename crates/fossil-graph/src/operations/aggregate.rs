//! Aggregation verbs — `aggregate`, `histogram`, `top_k`.
//!
//! Constant-memory by construction: every verb translates to a single SQL
//! pass with a bounded result set (`GROUP BY` cardinality cap or `LIMIT`).
//! Safe to expose to LLM tool callers without further cost-gating.

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// aggregate
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AggregateParams {
    pub vertex_type: String,
    pub group_by: String,
    pub agg: Aggregation,
    /// Optional measure column for `sum`/`avg`/`min`/`max` aggregations.
    /// Ignored when `agg` is `count`.
    #[serde(default)]
    pub measure: Option<String>,
    #[serde(default = "default_aggregate_limit")]
    pub limit: u32,
}

const fn default_aggregate_limit() -> u32 {
    1000
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AggregateResult {
    pub rows: Vec<AggregateRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AggregateRow {
    pub group: serde_json::Value,
    pub value: f64,
}

// ──────────────────────────────────────────────────────────────────────────
// histogram
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistogramParams {
    pub vertex_type: String,
    pub field: String,
    #[serde(default = "default_bins")]
    pub bins: u32,
}

const fn default_bins() -> u32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HistogramResult {
    /// Bin edges (length `bins + 1` for numeric, `bins` for categorical).
    pub edges: Vec<f64>,
    /// Per-bin counts.
    pub counts: Vec<u64>,
    /// Field role echoed back so the caller can pick the right chart.
    pub field_kind: HistogramKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistogramKind {
    Numeric,
    Temporal,
    Categorical,
}

// ──────────────────────────────────────────────────────────────────────────
// top_k
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopKParams {
    pub vertex_type: String,
    pub order_by: String,
    #[serde(default = "default_k")]
    pub k: u32,
    #[serde(default)]
    pub descending: bool,
}

const fn default_k() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TopKResult {
    /// Rows as opaque JSON objects so the verb stays generic across
    /// arbitrary vertex shapes. Bindings render to their UI of choice.
    pub rows: Vec<serde_json::Value>,
}
