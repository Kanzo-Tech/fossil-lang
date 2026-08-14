//! The three relational operators the source pipeline needs, executed.
//!
//! `Op::Filter`, `Op::Project` and `Op::Join` have been defined since phase 4
//! and reached by nothing: the operator algebra was defined WHOLE and lowered in
//! part on purpose, and the source pipeline is what starts paying that debt back.
//! The lowering that will emit them is F5; this file is the other half —
//! it builds the op list by hand, which is the pattern this repo already uses
//! for an operator no `.fossil` can produce yet, and asserts the **rows that
//! come out**, not the plan that was built.
//!
//! `cargo test`'s cwd is the crate root, so the sources are
//! `tests/fixtures/{users,teams}.csv`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::{Array, Int64Array, StringArray};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::SessionContext;
use fossil_base::{FossilDb, NativeSystem, System};
use fossil_graph_schema::Primitive;
use fossil_hir::ty::{Record, RecordField};
use fossil_hir::{BinOp, Ty, TyKind};
use fossil_mir::{Expr, JoinKind, Op, ProjectedColumn, SinkRef, SourceFormat, VProp};

/// The anchor these op-list tests resolve their sources against.
///
/// They build the ops by hand, so there is no program on disk to take a
/// directory from: the empty path anchors a relative URI to itself, which is
/// what every one of these fixtures wants — `users.csv` beside the test's own
/// working directory. It is spelled out rather than defaulted because the whole
/// point of `SourceAnchor` is that no caller resolves a path without saying
/// what it is resolved against.
fn anchor() -> fossil_base::SourceAnchor<'static> {
    fossil_base::SourceAnchor::beside(std::path::Path::new(""))
}
use smol_str::SmolStr;

fn db() -> FossilDb {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    FossilDb::new(system)
}

/// A `Record` row type over `names`, all `String`. Only `fossil_mir::schema_of`
/// reads it — the backend takes its columns from the file — but a `Source` is
/// not well-formed without one, and a test that lies about the row is a test
/// that stops being evidence.
fn row<'db>(db: &'db dyn fossil_base::Db, names: &[&str]) -> Ty<'db> {
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let fields: Vec<RecordField<'db>> = names
        .iter()
        .map(|n| RecordField {
            name: SmolStr::from(*n),
            ty: string_ty,
        })
        .collect();
    Ty::new(db, TyKind::Record(Record::new(db, fields)))
}

fn source<'db>(db: &'db dyn fossil_base::Db, file: &str, binding: &str, cols: &[&str]) -> Op<'db> {
    Op::Source {
        uri: SmolStr::from(format!("tests/fixtures/{file}")),
        format: SourceFormat::Csv,
        row_type: row(db, cols),
        binding: SmolStr::from(binding),
    }
}

fn col(source: &str, column: &str) -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::from(source),
        column: SmolStr::from(column),
    }
}

/// The `on = left.k == right.k` condition this engine admits — one key name on
/// both sides,
/// `USING (k)` — spelled as the lowering will spell it:
/// `BinOp { Eq, ColRef(left.k), ColRef(right.k) }`.
fn on_key<'db>(db: &'db dyn fossil_base::Db, left: &str, right: &str, key: &str) -> Expr<'db> {
    Expr::BinOp {
        op: BinOp::Eq,
        lhs: Box::new(col(left, key)),
        rhs: Box::new(col(right, key)),
        ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
    }
}

fn strings(batch: &RecordBatch, i: usize) -> Vec<String> {
    let a = batch
        .column(i)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("a string column");
    (0..a.len()).map(|r| a.value(r).to_string()).collect()
}

fn ints(batch: &RecordBatch, i: usize) -> Vec<i64> {
    let a = batch
        .column(i)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("an integer column");
    (0..a.len()).map(|r| a.value(r)).collect()
}

fn names(batch: &RecordBatch) -> Vec<String> {
    batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect()
}

/// Exactly one batch, or the assertion says how many there were.
fn one(batches: Vec<RecordBatch>) -> RecordBatch {
    assert_eq!(batches.len(), 1, "the fixture fits in one batch");
    batches.into_iter().next().expect("one batch")
}

/// `where(.id >= 2)` keeps the two rows it admits and drops the one it does not.
#[tokio::test]
async fn a_filter_keeps_the_rows_its_predicate_admits() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        Op::Filter {
            input: 0,
            pred: Expr::BinOp {
                op: BinOp::Ge,
                lhs: Box::new(col("users", "id")),
                rhs: Box::new(Expr::LitInt(2)),
                ty: Ty::new(&db, TyKind::Primitive(Primitive::Bool)),
            },
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 1, anchor())
        .await
        .expect("the filter plans");
    let batch = one(df.collect().await.expect("the filter runs"));

    assert_eq!(
        names(&batch),
        ["id", "name"],
        "a filter is schema-preserving"
    );
    assert_eq!(ints(&batch, 0), [2, 3]);
    assert_eq!(strings(&batch, 1), ["Bob", "Carol"]);
}

/// `select(users.name)` restricts the row to the columns it names, in that order.
#[tokio::test]
async fn a_projection_restricts_the_row_to_the_columns_it_names() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        Op::Project {
            input: 0,
            cols: vec![ProjectedColumn {
                source: SmolStr::new_static("users"),
                column: SmolStr::new_static("name"),
            }],
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 1, anchor())
        .await
        .expect("the projection plans");
    let batch = one(df.collect().await.expect("the projection runs"));

    assert_eq!(names(&batch), ["name"], "`id` is gone, not merely unread");
    assert_eq!(strings(&batch, 0), ["Alice", "Bob", "Carol"]);
}

/// `join(teams, on = users.id == teams.id)` is `USING (id)`: the key is in the
/// result once, the
/// two rows compose, and a row with no partner on either side is not there.
#[tokio::test]
async fn an_inner_join_composes_the_rows_and_names_the_key_once() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "teams.csv", "teams", &["id", "team"]),
        Op::Join {
            left: 0,
            right: 1,
            on: on_key(&db, "users", "teams", "id"),
            kind: JoinKind::Inner,
            left_name: SmolStr::new_static("users"),
            right_name: SmolStr::new_static("teams"),
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
        .await
        .expect("the join plans")
        .sort_by(vec![datafusion::prelude::col("id")])
        .expect("a deterministic order to assert against");
    let batch = one(df.collect().await.expect("the join runs"));

    assert_eq!(
        names(&batch),
        ["id", "name", "team"],
        "fila(izq) ⊎ fila(der) with `id` identified once — NOT `id, name, id, team`"
    );
    // users has 1,2,3; teams has 1,3,4. Inner → 1 and 3, and nothing else.
    assert_eq!(ints(&batch, 0), [1, 3]);
    assert_eq!(strings(&batch, 1), ["Alice", "Carol"]);
    assert_eq!(strings(&batch, 2), ["Blue", "Red"]);
}

/// The three compose: join, then filter on a column that came from the right
/// side, then project. Each operator reads the one before it by index, which is
/// the whole claim `input` makes.
#[tokio::test]
async fn the_three_operators_compose_in_one_chain() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "teams.csv", "teams", &["id", "team"]),
        Op::Join {
            left: 0,
            right: 1,
            on: on_key(&db, "users", "teams", "id"),
            kind: JoinKind::Inner,
            left_name: SmolStr::new_static("users"),
            right_name: SmolStr::new_static("teams"),
        },
        Op::Filter {
            input: 2,
            pred: Expr::BinOp {
                op: BinOp::Eq,
                lhs: Box::new(col("", "team")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static("Red"))),
                ty: Ty::new(&db, TyKind::Primitive(Primitive::Bool)),
            },
        },
        Op::Project {
            input: 3,
            cols: vec![
                ProjectedColumn {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("name"),
                },
                ProjectedColumn {
                    source: SmolStr::new_static("teams"),
                    column: SmolStr::new_static("team"),
                },
            ],
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 4, anchor())
        .await
        .expect("the chain plans");
    let batch = one(df.collect().await.expect("the chain runs"));

    assert_eq!(names(&batch), ["name", "team"]);
    assert_eq!(strings(&batch, 0), ["Carol"]);
    assert_eq!(strings(&batch, 1), ["Red"]);
}

/// The point of the whole thing: a vertex whose property came from the other
/// side of a join. An `EmitVertex` reads the relation its `input` names, and
/// with a pipeline in front of it that is no longer "the mapping's source".
#[tokio::test]
async fn a_joined_relation_feeds_the_vertex_it_emits() {
    let db = db();
    let string_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "teams.csv", "teams", &["id", "team"]),
        Op::Join {
            left: 0,
            right: 1,
            on: on_key(&db, "users", "teams", "id"),
            kind: JoinKind::Inner,
            left_name: SmolStr::new_static("users"),
            right_name: SmolStr::new_static("teams"),
        },
        Op::EmitVertex {
            input: 2,
            type_name: SmolStr::new_static("Person"),
            rdf_type: Some(SmolStr::new_static("https://example.org/Person")),
            id: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static(
                    "https://example.org/user/",
                ))),
                Box::new(col("", "id")),
            ),
            dedup: true,
            props: vec![
                VProp {
                    name: SmolStr::new_static("name"),
                    value: col("", "name"),
                    ty: string_ty,
                    rdf_uri: Some(SmolStr::new_static("https://example.org/name")),
                    single_valued: true,
                },
                VProp {
                    name: SmolStr::new_static("team"),
                    value: col("", "team"),
                    ty: string_ty,
                    rdf_uri: Some(SmolStr::new_static("https://example.org/team")),
                    single_valued: true,
                },
            ],
        },
        Op::Sink {
            input: 3,
            sink: SinkRef::GraphAr,
        },
    ];

    let ctx = SessionContext::new();
    let (vertex, node) = fossil_df::execute_vertex_ops(&ctx, &db, &ops, anchor())
        .await
        .expect("the joined vertex materialises");

    let props: Vec<&str> = node.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(props, ["name", "team"]);

    let batch = one(vertex.batches);
    assert_eq!(
        names(&batch),
        [
            "dense_id",
            "subject",
            "name",
            "team",
            "x",
            "y",
            "cluster_id"
        ],
        "the W0b shape, with a column that only exists because of the join",
    );
    assert_eq!(
        strings(&batch, 1),
        ["https://example.org/user/1", "https://example.org/user/3"],
        "only the two users with a team survive the inner join"
    );
    assert_eq!(strings(&batch, 2), ["Alice", "Carol"]);
    assert_eq!(strings(&batch, 3), ["Blue", "Red"]);
}

/// The other three `JoinKind`s stay unreachable, and reaching one says so.
#[tokio::test]
async fn an_outer_join_is_refused_by_name() {
    let db = db();
    for kind in [JoinKind::LeftOuter, JoinKind::RightOuter, JoinKind::Full] {
        let ops = vec![
            source(&db, "users.csv", "users", &["id", "name"]),
            source(&db, "teams.csv", "teams", &["id", "team"]),
            Op::Join {
                left: 0,
                right: 1,
                on: on_key(&db, "users", "teams", "id"),
                kind,
                left_name: SmolStr::new_static("users"),
                right_name: SmolStr::new_static("teams"),
            },
        ];
        let ctx = SessionContext::new();
        let err = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
            .await
            .expect_err("only Inner is executable")
            .to_string();
        assert!(
            err.contains(&format!("{kind:?}")) && err.contains("Inner"),
            "the error names the variant that was asked for: {err}"
        );
    }
}

/// A condition that is not the admitted equality fails, and the failure says
/// what arrived — it never becomes a quietly different join.
#[tokio::test]
async fn a_condition_that_is_not_an_equality_by_name_is_refused() {
    let db = db();
    let bool_ty = Ty::new(&db, TyKind::Primitive(Primitive::Bool));
    let two_sources = || {
        vec![
            source(&db, "users.csv", "users", &["id", "name"]),
            source(&db, "teams.csv", "teams", &["id", "team"]),
        ]
    };
    let cases: Vec<(Expr<'_>, &str)> = vec![
        // Not `==`.
        (
            Expr::BinOp {
                op: BinOp::Lt,
                lhs: Box::new(col("users", "id")),
                rhs: Box::new(col("teams", "id")),
                ty: bool_ty,
            },
            "not `==`",
        ),
        // Not two column references.
        (
            Expr::BinOp {
                op: BinOp::Eq,
                lhs: Box::new(col("users", "id")),
                rhs: Box::new(Expr::LitInt(1)),
                ty: bool_ty,
            },
            "integer literal",
        ),
        // Two different names — the extension that is declared and not built.
        (
            Expr::BinOp {
                op: BinOp::Eq,
                lhs: Box::new(col("users", "id")),
                rhs: Box::new(col("teams", "team")),
                ty: bool_ty,
            },
            "`id` and `team`",
        ),
        // Not a `BinOp` at all.
        (Expr::LitBool(true), "boolean literal"),
    ];

    for (on, expected) in cases {
        let mut ops = two_sources();
        ops.push(Op::Join {
            left: 0,
            right: 1,
            on,
            kind: JoinKind::Inner,
            left_name: SmolStr::new_static("users"),
            right_name: SmolStr::new_static("teams"),
        });
        let ctx = SessionContext::new();
        let err = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
            .await
            .expect_err("only `on = a.k == b.k` is admitted")
            .to_string();
        assert!(
            err.contains(expected),
            "the error names what arrived (wanted {expected:?}): {err}"
        );
    }
}

/// `a.join(a as b, on = a.k == b.k)` collides on every column but the key — a join
/// identifies the key and nothing else, so any other shared name is an error
/// rather than a shadowing — and it never reaches a plan. The checker is meant
/// to catch it; the backend
/// does not paper over it if it does not.
#[tokio::test]
async fn a_self_join_collides_and_says_which_column() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "users.csv", "again", &["id", "name"]),
        Op::Join {
            left: 0,
            right: 1,
            on: on_key(&db, "users", "again", "id"),
            kind: JoinKind::Inner,
            left_name: SmolStr::new_static("users"),
            right_name: SmolStr::new_static("again"),
        },
    ];
    let ctx = SessionContext::new();
    let err = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
        .await
        .expect_err("a shared non-key column is not joinable")
        .to_string();
    assert!(
        err.contains("name") && err.contains("users") && err.contains("again"),
        "the error names the column and both sources: {err}"
    );
}

/// An operator that is defined and not executed fails as itself. Seven of the
/// eleven are in this state; `Union` stands for them.
#[tokio::test]
async fn an_unexecuted_operator_fails_as_itself() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "users.csv", "again", &["id", "name"]),
        Op::Union { left: 0, right: 1 },
    ];
    let ctx = SessionContext::new();
    let err = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
        .await
        .expect_err("Union is defined and unreached")
        .to_string();
    assert!(err.contains("Union"), "the error names the operator: {err}");
}
