//! **The walking skeleton**: `examples/hello.fossil` through the executor
//! produces a readable `GraphAr` graph — five `Person` vertices, asserted by
//! CONTENT.
//!
//! The program runs where a reader of the file would run it: `examples/` is
//! served from memory and the program is compiled at its URL there, so
//! `users.csv` and `hello.shex` resolve beside it. The graph is read back with
//! `graphar-info` and `DuckDB`, never through the writer's own types.
//!
//! Content and not existence: any regression that silently changes the
//! produced graph — wrong template substitution, a dropped property, an
//! off-by-one row drop — is caught here. A refactor that breaks it for more
//! than three days is reverted and broken into smaller steps (`CLAUDE.md`,
//! "Hard Rules").
//!
//! **The shape document is not optional and its absence is silent.** A
//! property key is the last segment of a predicate IRI the shape declares, so a
//! run that never registered `hello.shex` writes five `Person`s with no `name`
//! column and no error — the name assertions below are what would catch it.

use std::path::Path;

#[path = "support/native.rs"]
mod native;
mod support;

#[test]
fn walking_skeleton_writes_5_person_vertices_with_expected_content() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let ran = native::run_dir(&examples, "hello.fossil", &[]).expect("hello.fossil runs");

    // The graph: one vertex type, `Person` — the local name of the shape's IRI,
    // never the mapping's own name — in one property group, `subject` first and
    // the key, then the program's `name`.
    let graph = ran.graph();
    assert_eq!(graph.vertices.len(), 1);
    assert!(graph.edges.is_empty());
    let person = &graph.vertices[0];
    assert_eq!(person.r#type, "Person");
    assert_eq!(ran.vertex_count("Person"), 5);
    let properties: Vec<(&str, bool)> = person.property_groups[0]
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.is_primary))
        .collect();
    assert_eq!(properties, [("subject", true), ("name", false)]);

    // The bytes: the chunk the information file names, read back by `DuckDB`
    // and never through the writer's own types.
    let root = ran.materialise();
    let chunk = ran.chunk("Person");
    assert_eq!(chunk, "vertex/Person/subject_name/chunk0");
    assert_eq!(
        native::column(root.path(), &chunk, "subject"),
        [
            "https://example.org/user/1",
            "https://example.org/user/2",
            "https://example.org/user/3",
            "https://example.org/user/4",
            "https://example.org/user/5",
        ],
        "the subjects are the five expanded user IRIs",
    );
    assert_eq!(
        native::column(root.path(), &chunk, "name"),
        ["Alice", "Bob", "Carol", "Dave", "Eve"],
        "the `name` column carries the five names from users.csv",
    );
    assert_eq!(
        native::column(root.path(), &chunk, "_graphArVertexIndex"),
        ["0", "1", "2", "3", "4"],
        "the index is the row, from 0",
    );
}
