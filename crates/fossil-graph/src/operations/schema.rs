//! Schema introspection verbs — `list_vertex_types`, `list_edge_types`,
//! `describe_field`.
//!
//! No SQL execution required — pure manifest reads. Cheap, side-effect-free,
//! safe for any binding to expose to any caller.

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────
// list_vertex_types
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListVertexTypesParams {}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ListVertexTypesResult {
    pub types: Vec<VertexTypeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VertexTypeSummary {
    /// Short local name as used in `DuckDB` table identifier (e.g. `"Person"`).
    pub name: String,
    /// Full RDF type IRI.
    pub iri: String,
    /// Vertex count from the manifest.
    pub count: u64,
    /// Field names for downstream calls to `describe_field`.
    pub fields: Vec<String>,
}

// ──────────────────────────────────────────────────────────────────────────
// list_edge_types
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEdgeTypesParams {}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ListEdgeTypesResult {
    pub edges: Vec<EdgeTypeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EdgeTypeSummary {
    pub source_type: String,
    pub name: String,
    pub target_type: String,
    pub iri: String,
    pub count: u64,
    /// Convenience: `DuckDB` view name `{source}_{name}_{target}`. Pre-computed
    /// here so bindings don't reimplement the `GraphAr` edge naming convention.
    pub table_name: String,
}

// ──────────────────────────────────────────────────────────────────────────
// describe_field
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeFieldParams {
    pub vertex_type: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DescribeFieldResult {
    pub datatype: String,
    /// Distinct value count when known from manifest stats.
    pub distinct: Option<u64>,
    /// Up to 8 sample values surfaced by the writer.
    pub samples: Vec<String>,
    /// Inferred role for chart-axis defaults: `identifier`, `dimension`,
    /// `measure`. Mirrors keasy `lib/graph-schema.ts::inferRole` — promoted
    /// here to be authoritative.
    pub role: FieldRole,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldRole {
    Identifier,
    Dimension,
    Measure,
}

// ──────────────────────────────────────────────────────────────────────────
// describe_vertex_type — batched per-type field stats + roles (one SQL query).
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeVertexTypeParams {
    pub vertex_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DescribeVertexTypeResult {
    /// Total row count of the vertex table (`COUNT(*)`), the denominator role
    /// inference uses for the cardinality test.
    pub count: u64,
    /// Every user-facing field (reserved columns filtered), in manifest order,
    /// with authoritative role + cardinality. One batched query computes all of
    /// it — the single source for what keasy used to derive client-side.
    pub fields: Vec<FieldStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FieldStat {
    pub name: String,
    /// `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
    pub datatype: String,
    /// Distinct value count (`COUNT(DISTINCT field)`).
    pub distinct: u64,
    /// Authoritative chart-axis role.
    pub role: FieldRole,
}
