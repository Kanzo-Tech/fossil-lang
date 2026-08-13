//! E2E: an expression in a property position produces its column.
//!
//! This is the counterpart of the test that measured the hole. Until
//! 2026-08-07, `ex:slug = str.slug(.name)` was dropped between the CST and
//! the HIR: the compiler reported success and the column was simply not in the
//! corpus. What is asserted here is the whole chain closing — the form in the
//! HIR, the arm in the checker, the lowering to MIR, and the render on this
//! engine — by the only evidence that cannot be faked: the values.
//!
//! `cargo test`'s cwd is the crate root, so the source is
//! `tests/fixtures/users.csv`.

#![cfg(not(target_arch = "wasm32"))]
// `${ex:}user/${.id}` is Fossil template syntax, not a Rust format arg.
#![allow(clippy::literal_string_with_formatting_args)]

use datafusion::arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;
use fossil_hir::def_map::def_map;

mod support;

const EXPR_SHEX: &str = include_str!("fixtures/expressions.shex");

/// `str.slug` is a multi-call template (`trim(regexp_replace(lower(trim(…))))`)
/// and `str.upper` is a single call, so one program covers both the nested and
/// the flat shape of a `LoweringKind::Expr`. `str.slug` used to be a native
/// Rust UDF this engine could not run at all — ruling 15 made it a template,
/// and this test is where that becomes visible as values.
const CALLS: &str = "\
prefix ex: <https://example.org/>
type { Person } = io.shex(\"expr.shex\")

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    @subject = `${ex:}user/${.id}`
    slug = str.slug(.name)
    shout = str.upper(.name)
";

#[tokio::test]
async fn a_call_produces_its_column() {
    let (db, file) = support::db_with_shapes(CALLS, "calls.fossil", &[("expr.shex", EXPR_SHEX)]);
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
    .unwrap_or_else(|e| {
        panic!(
            "execute_vertex runs the DataFusion plan: {e}; {:?}",
            support::diagnostics(&db, file)
        )
    });

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
type { Person } = io.shex(\"expr.shex\")

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    @subject = `${ex:}user/${.id}`
    tag = str.slug(str.upper(.name))
";
    let (db, file) = support::db_with_shapes(NESTED, "nested.fossil", &[("expr.shex", EXPR_SHEX)]);
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
    .unwrap_or_else(|e| {
        panic!(
            "execute_vertex runs the nested plan: {e}; {:?}",
            support::diagnostics(&db, file)
        )
    });

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
type { Person } = io.shex(\"expr.shex\")

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    @subject = `${ex:}user/${.id}`
    senior = .id >= 2
";
    let (db, file) = support::db_with_shapes(COMPARES, "cmp.fossil", &[("expr.shex", EXPR_SHEX)]);
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
    .unwrap_or_else(|e| {
        panic!(
            "execute_vertex runs the comparison plan: {e}; {:?}",
            support::diagnostics(&db, file)
        )
    });

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

/// A conditional produces one column whose value depends on the row.
///
/// F2 §3: `cond ? a : b` renders as a two-armed CASE. Both branches have the
/// same type by the time it gets here — the checker refuses anything else, so
/// the column has one type and a shape can check it.
#[tokio::test]
async fn a_conditional_chooses_per_row() {
    const TERNARY: &str = "\
prefix ex: <https://example.org/>
type { Person } = io.shex(\"expr.shex\")

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    @subject = `${ex:}user/${.id}`
    band = .id >= 2 ? \"senior\" : \"junior\"
";
    let (db, file) = support::db_with_shapes(TERNARY, "tern.fossil", &[("expr.shex", EXPR_SHEX)]);
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
    .unwrap_or_else(|e| {
        panic!(
            "execute_vertex runs the conditional plan: {e}; {:?}",
            support::diagnostics(&db, file)
        )
    });

    assert_eq!(node.properties[0].name, "band");
    let batch = vertex.batches.first().expect("at least one RecordBatch");
    let band = column::<StringArray>(batch, 2);
    let values: Vec<&str> = (0..band.len()).map(|i| band.value(i)).collect();
    assert_eq!(
        values,
        ["junior", "senior", "senior"],
        "ids 1,2,3 — both arms were taken, so neither is dead code"
    );
}

/// One catalogue entry, reached through the value or through the type.
///
/// `str.upper` is a member of the `str` TYPE, not a function in a drawer, so it
/// is reached either way round — the precedent is `s.len()` ≡ `str::len(&s)` in
/// Rust and `s.upper()` ≡ `str.upper(s)` in Python. It is a resolution rule and
/// not a second way of saying the same thing, and the accepted cost is that
/// both spellings will appear in the wild. The SOURCE-level pipeline
/// (`User.where(...)`, a relation rather than a value) is F5.
#[tokio::test]
async fn the_value_path_and_the_type_path_are_the_same_program() {
    // `a |> f()` was the third spelling here and ruling 7 of 2026-08-11 retired
    // it; the claim it pinned — two spellings, one program — survives as the two
    // paths to a member. `x.upper()` and `str.upper(x)` are one catalogue row
    // reached two ways, and this is the test that they produce the same column
    // rather than merely resolving to the same name.
    const PIPED: &str = "\
prefix ex: <https://example.org/>
type { Person } = io.shex(\"expr.shex\")

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    @subject = `${ex:}user/${.id}`
    piped = users.name.upper()
    called = str.upper(users.name)
";
    let (db, file) = support::db_with_shapes(PIPED, "piped.fossil", &[("expr.shex", EXPR_SHEX)]);
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
    .unwrap_or_else(|e| {
        panic!(
            "execute_vertex runs both paths: {e}; {:?}",
            support::diagnostics(&db, file)
        )
    });

    let batch = vertex.batches.first().expect("at least one RecordBatch");
    let piped = column::<StringArray>(batch, 2);
    let called = column::<StringArray>(batch, 3);
    let a: Vec<&str> = (0..piped.len()).map(|i| piped.value(i)).collect();
    let b: Vec<&str> = (0..called.len()).map(|i| called.value(i)).collect();
    assert_eq!(a, ["ALICE", "BOB", "CAROL"]);
    assert_eq!(
        a, b,
        "the value path and the type path are the same program"
    );
}
