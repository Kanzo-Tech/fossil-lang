//! Phase 1 success criterion #5 (ROADMAP) — snapshot test pinning the SQL +
//! manifest output of `examples/hello.fossil`.
//!
//! The hello.fossil source is inlined as a const (matches the convention used
//! by `fossil-hir` and `fossil-mir` unit tests; avoids a working-directory
//! dependency on an external example file that doesn't exist on disk yet —
//! Phase 5 STDL-06 lands the on-disk `examples/` corpus).
//!
//! Snapshots live in `tests/snapshots/`. CI runs `cargo insta test
//! --review-mode=ci` which fails on diff. Per RESEARCH.md Risk 5, never accept
//! snapshots blindly — always compare against the locked output documented in
//! RESEARCH.md Example 13 (the SQL block) and Example 4 (the manifest block).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_codegen::codegen_sql;
use fossil_hir::def_map::def_map;

const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

fn db_with_hello() -> (FossilDb, SourceFile) {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(
        &db,
        HELLO_FOSSIL.to_string(),
        "examples/hello.fossil".to_string(),
    );
    (db, file)
}

#[test]
fn compile_hello_fossil_produces_expected_sql() {
    let (db, file) = db_with_hello();
    let dm = def_map(&db, file);
    let mapping = *dm
        .mappings(&db)
        .first()
        .expect("hello.fossil must contain at least one mapping");

    let plan = codegen_sql(&db, mapping);

    insta::assert_snapshot!("hello_sql", plan.sql(&db));
    insta::assert_snapshot!("hello_manifest", plan.manifest_yaml(&db));
}
