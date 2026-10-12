//! A spec emits the program it describes: each spec here is a conformance program's
//! (`docs/programs/<name>/<name>.fossil`) written as data, and what [`fossil_mapping::emit`] writes
//! from it is that program — token for token, its comments and its layout aside. The programs
//! themselves are held by the conformance suite (`crates/fossil-df/tests/programs.rs`), so a
//! spec that emits one is a spec whose program checks, runs and writes the corpus it expects.

use std::path::Path;

use fossil_mapping::{Mapping, emit};

/// The program without its comments, one space wherever there was any.
fn tokens(program: &str) -> String {
    program
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

fn round_trip(name: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec: Mapping = serde_json::from_str(
        &std::fs::read_to_string(root.join(format!("tests/specs/{name}.json"))).expect("the spec"),
    )
    .expect("the spec is a Mapping");
    let program =
        std::fs::read_to_string(root.join(format!("../../docs/programs/{name}/{name}.fossil")))
            .expect("the program");
    let emitted = emit(&spec).expect("the spec emits");
    assert_eq!(tokens(&emitted), tokens(&program), "emitted:\n{emitted}");
}

#[test]
fn hello_is_its_spec() {
    round_trip("hello");
}

#[test]
fn relationship_is_its_spec_joins_and_links_included() {
    round_trip("relationship");
}

#[test]
fn rename_is_its_spec_the_attribute_included() {
    round_trip("rename");
}
