//! Native [`DuckExecutor`] — runs fossil-graph verb SQL against bundled `DuckDB`.
//!
//! `fossil-graph` owns the verb→SQL logic (WASM-clean); this crate only
//! satisfies the [`DuckExecutor`] seam by running that SQL and converting each
//! result row to a JSON object. The WASM counterpart, `fossil_graph_wasm::JsExecutor`,
//! implements the same seam against DuckDB-WASM — neither re-derives a verb's
//! SQL.
//!
//! The executor futures are single-threaded by design (see `fossil_graph::executor`),
//! so `future_not_send` is allowed here too.
#![allow(clippy::future_not_send)]

use duckdb::Connection;
use duckdb::types::Value as DuckValue;
use fossil_graph::{DuckExecutor, GraphError, QueryResult, Result};
use serde_json::{Map, Value};

/// A [`DuckExecutor`] backed by a native `DuckDB` [`Connection`]. The caller
/// owns the connection (and registers the `GraphAr` vertex/edge views on it); this
/// borrows it for the lifetime of a dispatch.
///
/// It is named for what BACKS it and not for `Duck`, which distinguishes
/// nothing: every implementor of [`DuckExecutor`] is a `DuckDB` one — that is
/// what the trait says, and `DuckDB` there is load-bearing because the verb SQL
/// is `DuckDB`'s dialect. `fossil_graph_wasm::JsExecutor` is named for the
/// `js_sys::Function` it wraps; this wraps a [`Connection`].
#[derive(Debug)]
pub struct ConnectionExecutor<'c> {
    conn: &'c Connection,
}

impl<'c> ConnectionExecutor<'c> {
    /// Wrap a connection on which the `GraphAr` views are already registered.
    #[must_use]
    pub const fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }
}

impl DuckExecutor for ConnectionExecutor<'_> {
    // Native execution is synchronous; the `async fn` just wraps it in an
    // immediately-ready future so the verb surface stays single-source across
    // the native runtime and the async DuckDB-WASM binding.
    async fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
        self.run(sql)
            .map(|result| result.rows)
            .map_err(|e| GraphError::Execution(e.to_string()))
    }

    async fn query_columns(&self, sql: &str) -> Result<QueryResult> {
        self.run(sql)
            .map_err(|e| GraphError::Execution(e.to_string()))
    }
}

impl ConnectionExecutor<'_> {
    /// Run `sql`, returning real `(column_name, type)` descriptors + JSON rows.
    /// Column metadata is only populated once the query has executed, so it is
    /// read from the executed statement (via `rows`), not the prepared one. The
    /// type string is the Arrow logical-type spelling `DuckDB` exposes.
    fn run(&self, sql: &str) -> duckdb::Result<QueryResult> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let columns: Vec<(String, String)> = rows.as_ref().map_or_else(Vec::new, |stmt| {
            stmt.column_names()
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, format!("{:?}", stmt.column_type(i))))
                .collect()
        });
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut obj = Map::with_capacity(columns.len());
            for (i, (name, _)) in columns.iter().enumerate() {
                let value: DuckValue = row.get(i)?;
                obj.insert(name.clone(), duck_to_json(value));
            }
            out.push(Value::Object(obj));
        }
        Ok(QueryResult { columns, rows: out })
    }
}

/// Convert an owned `DuckDB` value to JSON. Lists recurse (the `path` verb
/// returns `VARCHAR[]` path columns); unhandled exotic types fall back to their
/// debug string so a verb never fails on an unexpected column type.
fn duck_to_json(v: DuckValue) -> Value {
    match v {
        DuckValue::Null => Value::Null,
        DuckValue::Boolean(b) => Value::Bool(b),
        DuckValue::TinyInt(n) => Value::from(n),
        DuckValue::SmallInt(n) => Value::from(n),
        DuckValue::Int(n) => Value::from(n),
        DuckValue::BigInt(n) => Value::from(n),
        DuckValue::UTinyInt(n) => Value::from(n),
        DuckValue::USmallInt(n) => Value::from(n),
        DuckValue::UInt(n) => Value::from(n),
        DuckValue::UBigInt(n) => Value::from(n),
        DuckValue::HugeInt(n) => {
            i64::try_from(n).map_or_else(|_| Value::String(n.to_string()), Value::from)
        }
        DuckValue::Float(f) => json_f64(f64::from(f)),
        DuckValue::Double(f) => json_f64(f),
        DuckValue::Text(s) => Value::String(s),
        DuckValue::List(items) | DuckValue::Array(items) => {
            Value::Array(items.into_iter().map(duck_to_json).collect())
        }
        other => Value::String(format!("{other:?}")),
    }
}

fn json_f64(f: f64) -> Value {
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}
