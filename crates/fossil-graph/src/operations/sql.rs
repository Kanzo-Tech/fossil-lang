//! Escape-hatch verb — `execute_sql`.
//!
//! Bindings MAY hide this verb behind a permission flag. The keasy proxy
//! gates it: only org admins reach this tool; participant-role users see the
//! other five verbs but not this one. The reason is dual:
//!
//! 1. SQL is unbounded — a bad query brings down the WASM `DuckDB` heap.
//! 2. The other five verbs cover the supported question shapes with
//!    predictable cost. `execute_sql` is the LLM's last resort when none
//!    of those fit — useful but not for every caller.
//!
//! **`read`'s `where` is the same authority**, and a binding that gates this
//! verb must gate that field with it.
//!
//! When exposed, the result shape is the DuckDB-row-as-JSON form. Rows are
//! `serde_json::Value`s because the schema is unknown at verb-call time.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteSqlParams {
    pub sql: String,
    /// Hard cap on rows returned to the caller. The executor MUST apply an
    /// outer `LIMIT` regardless of what the user's SQL contains.
    #[serde(default = "default_row_cap")]
    pub row_cap: u32,
    /// Hard cap on wall-clock execution time, milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u32,
}

const fn default_row_cap() -> u32 {
    10_000
}

const fn default_timeout_ms() -> u32 {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecuteSqlResult {
    pub columns: Vec<ColumnDescriptor>,
    pub rows: Vec<serde_json::Value>,
    /// True when the result was truncated by `row_cap`.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ColumnDescriptor {
    pub name: String,
    pub duckdb_type: String,
}
