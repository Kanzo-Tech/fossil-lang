//! `fossil.schema.json` is the structs, and not a copy that can drift from
//! them: what a reader outside Rust checks `fossil.json` against.
//! `UPDATE_EXPECT=1 cargo test -p fossil-sinks --test schema` rewrites it.

use fossil_sinks::manifest::Manifest;

#[test]
fn the_schema_file_is_the_structs() {
    expect_test::expect_file!["../fossil.schema.json"].assert_eq(&Manifest::json_schema());
}
