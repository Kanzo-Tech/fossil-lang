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
//! Fossil never byte-writes Parquet from Rust (ADR-0017) — the runtime materializes tiles via
//! `DuckDB` `COPY ... (FORMAT PARQUET)` into the manifest-declared `prefix`. The vertex-tile naming
//! convention is `<prefix>chunk{k}.parquet` (ADR-0016, `RESEARCH` Open Q1) and the edge-tile one is
//! `<prefix>by_source/tile{k}.parquet`. This module declares the tiling; it does not emit bytes,
//! and `fossil-runtime`'s `enrich_layout` is the only thing that does — **what the emitter writes
//! is what the manifest says**, asserted on the artefact by
//! `fossil-engine/tests/conformance.rs` rather than agreed by convention. ADR-0041 is the record of
//! that gap being open for a long time; it does not get to reopen.

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
    /// Rows per tile (configurable; default [`DEFAULT_CHUNK_SIZE`]). Tile `k` is
    /// the `dense_id` range `[k·chunk_size, (k+1)·chunk_size)` and a power of
    /// two, so a reader addresses it with [`tile_of`] rather than a division.
    pub chunk_size: u64,
    /// Output path prefix for this vertex's tiles, e.g. `"vertex/person/"`.
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
    /// The addressing unit of an edge tile, equal to [`Self::src_chunk_size`].
    ///
    /// **Not a row count**, and it never was one for edges: an edge lives in its
    /// source's tile (CSR — ADR-0042 §3.2), so tile `k` under
    /// `<prefix>by_source/` holds every edge whose `src_dense` is in vertex tile
    /// `k` and its row count is the degree of those 4,096 vertices. The
    /// alternative — the deepest tile containing both endpoints — was measured
    /// and is dominated on both curves: 2.29× → 15.86× over-read against CSR's
    /// flat 1.95× → 2.89×, and 2.5–3.5× the tiles.
    pub chunk_size: u64,
    /// Source-vertex tile size (must equal the source [`VertexInfo::chunk_size`]:
    /// an edge tile is addressed by the source's tile, so a different number
    /// here would address nothing).
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
    /// — the entries are already `<dest>`-relative `rel_path`s.
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

/// How many bits a `dense_id` is shifted right by to name the tile holding it.
///
/// Published as arithmetic and not as prose, because that is the difference
/// between an implementation somebody can copy and one they have to re-derive
/// (ADR-0045, «Decidido el 2026-08-05» §4 — Iceberg publishes Murmur3 with a
/// vector table and every port agrees; `PMTiles` links Wikipedia for its Hilbert
/// curve and every port differs).
///
/// **The operands, spelled out.** The input is an unsigned 64-bit `dense_id`,
/// the shift is logical, and the result is an unsigned 64-bit tile number. Not
/// pedantry: `>>` is arithmetic on a signed type in Rust, and in JavaScript it
/// truncates to 32 bits before shifting, so the same three characters mean
/// three different things across the layers that have to agree. The border
/// vectors a re-implementation is checked against are in this module's tests.
pub const TILE_SHIFT: u32 = 12;

/// The tile a `dense_id` lives in — the whole of the addressing scheme.
///
/// There is no tile tree and nothing to discover: tile `i` **is** the range
/// `[i·4096, (i+1)·4096)`, its parent is a further shift, and the lowest common
/// ancestor of two vertices is the common prefix of their ids. A reader computes
/// every URL it wants before it emits the first request, which is the whole
/// content of *the camera is addressed, not queried* (ADR-0042 §3.3).
#[must_use]
pub const fn tile_of(dense_id: u64) -> u64 {
    dense_id >> TILE_SHIFT
}

/// Rows per tile when a mapping does not override it — `1 << TILE_SHIFT`.
///
/// **A tile is a fixed 4,096-row `dense_id` range**, and both lines of reasoning
/// that reach that number arrived independently (ADR-0042 §3.1, ADR-0045 §8).
/// Measured on the five-million corpus served over a plain HTTP origin, counting
/// every request that answers, against an ideal payload of 0.6–0.9 MB per window
/// that is flat in N:
///
/// | `chunk_size` | requests | bytes | vs ideal |
/// |---|---|---|---|
/// | 1,024 | 178 | 1.11 MB | 1.73× |
/// | **4,096** | **78** | **1.48 MB** | **2.31×** |
/// | 8,192 | 47 | 1.89 MB | 2.95× |
/// | 32,768 | 25 | 4.61 MB | 7.20× |
/// | 122,880 | 35 | 11.73 MB | 18.3× |
///
/// The two curves have no common optimum — bytes bottom out at 1,024–2,048 and
/// requests fall monotonically — so what chooses is `λ·β`, the bytes a link
/// moves in the latency of one request: 32,768 in series, 8,192 with six in
/// flight, **4,096 fully multiplexed**, which is what an addressed reader is.
///
/// **And the byte curve is flat from 1,024 to 8,192, so the emitter is not tuned
/// inside that band.** A change there is not an improvement, it is noise with a
/// `git blame` on it.
///
/// It was 122,880 — `DuckDB`'s default `ROW_GROUP_SIZE`, chosen when a chunk was
/// thought to be a row group and measured only in milliseconds over localhost,
/// where a request costs nothing. It is **Pareto-dominated by 32,768 on both
/// curves at once** (more requests *and* four times the bytes, because a
/// 122,880-row edge tile crosses several row groups and costs 9.4 requests), and
/// it is not a power of two, which forces a division where a shift does.
pub const DEFAULT_CHUNK_SIZE: u64 = 1 << TILE_SHIFT;

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

    /// The border vectors of [`tile_of`], which are the deliverable.
    ///
    /// A second implementation — the wasm reader, the `TypeScript`/`DuckDB` path,
    /// or a stranger's — is checked against this table and not against a
    /// sentence. Every value here is a border: the first id, the last id of tile
    /// 0, the first of tile 1, and the three places where a 32-bit reading of the
    /// shift diverges from a 64-bit one. `2^31` and `2^53` exceed the `u32` a
    /// `dense_id` column holds *today*, and they are here precisely for that
    /// reason: they are where a port that took the shift as signed, or that ran
    /// it through a JavaScript `number`, gives a different answer.
    #[test]
    fn tile_of_border_vectors() {
        for (dense_id, tile) in [
            (0u64, 0u64),
            (4_095, 0),
            (4_096, 1),
            (8_191, 1),
            (2_147_483_647, 524_287),                   // 2^31 − 1
            (2_147_483_648, 524_288),                   // 2^31
            (9_007_199_254_740_992, 2_199_023_255_552), // 2^53
        ] {
            assert_eq!(tile_of(dense_id), tile, "tile_of({dense_id})");
        }
        // The shift and the row count are one statement, not two that agree.
        assert_eq!(DEFAULT_CHUNK_SIZE, 4_096);
        assert_eq!(tile_of(DEFAULT_CHUNK_SIZE - 1), 0);
        assert_eq!(tile_of(DEFAULT_CHUNK_SIZE), 1);
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
        // Asserted against the constant, not a literal: the value is a measured trade-off
        // (see DEFAULT_CHUNK_SIZE) and this test is about the spec *spelling* of the key.
        assert!(
            yaml.contains(&format!("chunk_size: {DEFAULT_CHUNK_SIZE}")),
            "{yaml}"
        );
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
