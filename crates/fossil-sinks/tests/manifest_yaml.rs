//! Snapshot lock for the `GraphAr` v1.0.0 manifest YAML shape.
//!
//! These `insta` snapshots freeze the serialized field names + layout so a future regression
//! to the older `graphar_version:`/`vertex_types:` spelling is caught. The
//! field-name guard assertions in `fossil_sinks::manifest` unit tests complement these.

use arrow_schema::DataType;
use fossil_sinks::manifest::{
    AdjList, DEFAULT_CHUNK_SIZE, EdgeInfo, GRAPHAR_VERSION, Property, PropertyGroup, VertexInfo,
    data_type_name,
};

fn person_vertex() -> VertexInfo {
    VertexInfo::new(
        "Person",
        10_000,
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
        edge_count: 19_998,
        chunk_size: DEFAULT_CHUNK_SIZE,
        src_chunk_size: DEFAULT_CHUNK_SIZE,
        dst_chunk_size: DEFAULT_CHUNK_SIZE,
        directed: true,
        prefix: "edge/person_knows_person/".to_string(),
        adj_lists: vec![
            AdjList {
                ordered: true,
                aligned_by: "src".to_string(),
                prefix: "by_source/".to_string(),
                file_type: "parquet".to_string(),
            },
            AdjList {
                ordered: true,
                aligned_by: "dst".to_string(),
                prefix: "by_target/".to_string(),
                file_type: "parquet".to_string(),
            },
        ],
        property_groups: vec![],
        version: GRAPHAR_VERSION.to_string(),
    }
}

#[test]
fn vertex_person_yaml_snapshot() {
    insta::assert_snapshot!(
        "vertex_person",
        person_vertex().to_yaml().expect("vertex serialization")
    );
}

#[test]
fn edge_person_knows_person_yaml_snapshot() {
    insta::assert_snapshot!(
        "edge_person_knows_person",
        knows_edge().to_yaml().expect("edge serialization")
    );
}
