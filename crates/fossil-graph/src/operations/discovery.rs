//! Discovery verbs — semantic + structural lookups.
//!
//! - `search_by_label` is the **semantic** entry point: vector search over
//!   the writer-emitted `embedding` column (W3). Without W3 the binding
//!   returns `NotImplemented`.
//! - `find_neighbors` walks the `GraphAr` edge tables breadth-first up to
//!   `depth`. Bounded by `limit` so a hub vertex doesn't explode the result.
//! - `find_path` returns the shortest path (BFS over reachable edges).

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// search_by_label
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchByLabelParams {
    pub query: String,
    /// Restrict the vector search to a subset of vertex types. Empty = all.
    #[serde(default)]
    pub vertex_types: Vec<String>,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
}

const fn default_top_k() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchByLabelResult {
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchHit {
    pub iri: String,
    pub label: String,
    pub vertex_type: String,
    /// Cosine similarity in [0, 1].
    pub score: f32,
}

// ──────────────────────────────────────────────────────────────────────────
// find_neighbors
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindNeighborsParams {
    pub iri: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    /// Restrict traversal to a subset of edge names. Empty = all.
    #[serde(default)]
    pub edge_types: Vec<String>,
    #[serde(default = "default_neighbor_limit")]
    pub limit: u32,
}

const fn default_depth() -> u8 {
    1
}

const fn default_neighbor_limit() -> u32 {
    500
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindNeighborsResult {
    pub vertices: Vec<NeighborVertex>,
    pub edges: Vec<NeighborEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NeighborVertex {
    pub iri: String,
    pub label: String,
    pub vertex_type: String,
    /// Hop count from the origin (0 = origin itself).
    pub hop: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NeighborEdge {
    pub source: String,
    pub target: String,
    pub predicate: String,
}

// ──────────────────────────────────────────────────────────────────────────
// find_path
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindPathParams {
    pub source_iri: String,
    pub target_iri: String,
    #[serde(default = "default_max_hops")]
    pub max_hops: u8,
}

const fn default_max_hops() -> u8 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindPathResult {
    /// Ordered path vertices including endpoints. Empty when no path exists
    /// within `max_hops`.
    pub vertices: Vec<NeighborVertex>,
    pub edges: Vec<NeighborEdge>,
}
