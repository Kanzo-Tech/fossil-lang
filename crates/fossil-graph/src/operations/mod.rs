//! The 17 verbs of the fossil-graph surface.
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
pub mod graphrag;
pub mod schema;
pub mod sql;
pub mod viewport;

use serde::{Deserialize, Serialize};

/// All graph operations dispatchable on the surface.
///
/// The `tag = "verb"` serde representation makes the wire form
/// `{ "verb": "list_vertex_types", "params": { … } }` — identical for MCP
/// tool calls, HTTP POST bodies, and CLI subcommand args.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "verb", content = "params", rename_all = "snake_case")]
pub enum Operation {
    // Schema introspection — no SQL, just manifest reads.
    ListVertexTypes(schema::ListVertexTypesParams),
    ListEdgeTypes(schema::ListEdgeTypesParams),
    DescribeField(schema::DescribeFieldParams),
    DescribeVertexType(schema::DescribeVertexTypeParams),

    // Discovery — semantic / structural lookups.
    SearchByLabel(discovery::SearchByLabelParams),
    FindNeighbors(discovery::FindNeighborsParams),
    FindPath(discovery::FindPathParams),
    GetVertex(discovery::GetVertexParams),

    // Aggregation — bounded constant-memory queries.
    Aggregate(aggregate::AggregateParams),
    Histogram(aggregate::HistogramParams),
    TopK(aggregate::TopKParams),

    // GraphRAG — relies on writer-emitted cluster_summary + embedding columns
    // (writer W3). Stubs return NotImplemented until then.
    SummarizeCluster(graphrag::SummarizeClusterParams),
    AnswerWithCommunities(graphrag::AnswerWithCommunitiesParams),

    // Viewport — the larger-than-RAM-friendly visual layer (writer W3 emits
    // morton-sorted vertex Parquet so bbox + LIMIT = predicate pushdown).
    Viewport(viewport::ViewportParams),
    SetSelection(viewport::SetSelectionParams),
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
            Self::ListVertexTypes(_) => "list_vertex_types",
            Self::ListEdgeTypes(_) => "list_edge_types",
            Self::DescribeField(_) => "describe_field",
            Self::DescribeVertexType(_) => "describe_vertex_type",
            Self::SearchByLabel(_) => "search_by_label",
            Self::FindNeighbors(_) => "find_neighbors",
            Self::FindPath(_) => "find_path",
            Self::GetVertex(_) => "get_vertex",
            Self::Aggregate(_) => "aggregate",
            Self::Histogram(_) => "histogram",
            Self::TopK(_) => "top_k",
            Self::SummarizeCluster(_) => "summarize_cluster",
            Self::AnswerWithCommunities(_) => "answer_with_communities",
            Self::Viewport(_) => "viewport",
            Self::SetSelection(_) => "set_selection",
            Self::MaterializeGraph(_) => "materialize_graph",
            Self::ExecuteSql(_) => "execute_sql",
        }
    }
}
