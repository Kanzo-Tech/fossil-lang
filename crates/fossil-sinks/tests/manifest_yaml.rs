//! Snapshot lock for the `GraphAr` v1.0.0 manifest YAML shape.
//!
//! These `insta` snapshots freeze the serialized field names + layout so a future regression
//! to the older `graphar_version:`/`vertex_types:` spelling is caught. The
//! field-name guard assertions in `fossil_sinks::manifest` unit tests complement these.

use arrow_schema::DataType;
use fossil_sinks::generated::{CELL_COLUMNS, QUOTIENT_COLUMNS};
use fossil_sinks::manifest::{
    Cardinality, CellRung, CellTree, CoordinateSystem, DEFAULT_CHUNK_SIZE,
    DEFAULT_VERTICES_PER_CELL, EdgeInfo, Projection, Property, Provenance, VertexInfo,
    data_type_name, declared_properties,
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
    .with_cells(cell_tree())
}

/// Two rungs over the same type, finest first — the shape a cell block is
/// emitted in, which is what the snapshot beside this is a lock on. The line
/// scanners over this document read a nested mapping one level deep and skip
/// what is deeper, so the rungs have to nest where they nest and nowhere else.
///
/// **`new` and not `planned`**, because what this locks is the document's
/// spelling and not the arithmetic. A planned tree runs every rung down to one
/// cell, which would make the snapshot a table of `div_ceil` rather than a
/// picture of the block, and it publishes no quotient — where this fixture
/// deliberately carries both shapes, one rung with and one without.
/// `CellTree::planned`'s own unit tests are where the counts are checked.
fn cell_tree() -> CellTree {
    CellTree::new(
        DEFAULT_VERTICES_PER_CELL,
        vec!["knows".to_string()],
        vec![
            CellRung::at(1, 625, cell_columns()).with_quotient(5_012, quotient_columns()),
            CellRung::at(2, 157, cell_columns()),
        ],
    )
    .with_coordinates(vec![CoordinateSystem::derived(
        "cell",
        "x",
        "y",
        "cell-member-centroid",
    )])
}

/// The columns of a cell row, **off the generated table** rather than spelled
/// here.
///
/// This fixture used to name them itself, under a comment saying the model
/// reserved no spelling for any of them — which was true, and stopped being
/// true when `corpus.bnf` grew the set. A snapshot that locked a spelling
/// nothing writes would lock the wrong document.
fn cell_columns() -> Vec<Property> {
    declared_properties(CELL_COLUMNS)
}

/// The pair and its weight — a different arity from a cell row, which is the
/// whole reason it is a different artefact.
fn quotient_columns() -> Vec<Property> {
    declared_properties(QUOTIENT_COLUMNS)
}

fn knows_edge() -> EdgeInfo {
    EdgeInfo::new(
        "Person",
        "knows",
        "Person",
        19_998,
        DEFAULT_CHUNK_SIZE,
        "edge/person_knows_person/",
        vec![
            Projection::payload("by_source/", endpoints()).aligned_by("src", true),
            Projection::payload("by_target/", endpoints()).aligned_by("dst", true),
        ],
    )
    .with_cardinality(Cardinality::Multi)
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
