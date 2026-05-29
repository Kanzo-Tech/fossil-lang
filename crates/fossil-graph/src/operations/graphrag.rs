//! `GraphRAG` verbs — `summarize_cluster`, `answer_with_communities`.
//!
//! Hierarchical retrieval: instead of retrieving raw triples, the LLM gets
//! pre-summarised cluster context (cheaper, denser) and cites cluster IDs.
//!
//! These verbs depend on writer W3 emitting `cluster_id: u32` and
//! `cluster_summary: TEXT` on the vertex Parquet. Without W3 the bindings
//! must return [`crate::GraphError::NotImplemented`] — there is no in-line
//! fallback because computing summaries on-demand would defeat the cost
//! model `GraphRAG` sells.

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// summarize_cluster
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummarizeClusterParams {
    pub cluster_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SummarizeClusterResult {
    /// Writer-emitted summary text.
    pub summary: String,
    /// Cluster size for caller context.
    pub vertex_count: u64,
    /// Dominant vertex type (mode of `type_idx` inside the cluster).
    pub dominant_type: String,
}

// ──────────────────────────────────────────────────────────────────────────
// answer_with_communities
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerWithCommunitiesParams {
    pub question: String,
    /// Cap on how many cluster summaries the LLM may pull as context. The
    /// Microsoft `GraphRAG` paper recommends 5–10; defaulting low to match.
    #[serde(default = "default_max_communities")]
    pub max_communities: u8,
}

const fn default_max_communities() -> u8 {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AnswerWithCommunitiesResult {
    pub answer: String,
    /// Clusters the answer was derived from. The binding renders these as
    /// "sources" / citations.
    pub cited_clusters: Vec<u32>,
}
