//! The introspection verb — `schema`.
//!
//! One verb answers what four used to: the vertex types, the edge types, and —
//! on request — a type's per-field statistics. **The cheap/expensive line is a
//! parameter, not a second verb.**
//!
//! - Bare `schema`: the manifest plus one `count(*)` per table, which `DuckDB`
//!   answers from the Parquet footer. No per-field query, ever.
//! - `schema { vertex_type }`: adds ONE batched query (`count(*)` + a
//!   `count(DISTINCT …)` per field) for that type's fields.
//! - `schema { vertex_type, field }`: narrows to that field and adds one more
//!   query for its samples. **Samples cost a query, so they arrive only when a
//!   field is named** — see [`FieldStat::samples`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaParams {
    /// Name a vertex type to also get its per-field statistics. Omitted, the
    /// answer is the type lists alone and no field is queried.
    #[serde(default)]
    pub vertex_type: Option<String>,
    /// Name a field to narrow the statistics to it and pick up its samples.
    /// Ignored without `vertex_type`.
    #[serde(default)]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SchemaResult {
    pub vertices: Vec<VertexTypeSummary>,
    pub edges: Vec<EdgeTypeSummary>,
    /// Per-field statistics for the named `vertex_type`, narrowed to `field`
    /// when one was named. **Empty when no `vertex_type` was named** — that is
    /// the whole of the cheap/expensive distinction.
    pub fields: Vec<FieldStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VertexTypeSummary {
    /// Short local name as used in `DuckDB` table identifier (e.g. `"Person"`).
    pub name: String,
    /// Full RDF type IRI.
    pub iri: String,
    /// Vertex count from the manifest.
    pub count: u64,
    /// Field names — what a follow-up `schema { vertex_type, field }` may name.
    pub fields: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FieldStat {
    pub name: String,
    /// `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
    pub datatype: String,
    /// Distinct value count (`COUNT(DISTINCT field)`).
    pub distinct: u64,
    /// Authoritative chart-axis role.
    pub role: FieldRole,
    /// Up to 8 non-null values. **Populated only when the call named this
    /// field**: they are a second query, and a bare per-type call would pay it
    /// once per column.
    pub samples: Vec<String>,
}

/// Inferred role for chart-axis defaults. Mirrors keasy `lib/graph-schema.ts::
/// inferRole` — promoted here to be authoritative.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldRole {
    Identifier,
    Dimension,
    Measure,
}
