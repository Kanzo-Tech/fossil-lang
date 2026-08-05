//! The aggregation verb — `aggregate`.
//!
//! Constant-memory by construction: one SQL pass with a bounded result set
//! (`GROUP BY` cardinality cap or `LIMIT`). Safe to expose to LLM tool callers
//! without further cost-gating.

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// aggregate
// ──────────────────────────────────────────────────────────────────────────

/// One grouping, over values or over ranges.
///
/// **Binning is grouping**, which is why `histogram` is not a second verb: it
/// was the same `GROUP BY` with the key computed from a range instead of read
/// from a column. Setting [`Self::bins`] is what picks which, and it is the
/// only difference between the two.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AggregateParams {
    pub vertex_type: String,
    /// The column the groups come from — its values, or its ranges when
    /// [`Self::bins`] is set.
    pub group_by: String,
    pub agg: Aggregation,
    /// Optional measure column for `sum`/`avg`/`min`/`max` aggregations.
    /// Ignored when `agg` is `count`.
    #[serde(default)]
    pub measure: Option<String>,
    /// Group over this many equal-width ranges of `group_by` rather than over
    /// its distinct values — what `histogram` used to be. Requires a numeric or
    /// temporal column: a categorical one has no ranges, and grouping it by
    /// value is already the answer.
    #[serde(default)]
    pub bins: Option<u32>,
    /// Cap on rows returned. Groups are ordered by value and cut here; a binned
    /// call is cut to this many bins instead, so `limit` means one thing.
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
    /// Bin boundaries, `rows.len() + 1` of them, low to high. **Empty unless
    /// the call set `bins`** — a grouping over values has no axis to draw.
    pub edges: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AggregateRow {
    /// The group's key: the column's value, or the bin's ordinal when the call
    /// was binned (pair it with `AggregateResult::edges` for the range).
    pub group: serde_json::Value,
    pub value: f64,
}
