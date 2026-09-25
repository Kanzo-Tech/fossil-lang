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
    /// Every vertex type's per-field statistics, on its
    /// [`VertexTypeSummary::stats`] — one batched query per type, the same one
    /// `vertex_type` spends on one. What a host drawing a schema panel asks for
    /// instead of one call per type. Samples are never part of it.
    #[serde(default)]
    pub stats: bool,
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
    /// How many vertices the type has.
    ///
    /// **Not from the manifest, despite what this said.** The executor answers
    /// it with `count_rows` — a `SELECT count(*)` over the type's tiles. The
    /// manifest carries a declared `vertex_count` now, so the query is a second
    /// way to ask one question and can go; what it buys until then is that it
    /// measures the bytes rather than trusting the declaration, which is the
    /// disagreement `apps/corpus`'s `declared-count` guard exists to catch.
    pub count: u64,
    /// Field names — what a follow-up `schema { vertex_type, field }` may name.
    pub fields: Vec<String>,
    /// Per-field statistics for this type, in the order of [`Self::fields`].
    /// **Empty unless the call set `stats`**; with it, empty only for a type
    /// that declares no field.
    pub stats: Vec<FieldStat>,
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
    /// What an axis over the field can do with it — see [`FieldKind`].
    pub kind: FieldKind,
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

/// What kind of VALUE a field holds, read off its `GraphAr` data type alone.
///
/// Orthogonal to [`FieldRole`], which also weighs the name and the cardinality:
/// an `int64` `user_id` is an `Identifier` by role and `Numeric` by kind. The
/// kind is what decides whether `aggregate` may bin the field (`Numeric` and
/// `Temporal` have ranges, `Categorical` groups by value only) and whether an
/// axis is a time axis. It is the one table of `GraphAr` spellings; a reader
/// asks for the kind instead of keeping a copy of it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// An integer or floating-point spelling (`int8` … `uint64`, `float`, `double`).
    Numeric,
    /// `date`, `timestamp` or `time`.
    Temporal,
    /// Everything else — `string`, `bool`, and any spelling this table does not know.
    Categorical,
}

impl FieldKind {
    /// The kind of a `GraphAr` `data_type` spelling. Case-sensitive, as the
    /// writer emits it.
    #[must_use]
    pub fn of(datatype: &str) -> Self {
        match datatype {
            "int8" | "int16" | "int32" | "int64" | "uint8" | "uint16" | "uint32" | "uint64"
            | "float" | "double" => Self::Numeric,
            "date" | "timestamp" | "time" => Self::Temporal,
            _ => Self::Categorical,
        }
    }

    /// Whether the field has ranges to bin over.
    #[must_use]
    pub const fn is_binnable(self) -> bool {
        matches!(self, Self::Numeric | Self::Temporal)
    }
}
