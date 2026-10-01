//! `fossil.schema.json` is the structs, and not a copy that can drift from
//! them.
//!
//! The file is what a reader outside Rust checks `fossil.json` against —
//! `@fossil-lang/corpus` writes its types by hand and holds them against it in
//! `tests/manifest.test.ts` — so it is checked in, and
//! this test fails when it is stale. `FOSSIL_BLESS=1 cargo test -p
//! fossil-sinks --test schema` rewrites it.

use std::path::Path;

use fossil_sinks::manifest::Manifest;

#[test]
fn the_schema_file_is_the_structs() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fossil.schema.json");
    let want = Manifest::json_schema();
    if std::env::var_os("FOSSIL_BLESS").is_some() {
        std::fs::write(&path, &want).expect("write fossil.schema.json");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "crates/fossil-sinks/fossil.schema.json is stale — run \
         `FOSSIL_BLESS=1 cargo test -p fossil-sinks --test schema` and commit it"
    );
}
