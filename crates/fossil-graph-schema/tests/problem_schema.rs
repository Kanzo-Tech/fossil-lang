//! `problem.schema.json` is `Problem`, and not a copy that can drift from it.
//!
//! The file is what the TypeScript `Code`, `CODES` and `ProblemData` are
//! generated from — titles included, as each variant's `title`, and each
//! `#[error]` as written, as `x-detail` — so it is
//! checked in, and this test fails when it is stale. `FOSSIL_BLESS=1 cargo test
//! -p fossil-graph-schema --test problem_schema` rewrites it.

use std::path::Path;

use fossil_graph_schema::{Problem, TEMPLATES};

#[test]
fn the_schema_file_is_the_enum() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("problem.schema.json");
    let mut schema =
        serde_json::to_value(schemars::schema_for!(Problem)).expect("a derived schema serialises");
    // Each arm also carries its code's `#[error]` as written, as `x-detail`: the
    // one place the TypeScript that renders a detail is generated from.
    for arm in schema["oneOf"].as_array_mut().expect("a top-level oneOf") {
        let code = arm["properties"]["code"]["enum"][0]
            .as_str()
            .expect("one code")
            .to_string();
        let (_, template) = TEMPLATES
            .iter()
            .find(|(c, _)| *c == code)
            .expect("a template per code");
        arm["x-detail"] = serde_json::Value::from(*template);
    }
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
