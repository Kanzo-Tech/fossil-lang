//! Emit a minimal `GraphAr` manifest fixture as `{ rel_path: yaml }` JSON to stdout.
//!
//! Mirrors the in-crate test `fixture()` (one `Person` vertex with `dense_id`/
//! `age`/`name`, one `Person_knows_Person` edge), but reconstructed with the
//! public `fossil_sinks::manifest` API so the `@fossil-lang/graph` binding's
//! WASM smoke test reads a manifest whose SERIALISATION is the writer's — no
//! hand-written YAML that could drift from the structs.
//!
//! **That is the whole of the fidelity, and the word used to be "faithful to
//! the writer's emission", which reads like more.** The field list here is not
//! the writer's: `fossil_df::vertex_info` emits `dense_id`, `subject`, the
//! program's own properties, then `x`, `y` and `cluster_id`, and this emits
//! `dense_id`, `age`, `name`. It is a minimal shape for a binding's smoke test,
//! and a reader who took the old sentence at its word would conclude fossil
//! writes vertices with no identity column. What the structs guarantee is that
//! the YAML is spelled the way the writer spells it; what it contains is a
//! fixture's business.
//!
//! ```sh
//! cargo run -p fossil-graph --example dump_fixture
//! ```

use fossil_sinks::manifest::{
    Container, DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
};
use serde_json::{Map, Value, json};

fn main() {
    let mut person = VertexInfo::new(
        "Person",
        3,
        DEFAULT_CHUNK_SIZE,
        "vertex/Person/",
        vec![PropertyGroup {
            file_type: "parquet".into(),
            properties: vec![
                Property {
                    name: "dense_id".into(),
                    data_type: "uint32".into(),
                    // Never the primary: it is the ADDRESS, and the layout pass
                    // gives it away on every relayout. This type carries no
                    // identity column at all, so it has no primary — which is a
                    // legal corpus, and what `identity-is-the-subject` reports
                    // without failing on.
                    is_primary: false,
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
        edge_count: 2,
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
        Container::RowGroups,
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
