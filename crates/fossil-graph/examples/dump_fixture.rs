//! Emit a minimal GraphAr manifest fixture as `{ rel_path: yaml }` JSON to stdout.
//!
//! Mirrors the in-crate test `fixture()` (one `Person` vertex with `dense_id`/
//! `age`/`name`, one `Person_knows_Person` edge), but reconstructed with the
//! public `fossil_sinks::manifest` API so the `@fossil-lang/graph` binding's
//! WASM smoke test reads a manifest faithful to the writer's emission — no
//! hand-written YAML that could drift from the structs.
//!
//! ```sh
//! cargo run -p fossil-graph --example dump_fixture
//! ```

use fossil_sinks::manifest::{
    DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
};
use serde_json::{Map, Value, json};

fn main() {
    let mut person = VertexInfo::new(
        "Person",
        DEFAULT_CHUNK_SIZE,
        "vertex/Person/",
        vec![PropertyGroup {
            file_type: "parquet".into(),
            properties: vec![
                Property {
                    name: "dense_id".into(),
                    data_type: "uint32".into(),
                    is_primary: true,
                    is_nullable: Some(false),
                },
                Property {
                    name: "age".into(),
                    data_type: "int64".into(),
                    is_primary: false,
                    is_nullable: None,
                },
                Property {
                    name: "name".into(),
                    data_type: "string".into(),
                    is_primary: false,
                    is_nullable: None,
                },
            ],
        }],
    );
    person.iri = "http://example.org/Person".into();

    let edge = EdgeInfo {
        src_type: "Person".into(),
        edge_type: "knows".into(),
        iri: "http://example.org/knows".into(),
        dst_type: "Person".into(),
        chunk_size: DEFAULT_CHUNK_SIZE,
        src_chunk_size: DEFAULT_CHUNK_SIZE,
        dst_chunk_size: DEFAULT_CHUNK_SIZE,
        directed: true,
        prefix: "edge/Person_knows_Person/".into(),
        adj_lists: vec![],
        property_groups: vec![],
        version: "gar/v1".into(),
    };

    let graph = GraphInfo::new(
        "graph",
        "",
        vec!["vertex/Person.vertex.yml".into()],
        vec!["edge/Person_knows_Person/Person_knows_Person.edge.yml".into()],
    );

    let mut files = Map::new();
    files.insert(
        "graph.graph.yml".into(),
        Value::String(graph.to_yaml().unwrap()),
    );
    files.insert(
        "vertex/Person.vertex.yml".into(),
        Value::String(person.to_yaml().unwrap()),
    );
    files.insert(
        "edge/Person_knows_Person/Person_knows_Person.edge.yml".into(),
        Value::String(edge.to_yaml().unwrap()),
    );

    println!("{}", serde_json::to_string_pretty(&json!(files)).unwrap());
}
