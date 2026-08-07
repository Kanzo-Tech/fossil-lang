//! E2E: an expression in a property position produces its column.
//!
//! This is the counterpart of the test that measured the hole. Until
//! 2026-08-07, `ex:slug = clean.slug(.name)` was dropped between the CST and
//! the HIR: the compiler reported success and the column was simply not in the
//! corpus. What is asserted here is the whole chain closing — the form in the
//! HIR, the arm in the checker, the lowering to MIR, and the render on this
//! engine — by the only evidence that cannot be faked: the values.
//!
//! `cargo test`'s cwd is the crate root, so the source is
//! `tests/fixtures/users.csv`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_hir::def_map::def_map;

/// `clean.slug` is a UDF: no engine ships it, so this exercises the fossil-side
/// implementation as well as the plumbing. `clean.upper` is a builtin, so the
/// same program covers both halves of `LoweringKind`.
const CALLS: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:slug = clean.slug(.name)
    ex:shout = clean.upper(.name)
";

#[tokio::test]
async fn a_call_produces_its_column() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, CALLS.to_string(), "calls.fossil".to_string());
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");

    let ctx = SessionContext::new();
    let (vertex, node) = fossil_df::execute_vertex(
        &ctx,
        &db,
        mapping,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .expect("execute_vertex runs the DataFusion plan");

    let props: Vec<&str> = node.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        props,
        ["slug", "shout"],
        "both computed properties reach the graph schema"
    );

    let batch = vertex.batches.first().expect("at least one RecordBatch");
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        [
            "dense_id",
            "subject",
            "slug",
            "shout",
            "x",
            "y",
            "cluster_id"
        ],
        "the call's column is in the materialised shape"
    );

    let slug = column::<StringArray>(batch, 2);
    let slugs: Vec<&str> = (0..slug.len()).map(|i| slug.value(i)).collect();
    assert_eq!(
        slugs,
        ["alice", "bob", "carol"],
        "the UDF ran: these are slugs, not the input"
    );

    let shout = column::<StringArray>(batch, 3);
    let shouts: Vec<&str> = (0..shout.len()).map(|i| shout.value(i)).collect();
    assert_eq!(shouts, ["ALICE", "BOB", "CAROL"], "the builtin ran too");
}

/// A nested call types and runs: the argument of one function is another.
#[tokio::test]
async fn calls_nest() {
    const NESTED: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:tag = clean.slug(clean.upper(.name))
";
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, NESTED.to_string(), "nested.fossil".to_string());
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");

    let ctx = SessionContext::new();
    let (vertex, _) = fossil_df::execute_vertex(
        &ctx,
        &db,
        mapping,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .expect("execute_vertex runs the nested plan");

    let batch = vertex.batches.first().expect("at least one RecordBatch");
    let tag = column::<StringArray>(batch, 2);
    let tags: Vec<&str> = (0..tag.len()).map(|i| tag.value(i)).collect();
    assert_eq!(tags, ["alice", "bob", "carol"], "upper then slug");
}

fn column<A: Array + 'static>(
    batch: &datafusion::arrow::record_batch::RecordBatch,
    i: usize,
) -> &A {
    batch
        .column(i)
        .as_any()
        .downcast_ref::<A>()
        .expect("column has the expected array type")
}

/// A comparison produces a boolean column, and its integer literal survives.
///
/// F2 §2: `.id >= 2` is the shape a filter predicate has, and until it lowered,
/// `Expr::BinOp` and `Expr::LitBool` were constructed by nothing and the
/// backend answered `unimplemented!()` for both.
#[tokio::test]
async fn a_comparison_produces_a_boolean_column() {
    const COMPARES: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:senior = .id >= 2
";
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, COMPARES.to_string(), "cmp.fossil".to_string());
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");

    let ctx = SessionContext::new();
    let (vertex, node) = fossil_df::execute_vertex(
        &ctx,
        &db,
        mapping,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .expect("execute_vertex runs the comparison plan");

    assert_eq!(node.properties[0].name, "senior");
    assert_eq!(
        node.properties[0].datatype,
        fossil_graph_schema::Primitive::Bool,
        "the column's declared datatype is the operator's, not the row's"
    );

    let batch = vertex.batches.first().expect("at least one RecordBatch");
    let senior = column::<datafusion::arrow::array::BooleanArray>(batch, 2);
    let values: Vec<bool> = (0..senior.len()).map(|i| senior.value(i)).collect();
    assert_eq!(
        values,
        [false, true, true],
        "ids 1,2,3 against `>= 2` — the literal reached the plan"
    );
}
