//! Escape-hatch verb — `execute_sql`.
//!
//! Bindings MAY hide this verb behind a permission: SQL is unbounded, and a
//! bad query can bring down the WASM `DuckDB` heap. Its `sql` is [`RawSql`],
//! which only a permission token fills — [`raw_sql`](super::raw_sql) is the
//! argument.
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

/// [`ExecuteSqlParams`] as it arrives — the same fields with `sql` still a
/// bare `String`, because a permission has not been applied to it yet.
///
/// **The one risk this shape carries is drifting from the struct it mirrors**,
/// and `crates/fossil-graph/tests/schemas.rs` is where that is caught: it
/// derives both schemas and asserts they are the same document, down to the
/// per-field prose. A field added above and not here fails there rather than
/// silently becoming unreachable from the wire.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WireExecuteSqlParams {
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
