//! The verbs of the fossil-graph surface. **Six**, and the set is closed:
//! `read` · `expand{into|all}` · `path` · `aggregate` · `schema` ·
//! `execute_sql`. A seventh enters only if it serves a case these six
//! demonstrably cannot, with the demonstration being a measurement.
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

pub mod aggregate;
pub mod discovery;
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
/// `{ "verb": "schema", "params": { … } }` — identical for MCP tool calls,
/// HTTP POST bodies, and CLI subcommand args.
///
/// # It serialises, and it does not deserialise
///
/// There is no `Deserialize` impl, and its absence is load-bearing rather than
/// an oversight: two variants carry [`RawSql`], and [`Operation::from_wire`] is
/// the one door from wire JSON because that door takes the permission those
/// two fields share. [`raw_sql`] is the argument.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(tag = "verb", content = "params", rename_all = "snake_case")]
pub enum Operation {
    // Introspection — the manifest, and per-field statistics on request.
    Schema(schema::SchemaParams),

    // Reads — rows of one type, the neighbourhood of a set, the route between
    // two. `read` absorbed `get_vertex` (a predicate on `subject`) and `top_k`
    // (an order and a limit).
    Read(discovery::ReadParams),
    Expand(discovery::ExpandParams),
    Path(discovery::PathParams),

    // Aggregation — bounded constant-memory queries. Binning is grouping, so
    // `histogram` is a parameter of this one rather than a verb beside it.
    Aggregate(aggregate::AggregateParams),

    // Escape hatch — text2sql lives here. Bindings MAY hide this verb behind
    // a permission flag (keasy proxy disables it for participant users).
    ExecuteSql(sql::ExecuteSqlParams),
}

impl Operation {
    /// Which verb this is.
    #[must_use]
    pub const fn verb(&self) -> Verb {
        match self {
            Self::Schema(_) => Verb::Schema,
            Self::Read(_) => Verb::Read,
            Self::Expand(_) => Verb::Expand,
            Self::Path(_) => Verb::Path,
            Self::Aggregate(_) => Verb::Aggregate,
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
    /// `sql` is the permission the two raw-SQL fields share:
    /// `execute_sql`'s `sql` and `read`'s `where`. `None` refuses both; `Some`
    /// admits both. There is no third answer, which is the point — see
    /// [`raw_sql`].
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

/// A verb by name — the closed set, for a binding that has a tool name in hand
/// rather than a payload.
///
/// It exists because the MCP binding registers **one tool per verb** and needs
/// the set, its prose and its schemas without holding a second copy of any of
/// them. Every method below is an exhaustive `match`, so a seventh variant on
/// [`Operation`] cannot land without answering all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Schema,
    Read,
    Expand,
    Path,
    Aggregate,
    ExecuteSql,
}

impl Verb {
    /// The six, in the order a caller meets them: introspect, read, summarise,
    /// and then the hatch.
    pub const ALL: [Self; 6] = [
        Self::Schema,
        Self::Read,
        Self::Expand,
        Self::Path,
        Self::Aggregate,
        Self::ExecuteSql,
    ];

    /// The wire name — the serde tag, and the MCP tool name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::Read => "read",
            Self::Expand => "expand",
            Self::Path => "path",
            Self::Aggregate => "aggregate",
            Self::ExecuteSql => "execute_sql",
        }
    }

    /// Parse a wire name back. `None` for anything outside the six.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }

    /// **Does this verb put a caller's SQL in front of the engine?**
    ///
    /// True for `execute_sql` always, and for `read` because of `where` — which
    /// is the whole of the rule that used to be a comment. A binding listing
    /// its tools under a policy that withholds SQL must drop `execute_sql`; it
    /// keeps `read`, whose `where` the same policy closes at
    /// [`Operation::from_wire`].
    #[must_use]
    pub const fn reaches_raw_sql(self) -> bool {
        match self {
            Self::ExecuteSql | Self::Read => true,
            Self::Schema | Self::Expand | Self::Path | Self::Aggregate => false,
        }
    }

    /// One sentence for a caller choosing between the six. This is the text an
    /// MCP client shows a model, and it lives here rather than in a binding so
    /// the two bindings cannot describe the same verb differently.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Schema => {
                "What the corpus contains: its vertex types, its edge types, and — when \
                 `vertex_type` is given — that type's fields, with per-field statistics when \
                 `field` names one. Start here; the other verbs take names this one returns."
            }
            Self::Read => {
                "Rows of one vertex type, filtered by a SQL `WHERE` predicate, ordered by a \
                 column, and capped. Reading one vertex by IRI is `where: \"subject = '…'\"`; \
                 a top-N is an `order_by` plus a `limit`."
            }
            Self::Expand => {
                "The neighbourhood of a set of vertices named by subject IRI: `all` walks \
                 outward up to `depth` hops, `into` keeps only the edges whose both ends are \
                 already in the set. Returns vertices and edges."
            }
            Self::Path => {
                "The shortest route between two vertices, by subject IRI, within `max_hops`."
            }
            Self::Aggregate => {
                "A grouped summary of one vertex type: count/sum/avg/min/max over the values of \
                 `group_by`, or over `bins` equal-width ranges of it when that column is numeric \
                 or temporal. Bounded by `limit`."
            }
            Self::ExecuteSql => {
                "Run SQL directly against the corpus views (one view per vertex type and per \
                 edge type, named as `schema` reports them). The escape hatch, for questions the \
                 other five verbs cannot shape; results are capped by `row_cap`."
            }
        }
    }

    /// The verb's params as a JSON Schema document.
    ///
    /// The same `schemars` derive the snapshot tests publish, so an MCP tool's
    /// input schema and the wire contract are one artefact rather than two that
    /// agree today.
    #[must_use]
    pub fn params_schema(self) -> Value {
        fn of<T: schemars::JsonSchema>() -> Value {
            serde_json::to_value(schemars::schema_for!(T))
                .unwrap_or_else(|e| unreachable!("a schemars schema serialises: {e}"))
        }
        match self {
            // The wire mirrors, not the structs: what a caller sends is what
            // the schema must describe, and `where`/`sql` arrive as strings.
            Self::Read => of::<discovery::WireReadParams>(),
            Self::ExecuteSql => of::<sql::WireExecuteSqlParams>(),
            Self::Schema => of::<schema::SchemaParams>(),
            Self::Expand => of::<discovery::ExpandParams>(),
            Self::Path => of::<discovery::PathParams>(),
            Self::Aggregate => of::<aggregate::AggregateParams>(),
        }
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
            Self::Expand => Operation::Expand(serde_json::from_value(params).map_err(bad)?),
            Self::Path => Operation::Path(serde_json::from_value(params).map_err(bad)?),
            Self::Aggregate => Operation::Aggregate(serde_json::from_value(params).map_err(bad)?),
            Self::Read => {
                let w: discovery::WireReadParams = serde_json::from_value(params).map_err(bad)?;
                let predicate = match w.r#where {
                    None => None,
                    Some(text) => Some(RawSql::new(
                        sql.ok_or(GraphError::RawSqlWithheld {
                            field: "read.where",
                        })?,
                        text,
                    )),
                };
                Operation::Read(discovery::ReadParams {
                    vertex_type: w.vertex_type,
                    r#where: predicate,
                    order_by: w.order_by,
                    descending: w.descending,
                    limit: w.limit,
                })
            }
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
