//! **The walking skeleton**: `examples/hello.fossil` through the executor
//! produces a readable corpus — five `Person` vertices, asserted by CONTENT.
//!
//! The program runs where a reader of the file would run it: `examples/` is
//! served from memory and the program is compiled at its URL there, so
//! `users.csv` and `hello.shex` resolve beside it. The corpus is read back with
//! `DuckDB`, never through the writer's own types.
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
    let corpus = native::run_dir(&examples, "hello.fossil", &[]).expect("hello.fossil runs");

    // The manifest: one vertex table, `Person` — the local name of the shape's
    // IRI, never the mapping's own name — carrying the program's `name`.
    let manifest = corpus.manifest();
    assert_eq!(manifest.vertex_tables.len(), 1);
    let person = &manifest.vertex_tables[0];
    assert_eq!(person.name, "Person");
    assert_eq!(person.path, "vertex/Person.parquet");
    assert_eq!(person.record_count, 5);
    for column in ["dense_id", "subject", "name"] {
        assert!(
            person.properties.iter().any(|p| p.name == column),
            "`{column}` is declared: {:?}",
            person.properties
        );
    }

    // The bytes.
    let root = corpus.materialise();
    assert_eq!(
        native::column(root.path(), &person.path, "subject"),
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
        native::column(root.path(), &person.path, "name"),
        ["Alice", "Bob", "Carol", "Dave", "Eve"],
        "the `name` column carries the five names from users.csv",
    );
}
