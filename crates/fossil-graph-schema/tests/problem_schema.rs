//! `problem.schema.json` is `Problem`, and not a copy that can drift from it.
//!
//! The file is what the TypeScript `Code`, `CODES` and `ProblemData` are
//! generated from — titles included, as each variant's `title` — so it is
//! checked in, and this test fails when it is stale. `FOSSIL_BLESS=1 cargo test
//! -p fossil-graph-schema --test problem_schema` rewrites it.

use std::path::Path;

use fossil_graph_schema::Problem;

#[test]
fn the_schema_file_is_the_enum() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("problem.schema.json");
    let schema = schemars::schema_for!(Problem);
    let mut want = serde_json::to_string_pretty(&schema).expect("a derived schema serialises");
    want.push('\n');
    if std::env::var_os("FOSSIL_BLESS").is_some() {
        std::fs::write(&path, &want).expect("write problem.schema.json");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "crates/fossil-graph-schema/problem.schema.json is stale — run \
         `FOSSIL_BLESS=1 cargo test -p fossil-graph-schema --test problem_schema` and commit it"
    );
}
