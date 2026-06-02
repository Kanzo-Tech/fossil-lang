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
use fossil_codegen::{codegen_sql, decompose_for_writer};
use fossil_descriptors_output::{AcceptAllDescriptor, OutputDescriptorKind};
use fossil_hir::def_map::def_map;
use fossil_mir::lower_to_mir;
use fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;

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

/// With NO explicit output shape (`AcceptAll`), `decompose_for_writer`
/// synthesises the vertex decomposition from the typed mapping — `hello.fossil`
/// produces a proper `Person` vertex with a `name` property, NOT the legacy flat
/// `_triples` passthrough. This is the "shape lives in fossil" default: the
/// mapping already declares the output, so the host authors no ShEx.
#[test]
fn accept_all_synthesises_typed_vertex_from_mapping() {
    let (db, file) = db_with_hello();
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("hello.fossil must contain at least one mapping");

    let mir = lower_to_mir(&db, mapping);
    let kind = OutputDescriptorKind::AcceptAll(AcceptAllDescriptor);
    let (_prelude, plan) = decompose_for_writer(
        &db,
        mapping,
        mir,
        &kind,
        DEFAULT_CHUNK_SIZE,
        &fossil_codegen::identity_source_uri,
    );

    // One synthesised vertex table for the mapping's subject shape `ex:Person`.
    assert_eq!(plan.vertices.len(), 1, "one synthesised vertex table");
    let v = &plan.vertices[0];
    assert_eq!(v.type_name, "Person", "subject shape local name");
    assert_ne!(v.type_name, "_triples", "must NOT be the flat passthrough");
    assert_eq!(v.vertex_id_col, "iri");

    // The `ex:name = .name` literal property is carried with its column name.
    let name = v
        .properties
        .iter()
        .find(|p| p.name == "name")
        .expect("synthesised `name` property");
    assert!(
        !name.data_type.is_empty(),
        "property carries a GraphAr datatype spelling"
    );
}
