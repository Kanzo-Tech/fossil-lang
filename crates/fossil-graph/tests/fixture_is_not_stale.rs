//! **The manifest fixture three TypeScript tests read is a generated file that
//! nothing regenerates.**
//!
//! `examples/dump_fixture.rs` writes `packages/graph/tests/fixtures/manifest.json`,
//! and `packages/graph/tests/{address,client,e2e}.test.ts` read it. Between
//! those two facts there is no script, no CI step and no test: `git log` on that
//! file shows **one** commit, the one that introduced the binding, and the
//! generator has been decorative ever since.
//!
//! What that cost, measured on 2026-08-25 by running the generator and diffing:
//!
//! - `chunk_size: 1024`, from before `c416e07` made a tile 4,096 rows;
//! - **no `vertex_count` and no `edge_count` at all**, from before those fields
//!   existed — and `vertex_count` is the field `resolveCorpus` needs to know how
//!   many tiles a type has, so the TS tests passed by never asking;
//! - `dense_id` marked `is_primary`, which `0b2f9a6` had just settled the other
//!   way.
//!
//! And it did not fail quietly on its own: `address.test.ts` had transcribed
//! `1024` into two assertions, so the stale fixture and the stale constants
//! agreed with each other. A copy of a generated file inside a test is a second
//! spelling of that file, and it is the copy that keeps it green.
//!
//! # What this proves, and what it does not
//!
//! It holds three properties of the committed JSON against the Rust that
//! produces it. Each one would have caught this on its own.
//!
//! It does **not** prove the file equals the generator's output. That check
//! wants the generator callable from a test, and it is an `example`, whose
//! `main` no test can import. Making it importable means the fixture builder
//! moves into the published crate for a test's benefit, which is a bigger
//! change than this defect justifies — and the three properties below are the
//! ones that actually drifted. A fourth kind of drift would still get through,
//! and the honest place to say so is here.

use std::path::Path;

use fossil_sinks::manifest::{DEFAULT_CHUNK_SIZE, VertexInfo};

/// The fixture, as the `{ rel_path: yaml }` map the generator emits.
fn fixture() -> serde_json::Map<String, serde_json::Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/graph/tests/fixtures/manifest.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str::<serde_json::Value>(&text)
        .expect("the fixture is a JSON object of path → YAML")
        .as_object()
        .expect("a JSON object")
        .clone()
}

fn vertex() -> VertexInfo {
    let map = fixture();
    let yaml = map
        .get("vertex/Person.vertex.yml")
        .and_then(serde_json::Value::as_str)
        .expect("the fixture names a Person vertex manifest");
    serde_yaml_ng::from_str(yaml).expect("the fixture's vertex YAML deserializes into VertexInfo")
}

/// The guard is worthless if the file it reads is not the one the tests read.
#[test]
fn the_fixture_is_where_the_typescript_looks_for_it() {
    let map = fixture();
    assert!(
        map.contains_key("graph.graph.yml") && map.contains_key("vertex/Person.vertex.yml"),
        "the fixture does not carry the entry points a reader starts from: {:?}",
        map.keys().collect::<Vec<_>>(),
    );
}

#[test]
fn the_fixture_declares_the_tile_size_the_writer_uses() {
    let person = vertex();
    assert_eq!(
        person.chunk_size, DEFAULT_CHUNK_SIZE,
        "the fixture declares a tile of {} rows and the writer emits {DEFAULT_CHUNK_SIZE}; \
         it was 1024 for months after `c416e07` made a tile 4,096",
        person.chunk_size,
    );
}

#[test]
fn the_fixture_says_how_far_it_goes() {
    // `vertex_count` is `u64` and not `Option<u64>` precisely so a reader may
    // depend on it. A fixture written before the field existed deserializes
    // with serde's default — zero — which is a corpus that declares itself
    // empty, and the TS tests passed over it by never asking how many tiles
    // there are.
    let person = vertex();
    assert!(
        person.vertex_count > 0,
        "the fixture declares vertex_count {}, so `ceil(count / chunk_size)` is zero tiles",
        person.vertex_count,
    );
}

#[test]
fn the_fixture_does_not_mark_the_address_as_the_identity() {
    let person = vertex();
    let primary: Vec<&str> = person
        .property_groups
        .iter()
        .flat_map(|g| g.properties.iter())
        .filter(|p| p.is_primary)
        .map(|p| p.name.as_str())
        .collect();
    assert!(
        !primary.contains(&"dense_id"),
        "the fixture marks `dense_id` primary; it is the ADDRESS, and the layout pass gives it \
         to a different vertex on every relayout — see `identity-is-the-subject`",
    );
    // A type carrying no identity column has no primary, and that is legal.
    // What is not legal is a primary that is not the identity.
    for name in &primary {
        assert_eq!(
            *name, "subject",
            "the fixture marks `{name}` primary, and the identity is `subject`",
        );
    }
}
