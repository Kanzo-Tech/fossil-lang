//! `GraphAr` manifest reader.
//!
//! The writer side (`fossil-sinks::manifest`) is the source of truth for the
//! `GraphInfo` / `VertexInfo` / `EdgeInfo` shapes — this module parses the
//! YAML those structs serialise into and exposes lookup helpers used by every
//! verb. Same structs round-trip write→read; no parallel "`GraphArReader`"
//! that drifts from the writer's emission
//! ([[`feedback_no_duplicate_logic_across_crates`]] at the crate boundary).
//!
//! Entry point is the [`GraphInfo`] aggregate index (`graph.graph.yml`): in a
//! serverless/httpfs model a reader cannot list a directory, so it fetches the
//! one index file and resolves the per-type manifests it references.

use std::collections::HashMap;

use fossil_sinks::manifest::{EdgeInfo, GraphInfo, VertexInfo};

use crate::{GraphError, Result};

/// Dataset-relative location of the `GraphAr` aggregate index, as emitted by
/// `fossil_sinks::writer::plan_manifests`.
pub const GRAPH_INFO_PATH: &str = "graph.graph.yml";

/// Writer-emitted (non-user) vertex columns. Schema verbs hide these from
/// field listings so callers don't chart on `dense_id` or `x`/`y`.
pub const RESERVED_VERTEX_COLUMNS: [&str; 5] = ["dense_id", "subject", "x", "y", "cluster_id"];

/// Abstraction over fetching a manifest file by its dataset-relative path.
///
/// The three hosts differ only here: native reads the filesystem, the browser
/// reads via httpfs/OPFS, tests read an in-memory map. Everything above this
/// trait is host-agnostic, so the verb logic compiles identically to WASM.
pub trait ManifestSource {
    /// Fetch the bytes of a dataset-relative file (e.g. `graph.graph.yml`,
    /// `vertex/Person.vertex.yml`).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::InvalidManifest`] when the path cannot be read.
    fn fetch(&self, rel_path: &str) -> Result<Vec<u8>>;
}

/// Parsed `GraphAr` manifest: the aggregate [`GraphInfo`] plus every resolved
/// vertex/edge info, indexed for O(1) verb lookups.
#[derive(Debug, Clone)]
pub struct Manifest {
    graph: GraphInfo,
    vertices: Vec<VertexInfo>,
    edges: Vec<EdgeInfo>,
    /// Vertex type name → index into `vertices`. The index doubles as the
    /// `type_idx` ordinal the viewport verb emits (stable, manifest order).
    vertex_idx: HashMap<String, usize>,
    /// Edge table name (`{src}_{edge}_{dst}`) → index into `edges`.
    edge_idx: HashMap<String, usize>,
}

impl Manifest {
    /// Load the manifest from `source`, starting at [`GRAPH_INFO_PATH`].
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::InvalidManifest`] when the index or any
    /// referenced per-type manifest cannot be fetched or parsed.
    pub fn load(source: &impl ManifestSource) -> Result<Self> {
        Self::load_from(source, GRAPH_INFO_PATH)
    }

    /// Load the manifest from `source`, starting at an explicit index path.
    ///
    /// # Errors
    ///
    /// As [`Manifest::load`].
    pub fn load_from(source: &impl ManifestSource, graph_path: &str) -> Result<Self> {
        let graph: GraphInfo = parse_yaml(&source.fetch(graph_path)?, graph_path)?;

        let mut vertices = Vec::with_capacity(graph.vertices.len());
        let mut vertex_idx = HashMap::with_capacity(graph.vertices.len());
        for rel in &graph.vertices {
            let info: VertexInfo = parse_yaml(&source.fetch(rel)?, rel)?;
            vertex_idx.insert(info.vertex_type.clone(), vertices.len());
            vertices.push(info);
        }

        let mut edges = Vec::with_capacity(graph.edges.len());
        let mut edge_idx = HashMap::with_capacity(graph.edges.len());
        for rel in &graph.edges {
            let info: EdgeInfo = parse_yaml(&source.fetch(rel)?, rel)?;
            edge_idx.insert(edge_table_name(&info), edges.len());
            edges.push(info);
        }

        Ok(Self {
            graph,
            vertices,
            edges,
            vertex_idx,
            edge_idx,
        })
    }

    /// The aggregate index this manifest was built from.
    #[must_use]
    pub const fn graph(&self) -> &GraphInfo {
        &self.graph
    }

    /// All vertex infos, in manifest (`type_idx`) order.
    #[must_use]
    pub fn vertices(&self) -> &[VertexInfo] {
        &self.vertices
    }

    /// All edge infos, in manifest order.
    #[must_use]
    pub fn edges(&self) -> &[EdgeInfo] {
        &self.edges
    }

    /// Look up a vertex type by its short name (the `DuckDB` view identifier).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownEntity`] when the name is not in the manifest.
    pub fn lookup_vertex(&self, name: &str) -> Result<&VertexInfo> {
        self.vertex_idx
            .get(name)
            .map(|&i| &self.vertices[i])
            .ok_or_else(|| GraphError::UnknownEntity {
                kind: "vertex type",
                name: name.to_string(),
            })
    }

    /// The `type_idx` ordinal for a vertex type (its manifest position), as
    /// emitted on the viewport verb's vertices. `None` if the type is unknown
    /// or the manifest holds more than 256 types (the `u8` ceiling).
    #[must_use]
    pub fn vertex_type_idx(&self, name: &str) -> Option<u8> {
        self.vertex_idx
            .get(name)
            .and_then(|&i| u8::try_from(i).ok())
    }

    /// Look up an edge by its table name (`{src}_{edge}_{dst}`).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownEntity`] when the table name is not in the
    /// manifest.
    pub fn lookup_edge(&self, table_name: &str) -> Result<&EdgeInfo> {
        self.edge_idx
            .get(table_name)
            .map(|&i| &self.edges[i])
            .ok_or_else(|| GraphError::UnknownEntity {
                kind: "edge type",
                name: table_name.to_string(),
            })
    }

    /// User-facing field names of a vertex type, in manifest order, with the
    /// writer-emitted [`RESERVED_VERTEX_COLUMNS`] filtered out.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownEntity`] when the vertex type is unknown.
    pub fn vertex_fields(&self, name: &str) -> Result<Vec<String>> {
        let info = self.lookup_vertex(name)?;
        Ok(info
            .property_groups
            .iter()
            .flat_map(|g| g.properties.iter())
            .map(|p| p.name.as_str())
            .filter(|n| !RESERVED_VERTEX_COLUMNS.contains(n))
            .map(ToString::to_string)
            .collect())
    }
}

/// The `DuckDB` view name `GraphAr` assigns an edge: `{src}_{edge}_{dst}`.
/// Single source for the naming convention shared by the writer's edge dir,
/// the keasy view registration, and verb SQL.
#[must_use]
pub fn edge_table_name(e: &EdgeInfo) -> String {
    format!("{}_{}_{}", e.src_type, e.edge_type, e.dst_type)
}

fn parse_yaml<T: serde::de::DeserializeOwned>(bytes: &[u8], path: &str) -> Result<T> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| GraphError::InvalidManifest(format!("{path}: not UTF-8: {e}")))?;
    serde_yaml_ng::from_str(text).map_err(|e| GraphError::InvalidManifest(format!("{path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_sinks::manifest::{
        AdjList, DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
    };

    /// In-memory manifest source: the test analogue of httpfs/fs. Proves the
    /// reader is host-agnostic above the [`ManifestSource`] seam.
    struct MapSource(HashMap<String, Vec<u8>>);

    impl ManifestSource for MapSource {
        fn fetch(&self, rel_path: &str) -> Result<Vec<u8>> {
            self.0
                .get(rel_path)
                .cloned()
                .ok_or_else(|| GraphError::InvalidManifest(format!("missing {rel_path}")))
        }
    }

    fn vinfo(name: &str, user_fields: &[&str]) -> VertexInfo {
        let mut properties = vec![
            Property {
                name: "dense_id".into(),
                data_type: "uint32".into(),
                is_primary: true,
                is_nullable: Some(false),
            },
            Property {
                name: "subject".into(),
                data_type: "string".into(),
                is_primary: false,
                is_nullable: Some(false),
            },
        ];
        for f in user_fields {
            properties.push(Property {
                name: (*f).to_string(),
                data_type: "string".into(),
                is_primary: false,
                is_nullable: None,
            });
        }
        for layout in ["x", "y", "cluster_id"] {
            properties.push(Property {
                name: layout.to_string(),
                data_type: "uint32".into(),
                is_primary: false,
                is_nullable: Some(false),
            });
        }
        VertexInfo::new(
            name,
            DEFAULT_CHUNK_SIZE,
            format!("vertex/{name}/"),
            vec![PropertyGroup {
                file_type: "parquet".into(),
                properties,
            }],
        )
    }

    fn einfo(src: &str, edge: &str, dst: &str) -> EdgeInfo {
        EdgeInfo {
            src_type: src.into(),
            edge_type: edge.into(),
            iri: String::new(),
            dst_type: dst.into(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            src_chunk_size: DEFAULT_CHUNK_SIZE,
            dst_chunk_size: DEFAULT_CHUNK_SIZE,
            directed: true,
            prefix: format!("edge/{src}_{edge}_{dst}/"),
            adj_lists: vec![AdjList {
                ordered: true,
                aligned_by: "src".into(),
                file_type: "parquet".into(),
            }],
            property_groups: vec![],
            version: "gar/v1".into(),
        }
    }

    /// Two vertex types + one edge, serialised to YAML and indexed by `rel_path`
    /// exactly as the writer would lay them out under a dataset root.
    fn fixture() -> MapSource {
        let person = vinfo("Person", &["name", "age"]);
        let org = vinfo("Org", &["title"]);
        let works_at = einfo("Person", "works_at", "Org");
        let graph = GraphInfo::new(
            "graph",
            "",
            vec![
                "vertex/Person.vertex.yml".into(),
                "vertex/Org.vertex.yml".into(),
            ],
            vec!["edge/Person_works_at_Org/Person_works_at_Org.edge.yml".into()],
        );

        let mut map = HashMap::new();
        map.insert(
            GRAPH_INFO_PATH.into(),
            graph.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "vertex/Person.vertex.yml".into(),
            person.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "vertex/Org.vertex.yml".into(),
            org.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "edge/Person_works_at_Org/Person_works_at_Org.edge.yml".into(),
            works_at.to_yaml().unwrap().into_bytes(),
        );
        MapSource(map)
    }

    #[test]
    fn loads_index_and_resolves_every_type() {
        let m = Manifest::load(&fixture()).expect("load");
        assert_eq!(m.vertices().len(), 2);
        assert_eq!(m.edges().len(), 1);
        assert_eq!(m.lookup_vertex("Person").unwrap().vertex_type, "Person");
        assert_eq!(
            m.lookup_edge("Person_works_at_Org").unwrap().edge_type,
            "works_at"
        );
    }

    #[test]
    fn type_idx_follows_manifest_order() {
        let m = Manifest::load(&fixture()).expect("load");
        assert_eq!(m.vertex_type_idx("Person"), Some(0));
        assert_eq!(m.vertex_type_idx("Org"), Some(1));
        assert_eq!(m.vertex_type_idx("Nope"), None);
    }

    #[test]
    fn vertex_fields_hides_reserved_columns() {
        let m = Manifest::load(&fixture()).expect("load");
        // dense_id/subject/x/y/cluster_id filtered; only user fields remain.
        assert_eq!(m.vertex_fields("Person").unwrap(), vec!["name", "age"]);
    }

    #[test]
    fn unknown_vertex_is_a_typed_error() {
        let m = Manifest::load(&fixture()).expect("load");
        let err = m.lookup_vertex("Ghost").unwrap_err();
        assert!(matches!(
            err,
            GraphError::UnknownEntity {
                kind: "vertex type",
                ..
            }
        ));
    }

    #[test]
    fn missing_index_file_surfaces_invalid_manifest() {
        let empty = MapSource(HashMap::new());
        let err = Manifest::load(&empty).unwrap_err();
        assert!(matches!(err, GraphError::InvalidManifest(_)));
    }
}
