//! The verbs of the fossil-graph surface. **Two**, and the set is closed:
//! `schema` and `execute_sql`. A third enters only if it serves a case these
//! two demonstrably cannot, with the demonstration being a measurement.
//!
//! There were six. `read`, `expand`, `path` and `aggregate` were each a
//! bounded shape of a question `execute_sql` can already ask; their callers
//! were an MCP server and the corpus door's members of the same names, and
//! neither had a consumer left.
//!
//! **The camera is addressed, not queried** — the LOD is not a filter, it is a
//! different relation, and a `WHERE` cannot change which table it reads. The
//! tiles answer that, and they are not a verb. `/docs/design/corpus` argues it.
//!
//! Each verb is a variant of the [`Operation`] tagged enum carrying a `Params`
//! struct, with the paired `Result` type in the same submodule — a closed set
//! every transport binding dispatches by exhaustive match, which is why it is
//! an enum and not a trait plus a registry. Adding a verb means a variant, a
//! `Params`/`Result` pair and a snapshot test; removing one breaks every
//! binding.

pub mod raw_sql;
pub mod schema;
pub mod sql;

use serde::Serialize;
use serde_json::Value;

pub use raw_sql::{RawSql, RawSqlAccess};

use crate::error::{GraphError, Result};

/// All graph operations dispatchable on the surface.
///
/// The `tag = "verb"` serde representation makes the wire form
/// `{ "verb": "schema", "params": { … } }`, whatever transport carries it.
///
/// # It serialises, and it does not deserialise
///
/// There is no `Deserialize` impl, and its absence is load-bearing rather than
/// an oversight: `execute_sql` carries [`RawSql`], and [`Operation::from_wire`]
/// is the one door from wire JSON because that door takes the permission.
/// [`raw_sql`] is the argument.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(tag = "verb", content = "params", rename_all = "snake_case")]
pub enum Operation {
    // Introspection — the manifest, and per-field statistics on request.
    Schema(schema::SchemaParams),

    // SQL over the corpus views — text2sql lives here. A binding MAY withhold
    // it; see [`raw_sql`].
    ExecuteSql(sql::ExecuteSqlParams),
}

impl Operation {
    /// Which verb this is.
    #[must_use]
    pub const fn verb(&self) -> Verb {
        match self {
            Self::Schema(_) => Verb::Schema,
            Self::ExecuteSql(_) => Verb::ExecuteSql,
        }
    }

    /// Static verb name for diagnostics and binding error wrapping. Mirrors
    /// the `rename_all = "snake_case"` of the serde tag so the diagnostic
    /// string matches what bindings receive over the wire.
    #[must_use]
    pub const fn verb_name(&self) -> &'static str {
        self.verb().name()
    }

    /// The one way in from wire JSON — `{ "verb": …, "params": { … } }`.
    ///
    /// `sql` is the permission `execute_sql`'s `sql` needs: `None` refuses it,
    /// `Some` admits it — see [`raw_sql`].
    ///
    /// # Errors
    ///
    /// [`GraphError::InvalidParams`] when the envelope or the params do not
    /// parse, and [`GraphError::RawSqlWithheld`] when the payload reaches for
    /// SQL that `sql` did not grant.
    pub fn from_wire(value: &Value, sql: Option<RawSqlAccess>) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            verb: String,
            #[serde(default)]
            params: Value,
        }
        let env: Envelope =
            serde_json::from_value(value.clone()).map_err(|e| GraphError::InvalidParams {
                verb: "<envelope>",
                detail: e.to_string(),
            })?;
        let verb = Verb::from_name(&env.verb).ok_or_else(|| GraphError::InvalidParams {
            verb: "<envelope>",
            detail: format!("unknown verb `{}`", env.verb),
        })?;
        verb.parse_params(env.params, sql)
    }
}

/// A verb by name — the closed set, for parsing a wire envelope's tag.
///
/// Every method below is an exhaustive `match`, so a variant on [`Operation`]
/// cannot land without answering all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Schema,
    ExecuteSql,
}

impl Verb {
    /// Both, in the order a caller meets them: introspect, then query.
    pub const ALL: [Self; 2] = [Self::Schema, Self::ExecuteSql];

    /// The wire name — the serde tag.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::ExecuteSql => "execute_sql",
        }
    }

    /// Parse a wire name back. `None` for anything outside the set.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }

    /// Parse this verb's params, applying the raw-SQL permission.
    ///
    /// # Errors
    ///
    /// [`GraphError::InvalidParams`] when the params do not parse, and
    /// [`GraphError::RawSqlWithheld`] when they carry SQL that `sql` did not
    /// grant.
    pub fn parse_params(self, params: Value, sql: Option<RawSqlAccess>) -> Result<Operation> {
        let verb = self.name();
        let bad = |e: serde_json::Error| GraphError::InvalidParams {
            verb,
            detail: e.to_string(),
        };
        Ok(match self {
            Self::Schema => Operation::Schema(serde_json::from_value(params).map_err(bad)?),
            Self::ExecuteSql => {
                let w: sql::WireExecuteSqlParams = serde_json::from_value(params).map_err(bad)?;
                Operation::ExecuteSql(sql::ExecuteSqlParams {
                    sql: RawSql::new(
                        sql.ok_or(GraphError::RawSqlWithheld {
                            field: "execute_sql.sql",
                        })?,
                        w.sql,
                    ),
                    row_cap: w.row_cap,
                })
            }
        })
    }
}
