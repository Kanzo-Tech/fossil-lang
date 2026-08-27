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
//! verb gates that field with it — not by remembering to, but because both
//! fields are [`RawSql`] and one token fills them.
//! [`raw_sql`](super::raw_sql) is the argument.
//!
//! When exposed, the result shape is the DuckDB-row-as-JSON form. Rows are
//! `serde_json::Value`s because the schema is unknown at verb-call time.

use serde::{Deserialize, Serialize};

use super::raw_sql::RawSql;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteSqlParams {
    pub sql: RawSql,
    /// Hard cap on rows returned to the caller. The executor MUST apply an
    /// outer `LIMIT` regardless of what the user's SQL contains.
    #[serde(default = "default_row_cap")]
    pub row_cap: u32,
}

/// [`ExecuteSqlParams`] as it arrives. See [`WireReadParams`] for why the
/// mirror exists and where its drift is caught.
///
/// [`WireReadParams`]: super::discovery::WireReadParams
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WireExecuteSqlParams {
    pub sql: String,
    /// Hard cap on rows returned to the caller. The executor MUST apply an
    /// outer `LIMIT` regardless of what the user's SQL contains.
    #[serde(default = "default_row_cap")]
    pub row_cap: u32,
}

const fn default_row_cap() -> u32 {
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
