//! The verbs of the fossil-graph surface, on the way from seventeen to six.
//!
//! ADR-0042 closes the surface at `read`, `expand{into|all}`, `path`,
//! `aggregate`, `schema` and `execute_sql`. **Ten remain.** The four that never
//! had an implementation are gone, three of them because they were never verbs
//! — `summarize_cluster` and `answer_with_communities` are a `read` over a
//! level, and `set_selection` always belonged to the client. The four
//! introspection verbs are gone too: `schema` answers all of them, and what
//! used to separate the cheap ones from the expensive one is now a parameter.
//!
//! Each verb is a unit-struct on the [`Operation`] tagged enum with paired
//! `Params` and `Result` types in its own submodule. The enum is the closed
//! set of operations every transport binding can dispatch — adding a verb
//! means adding a variant + a `Params` / `Result` pair + a snapshot test.
//! Removing one is a breaking change to every binding ([[`feedback_rust_enum_not_trait_registry`]]:
//! enum over trait+registry for closed sets — the bindings benefit from
//! exhaustiveness checks on the match).

pub mod aggregate;
pub mod discovery;
pub mod schema;
pub mod sql;
pub mod viewport;

use serde::{Deserialize, Serialize};

/// All graph operations dispatchable on the surface.
///
/// The `tag = "verb"` serde representation makes the wire form
/// `{ "verb": "schema", "params": { … } }` — identical for MCP tool calls,
/// HTTP POST bodies, and CLI subcommand args.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "verb", content = "params", rename_all = "snake_case")]
pub enum Operation {
    // Introspection — the manifest, and per-field statistics on request.
    Schema(schema::SchemaParams),

    // Discovery — semantic / structural lookups.
    FindNeighbors(discovery::FindNeighborsParams),
    FindPath(discovery::FindPathParams),
    GetVertex(discovery::GetVertexParams),

    // Aggregation — bounded constant-memory queries.
    Aggregate(aggregate::AggregateParams),
    Histogram(aggregate::HistogramParams),
    TopK(aggregate::TopKParams),

    // Viewport — the larger-than-RAM-friendly visual layer (writer W3 emits
    // morton-sorted vertex Parquet so bbox + LIMIT = predicate pushdown).
    // Both leave with ADR-0042: the camera is addressed, not queried.
    Viewport(viewport::ViewportParams),
    MaterializeGraph(viewport::MaterializeGraphParams),

    // Escape hatch — text2sql lives here. Bindings MAY hide this verb behind
    // a permission flag (keasy proxy disables it for participant users).
    ExecuteSql(sql::ExecuteSqlParams),
}

impl Operation {
    /// Static verb name for diagnostics and binding error wrapping. Mirrors
    /// the `rename_all = "snake_case"` of the serde tag so the diagnostic
    /// string matches what bindings receive over the wire.
    #[must_use]
    pub const fn verb_name(&self) -> &'static str {
        match self {
            Self::Schema(_) => "schema",
            Self::FindNeighbors(_) => "find_neighbors",
            Self::FindPath(_) => "find_path",
            Self::GetVertex(_) => "get_vertex",
            Self::Aggregate(_) => "aggregate",
            Self::Histogram(_) => "histogram",
            Self::TopK(_) => "top_k",
            Self::Viewport(_) => "viewport",
            Self::MaterializeGraph(_) => "materialize_graph",
            Self::ExecuteSql(_) => "execute_sql",
        }
    }
}
