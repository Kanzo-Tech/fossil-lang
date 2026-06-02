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
/// mapping already declares the output, so the host authors no `ShEx`.
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

/// A two-mapping program whose `Order` mapping references `Person` by reusing
/// `Person`'s subject template as the value of `ex:placedBy` — the foreign key
/// the synthesised descriptor turns into an edge (Phase B, HOST-BOUNDARY §4).
/// `ex:external` reuses a template no mapping emits → a dangling IRI, no edge.
const EDGES_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")
orders := io.csv(\"orders.csv\")

Person : ex:Person from users
    iri = `${ex:}person/${.id}`
    ex:name = .name

Order : ex:Order from orders
    iri = `${ex:}order/${.order_id}`
    ex:placedBy = `${ex:}person/${.user_id}`
    ex:total = .amount
    ex:external = `${ex:}widget/${.wid}`
";

fn decompose_at(db: &FossilDb, file: SourceFile, idx: usize) -> fossil_sinks::decomp::SinkPlan {
    let mapping = *def_map(db, file)
        .mappings(db)
        .get(idx)
        .expect("mapping index in range");
    let mir = lower_to_mir(db, mapping);
    let kind = OutputDescriptorKind::AcceptAll(AcceptAllDescriptor);
    decompose_for_writer(
        db,
        mapping,
        mir,
        &kind,
        DEFAULT_CHUNK_SIZE,
        &fossil_codegen::identity_source_uri,
    )
    .1
}

/// The `Order` mapping's IRI-template `ex:placedBy` resolves to the `Person`
/// vertex (same subject skeleton) and becomes an edge; the literal `ex:total`
/// stays a property; the dangling `ex:external` produces no edge.
#[test]
fn accept_all_synthesises_edge_from_iri_template_foreign_key() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, EDGES_FOSSIL.to_string(), "edges.fossil".to_string());

    let order = decompose_at(&db, file, 1);
    assert_eq!(order.vertices.len(), 1, "one Order vertex");
    assert_eq!(order.vertices[0].type_name, "Order");

    // `ex:total` literal stays a property; the two IRI templates do not.
    let prop_names: Vec<&str> = order.vertices[0]
        .properties
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        prop_names,
        ["total"],
        "only the literal property is a column"
    );

    assert_eq!(
        order.edges.len(),
        1,
        "placedBy → Person edge only (external is dangling)"
    );
    let edge = &order.edges[0];
    assert_eq!(edge.src_type, "Order");
    assert_eq!(edge.predicate, "placedBy");
    assert_eq!(edge.dst_type, "Person");
    assert_eq!(edge.src_id_expr, "iri");
    assert_eq!(edge.dst_id_expr, "placedBy");
    assert!(
        edge.single_valued,
        "synthesised edges are single-valued (D-A)"
    );
}

/// A mapping with only literal properties (the `Person` head) synthesises no
/// edges — the Phase-A behaviour is preserved when no FK template is present.
#[test]
fn accept_all_literal_only_mapping_has_no_edges() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, EDGES_FOSSIL.to_string(), "edges.fossil".to_string());

    let person = decompose_at(&db, file, 0);
    assert_eq!(person.vertices[0].type_name, "Person");
    assert!(person.edges.is_empty(), "literal-only mapping has no edges");
}
