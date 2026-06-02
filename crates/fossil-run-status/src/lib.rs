//! The `fossil run --output-json` wire contract.
//!
//! `fossil run --dest <url> --output-json` emits one [`RunStatus`] object on
//! stdout describing the `GraphAr` dataset it just wrote: per vertex type its
//! Parquet file + row count + property columns; per edge type its CSR/CSC file
//! pair + endpoints + count. A host (keasy) deserializes it to persist the job's
//! output structure WITHOUT re-introspecting the dataset.
//!
//! This crate is the SINGLE source of truth for that shape: the CLI serializes
//! [`RunStatus`], host consumers deserialize the same struct (depend on this
//! crate — it is dependency-light + WASM-clean, NOT the execution library), and
//! the `JsonSchema` derives publish the contract for TypeScript codegen (the
//! same pipeline `fossil-graph` uses for its verb result types).
//!
//! ## Stats boundary
//!
//! [`RunStatus`] carries STRUCTURE only — no column-value statistics
//! (`n_unique`/`min`/`max`/`samples`). The host's browser data plane computes
//! those on demand via DuckDB-WASM over the mounted Parquet. (Host-boundary: the
//! server ships structure, the browser owns stats.)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The status object `fossil run --output-json` writes to stdout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct RunStatus {
    /// Destination URL the `GraphAr` dataset was written under (echoes `--dest`).
    pub dest: String,
    /// One entry per emitted vertex type.
    pub vertices: Vec<VertexStatus>,
    /// One entry per emitted edge type (empty until a shape/Phase-B adds edges).
    pub edges: Vec<EdgeStatus>,
}

/// One vertex type — its dataset-relative Parquet, row count, and property
/// columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct VertexStatus {
    /// The vertex type / `GraphAr` `type` (e.g. `Person`).
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// Dataset-relative Parquet path, e.g. `vertex/Person.parquet`.
    pub file: String,
    /// Row count (Parquet footer metadata); `None` if the count query failed.
    pub count: Option<i64>,
    /// The vertex's property columns.
    pub columns: Vec<ColumnStatus>,
}

/// A property column of a [`VertexStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ColumnStatus {
    /// Column / predicate local name.
    pub name: String,
    /// `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
    pub data_type: String,
}

/// One edge type — its CSR/CSC Parquet pair and endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct EdgeStatus {
    /// The edge type / predicate local name.
    pub edge_type: String,
    /// Source vertex type.
    pub src_type: String,
    /// Destination vertex type.
    pub dst_type: String,
    /// CSR-ordered (`by_source`) Parquet, dataset-relative.
    pub by_source: String,
    /// CSC-ordered (`by_target`) Parquet, dataset-relative.
    pub by_target: String,
    /// Edge count (Parquet footer metadata); `None` if the count query failed.
    pub count: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_wire_contract() {
        let json = r#"{
            "dest": "s3://bucket/job",
            "vertices": [
                { "type": "Person", "file": "vertex/Person.parquet", "count": 5,
                  "columns": [ { "name": "name", "data_type": "string" } ] }
            ],
            "edges": [
                { "edge_type": "knows", "src_type": "Person", "dst_type": "Person",
                  "by_source": "edge/k/by_source.parquet",
                  "by_target": "edge/k/by_target.parquet", "count": 2 }
            ]
        }"#;
        let parsed: RunStatus = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.vertices[0].vertex_type, "Person");
        assert_eq!(parsed.vertices[0].count, Some(5));
        assert_eq!(parsed.vertices[0].columns[0].name, "name");
        assert_eq!(parsed.edges[0].edge_type, "knows");
        // `type` rename round-trips on serialize.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"type\":\"Person\""), "rename lost: {back}");
    }
}
