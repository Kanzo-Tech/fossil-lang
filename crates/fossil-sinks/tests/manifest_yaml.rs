//! Snapshot lock for the `GraphAr` v1.0.0 manifest YAML shape.
//!
//! These `insta` snapshots freeze the serialized field names + layout so a future regression
//! to the older `graphar_version:`/`vertex_types:` spelling is caught. The
//! field-name guard assertions in `fossil_sinks::manifest` unit tests complement these.

use arrow_schema::DataType;
use fossil_sinks::manifest::{
    Cardinality, CoordinateSystem, DEFAULT_CHUNK_SIZE, EdgeInfo, GRAPHAR_VERSION, HolonRung,
    HolonTree, Projection, Property, Provenance, VertexInfo, data_type_name,
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
    // Two systems, which is what the field is a list FOR, and the pair locks
    // both halves of its spelling: a measured one that writes no `derived_by`,
    // and a derived one that does. A single-system fixture would freeze half.
    .with_coordinates(vec![
        CoordinateSystem::measured("geo", "lon", "lat", Provenance::Geographic),
        CoordinateSystem::derived("layout", "x", "y", "louvain+phyllotaxis"),
    ])
    // The third artefact, and the fixture carries BOTH of its shapes: a rung
    // that publishes a quotient and one that does not. A single-rung fixture
    // would freeze the block without freezing the choice the block exists to
    // leave open.
    .with_holons(holon_tree())
}

/// Two rungs over the same type, finest first — the shape a holon block is
/// emitted in, which is what the snapshot beside this is a lock on. The line
/// scanners over this document read a nested mapping one level deep and skip
/// what is deeper, so the rungs have to nest where they nest and nowhere else.
fn holon_tree() -> HolonTree {
    HolonTree::new(
        DEFAULT_CHUNK_SIZE,
        vec!["knows".to_string()],
        vec![
            HolonRung::at(1, 1_744, holon_columns()).with_quotient(5_012, quotient_columns()),
            HolonRung::at(2, 301, holon_columns()),
        ],
    )
    .with_coordinates(vec![CoordinateSystem::derived(
        "holon",
        "x",
        "y",
        "louvain-cut+member-centroid",
    )])
}

/// A group, its position, its member count, its parent — and the internal
/// weight the edges between its own children were absorbed into. The names are
/// this fixture's; the model reserves none.
fn holon_columns() -> Vec<Property> {
    [
        "holon_id",
        "x",
        "y",
        "member_count",
        "parent",
        "internal_weight",
    ]
    .into_iter()
    .zip(["uint32", "float", "float", "uint32", "uint32", "int64"])
    .map(|(name, data_type)| Property {
        name: name.to_string(),
        data_type: data_type.to_string(),
        is_primary: false,
        is_nullable: Some(false),
        cardinality: Some(Cardinality::Single),
    })
    .collect()
}

/// The pair and its weight — a different arity from a holon row, which is the
/// whole reason it is a different artefact.
fn quotient_columns() -> Vec<Property> {
    ["src_holon", "dst_holon", "weight"]
        .into_iter()
        .zip(["uint32", "uint32", "int64"])
        .map(|(name, data_type)| Property {
            name: name.to_string(),
            data_type: data_type.to_string(),
            is_primary: false,
            is_nullable: Some(false),
            cardinality: Some(Cardinality::Single),
        })
        .collect()
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
