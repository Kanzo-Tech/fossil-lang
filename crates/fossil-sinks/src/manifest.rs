//! `GraphAr` v1.0.0 manifest structs + `serde_yaml_ng` emission (SINK-02, ADR-0016).
//!
//! These structs serialize to the **`GraphAr` v1.0.0** vertex-info / edge-info YAML field
//! names — `version: gar/v1`, `type`, `chunk_size`, `prefix`, `property_groups`, and edge
//! `src_type`/`dst_type`/`adj_lists`. This deliberately supersedes the Phase-1 hand-templated
//! spelling (`graphar_version: 1.0.0`, `vertex_types:`, `data_type: string`), which conformed
//! to no `GraphAr` reader (see ADR-0016 + `RESEARCH` Pitfall 2).
//!
//! The structs are plain serializable data — no `Box<dyn Trait>`, safe to pass through Salsa
//! queries (CLAUDE.md hard rule). `data_type` strings are derived from [`arrow_schema::DataType`]
//! via [`data_type_name`], the single authority for the spec spellings (`int64`, `string`, ...).
//!
//! Fossil never byte-writes Parquet from Rust (ADR-0017) — the runtime materializes chunks via
//! `DuckDB` `COPY ... (FORMAT PARQUET)` into the manifest-declared `prefix`. The chunk-file naming
//! convention is `<prefix>chunk{k}.parquet` (ADR-0016, `RESEARCH` Open Q1); the actual chunked COPY
//! emission lands in plan 05-08. This module declares `chunk_size` and the layout; it does not
//! emit bytes.

use arrow_schema::DataType;
use serde::{Deserialize, Serialize};

/// The `GraphAr` manifest format version string. Emitted as `version: gar/v1`.
pub const GRAPHAR_VERSION: &str = "gar/v1";

/// `GraphAr` vertex-info manifest (one per vertex/shape type).
///
/// Serializes with the spec field names; `vertex_type` renames to `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexInfo {
    /// Shape label, e.g. `"Person"`. Emitted as the spec key `type`.
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// Full RDF type IRI (empty for non-RDF graphs). Carried into the manifest
    /// so the query side's schema verbs surface it without a separate registry
    /// ([[`feedback_no_duplicate_logic_across_crates`]]). Omitted from YAML when
    /// empty so non-RDF graphs keep the canonical `GraphAr` shape.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub iri: String,
    /// Rows per Parquet chunk (configurable; default [`DEFAULT_CHUNK_SIZE`]).
    pub chunk_size: u64,
    /// Output path prefix for this vertex's chunks, e.g. `"vertex/person/"`.
    pub prefix: String,
    /// Property groups (column groupings → one file per group per chunk).
    pub property_groups: Vec<PropertyGroup>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// `GraphAr` edge-info manifest (one per `(src_type, edge_type, dst_type)` triple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeInfo {
    /// Source vertex type label.
    pub src_type: String,
    /// Edge type label (the relationship name).
    pub edge_type: String,
    /// Full predicate IRI (empty for non-RDF graphs). Omitted from YAML when
    /// empty. See [`VertexInfo::iri`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub iri: String,
    /// Destination vertex type label.
    pub dst_type: String,
    /// Rows per edge chunk.
    pub chunk_size: u64,
    /// Source-vertex chunk size (must align with the source [`VertexInfo::chunk_size`]).
    pub src_chunk_size: u64,
    /// Destination-vertex chunk size (must align with the destination vertex).
    pub dst_chunk_size: u64,
    /// Whether the edge is directed.
    pub directed: bool,
    /// Output path prefix, e.g. `"edge/person_knows_person/"`.
    pub prefix: String,
    /// Adjacency-list orderings provided for this edge.
    pub adj_lists: Vec<AdjList>,
    /// Property groups carried on the edge.
    pub property_groups: Vec<PropertyGroup>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// `GraphAr` top-level **graph info** (`<name>.graph.yml`) — the aggregate
/// index that references every vertex-info and edge-info file in the graph.
///
/// Required for serverless consumption: an httpfs reader (fossil-graph,
/// DuckDB-WASM) cannot list a directory over HTTP, so the graph info is the
/// single entry point a binding fetches to discover all types and their
/// per-type YAML paths. This supersedes keasy's server-built `DataManifest`
/// (the query side now reads the same artifact the writer emits — single
/// source, [[`feedback_no_duplicate_logic_across_crates`]]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphInfo {
    /// Graph label, e.g. `"graph"`. Emitted as the spec key `name`.
    pub name: String,
    /// Prefix the `vertices`/`edges` entries are relative to. Usually `""`
    /// — the entries are already `<dest>`-relative rel_paths.
    pub prefix: String,
    /// Relative paths to each vertex-info YAML, e.g. `vertex/Person.vertex.yml`.
    pub vertices: Vec<String>,
    /// Relative paths to each edge-info YAML, e.g.
    /// `edge/Person_knows_Person/Person_knows_Person.edge.yml`.
    pub edges: Vec<String>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// A group of properties stored together in one file type per chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyGroup {
    /// Storage file type for this group, e.g. `"parquet"`.
    pub file_type: String,
    /// The properties (columns) in this group.
    pub properties: Vec<Property>,
}

/// A single property (column) of a vertex or edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    /// Property (column) name.
    pub name: String,
    /// `GraphAr` data-type spelling (`int64`, `string`, `double`, `bool`, ...), derived from
    /// an [`arrow_schema::DataType`] via [`data_type_name`].
    pub data_type: String,
    /// Whether this property is (part of) the primary key.
    pub is_primary: bool,
    /// Nullability, omitted from the YAML when `None` (`GraphAr` treats it as optional).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_nullable: Option<bool>,
}

/// An adjacency-list ordering descriptor for an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjList {
    /// Whether the adjacency list is sorted.
    pub ordered: bool,
    /// Which endpoint the list is aligned by, e.g. `"src"` or `"dst"`.
    pub aligned_by: String,
    /// Storage file type, e.g. `"parquet"`.
    pub file_type: String,
}

/// Default rows-per-chunk when a mapping does not override it. The manifest carries the value;
/// the runtime (05-08) materializes the chunks. `1024` per `RESEARCH` §"`GraphAr` Sink".
pub const DEFAULT_CHUNK_SIZE: u64 = 1024;

impl VertexInfo {
    /// Construct a `VertexInfo` with the [`GRAPHAR_VERSION`] preset.
    #[must_use]
    pub fn new(
        vertex_type: impl Into<String>,
        chunk_size: u64,
        prefix: impl Into<String>,
        property_groups: Vec<PropertyGroup>,
    ) -> Self {
        Self {
            vertex_type: vertex_type.into(),
            iri: String::new(),
            chunk_size,
            prefix: prefix.into(),
            property_groups,
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// Serialize this vertex-info to `GraphAr` v1.0.0 YAML.
    ///
    /// # Errors
    /// Returns the underlying `serde_yaml_ng` error if serialization fails (it cannot, for
    /// these plain structs, but the signature is honest).
    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }
}

impl EdgeInfo {
    /// Serialize this edge-info to `GraphAr` v1.0.0 YAML.
    ///
    /// # Errors
    /// Returns the underlying `serde_yaml_ng` error if serialization fails.
    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }
}

impl GraphInfo {
    /// Construct a `GraphInfo` with the [`GRAPHAR_VERSION`] preset.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        prefix: impl Into<String>,
        vertices: Vec<String>,
        edges: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            prefix: prefix.into(),
            vertices,
            edges,
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// Serialize this graph-info to `GraphAr` v1.0.0 YAML.
    ///
    /// # Errors
    /// Returns the underlying `serde_yaml_ng` error if serialization fails.
    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }
}

/// Map an [`arrow_schema::DataType`] to its `GraphAr` `data_type` string spelling.
///
/// `arrow-schema` is the single authority for the spellings (`RESEARCH` §"Don't Hand-Roll"):
/// the manifest's declared types must match what `DuckDB` COPY actually writes. Unhandled
/// arrow types fall back to `binary` (the `GraphAr` catch-all for opaque columns).
#[must_use]
pub fn data_type_name(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "bool",
        DataType::Int8 | DataType::Int16 | DataType::Int32 => "int32",
        DataType::Int64 => "int64",
        DataType::Float16 | DataType::Float32 => "float",
        DataType::Float64 => "double",
        DataType::Utf8 | DataType::LargeUtf8 => "string",
        DataType::Date32 | DataType::Date64 => "date",
        DataType::Timestamp(_, _) => "timestamp",
        DataType::Time32(_) | DataType::Time64(_) => "time",
        _ => "binary",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person_vertex() -> VertexInfo {
        VertexInfo::new(
            "Person",
            DEFAULT_CHUNK_SIZE,
            "vertex/person/",
            vec![PropertyGroup {
                file_type: "parquet".to_string(),
                properties: vec![
                    Property {
                        name: "id".to_string(),
                        data_type: data_type_name(&DataType::Int64),
                        is_primary: true,
                        is_nullable: Some(false),
                    },
                    Property {
                        name: "name".to_string(),
                        data_type: data_type_name(&DataType::Utf8),
                        is_primary: false,
                        is_nullable: None,
                    },
                ],
            }],
        )
    }

    fn knows_edge() -> EdgeInfo {
        EdgeInfo {
            src_type: "Person".to_string(),
            edge_type: "knows".to_string(),
            iri: String::new(),
            dst_type: "Person".to_string(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            src_chunk_size: DEFAULT_CHUNK_SIZE,
            dst_chunk_size: DEFAULT_CHUNK_SIZE,
            directed: true,
            prefix: "edge/person_knows_person/".to_string(),
            adj_lists: vec![AdjList {
                ordered: true,
                aligned_by: "src".to_string(),
                file_type: "parquet".to_string(),
            }],
            property_groups: vec![],
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    #[test]
    fn data_type_name_uses_spec_spellings() {
        assert_eq!(data_type_name(&DataType::Int64), "int64");
        assert_eq!(data_type_name(&DataType::Utf8), "string");
        assert_eq!(data_type_name(&DataType::Float64), "double");
        assert_eq!(data_type_name(&DataType::Boolean), "bool");
    }

    #[test]
    fn vertex_yaml_carries_graphar_v1_field_names() {
        let yaml = person_vertex().to_yaml().expect("vertex serialization");
        // Pitfall-2 field-name guard: spec spellings present, NOT the Phase-1 template.
        assert!(yaml.contains("version: gar/v1"), "{yaml}");
        assert!(yaml.contains("type: Person"), "{yaml}");
        assert!(yaml.contains("chunk_size: 1024"), "{yaml}");
        assert!(yaml.contains("prefix: vertex/person/"), "{yaml}");
        assert!(yaml.contains("property_groups:"), "{yaml}");
        assert!(yaml.contains("data_type: int64"), "{yaml}");
        assert!(yaml.contains("is_primary: true"), "{yaml}");
        // The Phase-1 spelling must NOT appear.
        assert!(!yaml.contains("graphar_version"), "{yaml}");
        assert!(!yaml.contains("vertex_types"), "{yaml}");
    }

    #[test]
    fn vertex_yaml_skips_none_nullable_but_keeps_some() {
        let yaml = person_vertex().to_yaml().expect("vertex serialization");
        // id has Some(false) → emitted; name has None → skipped.
        assert!(yaml.contains("is_nullable: false"), "{yaml}");
        // Exactly one occurrence (only id), proving skip_serializing_if works for name.
        assert_eq!(yaml.matches("is_nullable").count(), 1, "{yaml}");
    }

    #[test]
    fn vertex_info_round_trips_through_yaml() {
        // The query side (fossil-graph) deserialises the same structs the
        // writer serialises — single source, no parallel reader structs.
        let original = person_vertex();
        let yaml = original.to_yaml().expect("serialize");
        let parsed: VertexInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(original, parsed);
    }

    #[test]
    fn edge_info_round_trips_through_yaml() {
        let original = knows_edge();
        let yaml = original.to_yaml().expect("serialize");
        let parsed: EdgeInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(original, parsed);
    }

    #[test]
    fn edge_yaml_carries_graphar_v1_field_names() {
        let yaml = knows_edge().to_yaml().expect("edge serialization");
        assert!(yaml.contains("src_type: Person"), "{yaml}");
        assert!(yaml.contains("dst_type: Person"), "{yaml}");
        assert!(yaml.contains("edge_type: knows"), "{yaml}");
        assert!(yaml.contains("adj_lists:"), "{yaml}");
        assert!(yaml.contains("directed: true"), "{yaml}");
        assert!(yaml.contains("version: gar/v1"), "{yaml}");
    }
}
