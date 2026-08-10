//! The read verbs — `read`, `expand{into|all}`, `path`.
//!
//! - `read` is the general bounded read of one vertex type: a predicate, an
//!   order, a limit. It absorbed `get_vertex` (a predicate on `subject`) and
//!   `top_k` (an order and a limit).
//! - `expand` walks the `GraphAr` edge tables from a set of vertices, either to
//!   whatever they reach or only among themselves.
//! - `path` returns the shortest route between two vertices (BFS over
//!   reachable edges).

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// read
// ──────────────────────────────────────────────────────────────────────────

/// Rows of one vertex type, filtered, ordered and capped.
///
/// **`where` is SQL and is trusted exactly as far as `execute_sql` is.** A
/// binding that gates the escape hatch behind a permission MUST gate this field
/// with it: the two carry the same authority over the same engine.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadParams {
    pub vertex_type: String,
    /// A `WHERE` predicate over the type's columns, without the keyword.
    /// Reading one vertex is `subject = '…'`.
    #[serde(default)]
    pub r#where: Option<String>,
    /// Column to order by. Absent, the rows arrive in storage order, which is
    /// the writer's Morton order and says nothing the caller asked about.
    #[serde(default)]
    pub order_by: Option<String>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default = "default_read_limit")]
    pub limit: u32,
}

const fn default_read_limit() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReadResult {
    /// Rows as opaque JSON objects so the verb stays generic across arbitrary
    /// vertex shapes. `subject` rides along as the identity; the writer's
    /// layout columns do not — those are the tiles' business, not the algebra's.
    pub rows: Vec<serde_json::Value>,
}

// ──────────────────────────────────────────────────────────────────────────
// expand
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpandParams {
    /// The vertices to expand from, by subject IRI.
    pub from: Vec<String>,
    #[serde(default)]
    pub mode: ExpandMode,
    /// Hops to walk. [`ExpandMode::Into`] ignores it — the induced subgraph has
    /// no frontier to advance.
    #[serde(default = "default_depth")]
    pub depth: u8,
    /// Restrict traversal to a subset of edge names. Empty = all.
    #[serde(default)]
    pub edge_types: Vec<String>,
    #[serde(default = "default_expand_limit")]
    pub limit: u32,
}

/// Which edges an expansion keeps — Neo4j's `Expand(All)` / `Expand(Into)`.
#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum ExpandMode {
    /// Walk outward: every edge leaving the frontier, up to `depth`.
    #[default]
    All,
    /// The subgraph induced on `from`: only edges whose **both** ends are in
    /// the set.
    ///
    /// **The form is right and the speed argument is not available here.**
    /// ADR-0041 justified this mode by the membership mask Kùzu (`SEMI_MASKER`)
    /// and Neo4j push inside the scan; ADR-0042 measured that `DuckDB` does not
    /// accept that class of pruning — a range join against the ids costs more
    /// than not pruning at all. So this is a shape a caller wants, not a fast
    /// path we have.
    Into,
}

const fn default_depth() -> u8 {
    1
}

const fn default_expand_limit() -> u32 {
    500
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExpandResult {
    pub vertices: Vec<GraphVertex>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphVertex {
    pub iri: String,
    pub label: String,
    pub vertex_type: String,
    /// Hop count from the origin set (0 = a vertex the call named).
    pub hop: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub predicate: String,
}

// ──────────────────────────────────────────────────────────────────────────
// path
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathParams {
    pub source_iri: String,
    pub target_iri: String,
    #[serde(default = "default_max_hops")]
    pub max_hops: u8,
}

const fn default_max_hops() -> u8 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PathResult {
    /// Ordered path vertices including endpoints. Empty when no path exists
    /// within `max_hops`.
    pub vertices: Vec<GraphVertex>,
    pub edges: Vec<GraphEdge>,
}
