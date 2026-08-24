//! The `fossil run --output-json` wire contract, beside the code that fills it.
//!
//! `fossil run --dest <url> --output-json` emits one [`RunStatus`] object on
//! stdout describing the `GraphAr` dataset it just wrote: per vertex type its
//! Parquet file + row count + property columns; per edge type its CSR/CSC file
//! pair + endpoints + count. A host deserializes it to persist the job's output
//! structure WITHOUT re-introspecting the dataset.
//!
//! [`crate::GraphArData::run_status`] is the only thing that builds one, which
//! is why the shape lives here and not in a crate of its own: the wire contract
//! is what this backend says about what it wrote.
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
pub struct VertexStatus {
    /// The vertex type / `GraphAr` `type` (e.g. `Person`).
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// The full RDF type IRI the mapping declares for this vertex (the shape IRI,
    /// e.g. `https://example.org/Person`); `None` when the output has no RDF
    /// type. This is the output *spec* fossil owns — the governance layer (DCAT)
    /// reads it from the manifest instead of re-deriving it from the program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdf_type: Option<String>,
    /// **Where this vertex type's rows are**, dataset-relative — and a host
    /// fetches it, so it names something that exists.
    ///
    /// Two spellings, and which one you get says whether the dataset has been
    /// tiled: a single Parquet (`vertex/Person.parquet`) from a producer that
    /// wrote one, and the chunk PREFIX (`vertex/Person/`, holding
    /// `chunk{k}.parquet` — the same string the `GraphAr` `VertexInfo`
    /// declares) once a layout pass has replaced it with chunks.
    ///
    /// It used to be the single Parquet unconditionally, and native
    /// `fossil run` deletes that file: the layout pass reads it, writes the
    /// chunks, and removes it as the last thing it does, so the status went out
    /// naming a path that had just stopped existing. The browser executor runs
    /// no layout pass, which is why the same field was truthful there and
    /// nobody noticed.
    pub file: String,
    /// Row count (Parquet footer metadata); `None` if the count query failed.
    pub count: Option<i64>,
    /// The vertex's property columns.
    pub columns: Vec<ColumnStatus>,
}

/// A property column of a [`VertexStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColumnStatus {
    /// Column / predicate local name.
    pub name: String,
    /// `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
    pub data_type: String,
    /// The full RDF predicate IRI this column maps (e.g.
    /// `https://example.org/name`); `None` when the output has no RDF predicate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdf_uri: Option<String>,
    /// The XSD datatype IRI of the literal value (e.g.
    /// `http://www.w3.org/2001/XMLSchema#string`); `None` when not an RDF
    /// literal. Part of the output spec the governance layer (DCAT) consumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xsd_datatype: Option<String>,
}

/// One edge type — its CSR/CSC Parquet pair and endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    use super::RunStatus;

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
