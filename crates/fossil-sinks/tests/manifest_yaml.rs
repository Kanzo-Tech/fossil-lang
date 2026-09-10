//! Snapshot lock for the `GraphAr` v1.0.0 manifest YAML shape.
//!
//! These `insta` snapshots freeze the serialized field names + layout so a future regression
//! to the older `graphar_version:`/`vertex_types:` spelling is caught. The
//! field-name guard assertions in `fossil_sinks::manifest` unit tests complement these.

use arrow_schema::DataType;
use fossil_sinks::manifest::{
    Cardinality, DEFAULT_CHUNK_SIZE, EdgeInfo, GRAPHAR_VERSION, Projection, Property, VertexInfo,
    data_type_name,
};

fn person_vertex() -> VertexInfo {
    VertexInfo::new(
        "Person",
        10_000,
        DEFAULT_CHUNK_SIZE,
        "vertex/person/",
        vec![Projection::payload(
            "",
            vec![
                Property {
                    name: "id".to_string(),
                    data_type: data_type_name(&DataType::Int64),
                    is_primary: true,
                    is_nullable: Some(false),
                    cardinality: Some(Cardinality::Single),
                },
                Property {
                    name: "name".to_string(),
                    data_type: data_type_name(&DataType::Utf8),
                    is_primary: false,
                    is_nullable: None,
                    cardinality: None,
                },
            ],
        )],
    )
}

fn knows_edge() -> EdgeInfo {
    EdgeInfo {
        src_type: "Person".to_string(),
        edge_type: "knows".to_string(),
        iri: String::new(),
        dst_type: "Person".to_string(),
        cardinality: Some(Cardinality::Multi),
        edge_count: 19_998,
        chunk_size: DEFAULT_CHUNK_SIZE,
        src_chunk_size: DEFAULT_CHUNK_SIZE,
        dst_chunk_size: DEFAULT_CHUNK_SIZE,
        directed: true,
        prefix: "edge/person_knows_person/".to_string(),
        projections: vec![
            Projection::payload("by_source/", endpoints()).aligned_by("src", true),
            Projection::payload("by_target/", endpoints()).aligned_by("dst", true),
        ],
        version: GRAPHAR_VERSION.to_string(),
    }
}

/// The two columns every adjacency tile carries — the pair that IS the edge.
fn endpoints() -> Vec<Property> {
    ["src_dense", "dst_dense"]
        .into_iter()
        .map(|name| Property {
            name: name.to_string(),
            data_type: "uint32".to_string(),
            is_primary: false,
            is_nullable: Some(false),
            cardinality: Some(Cardinality::Single),
        })
        .collect()
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
