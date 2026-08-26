//! The relational operators the source pipeline needs, executed.
//!
//! `Op::Filter`, `Op::Project`, `Op::Join`, `Op::Distinct` and `Op::Union` were
//! defined when the algebra was, and reached by nothing: the operator algebra was
//! defined WHOLE and lowered in part on purpose, and the source pipeline is what starts paying that debt back.
//! `fossil-hir` and `fossil-mir` emit five of them from a program now; this is
//! the other half — it builds the op list by hand, which is the pattern this
//! repo already uses for an operator no `.fossil` can produce yet, and asserts
//! the **rows that come out**, not the plan that was built.
//!
//! `cargo test`'s cwd is the crate root, so the sources are
//! `tests/fixtures/{users,teams}.csv`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::{Array, Int64Array, StringArray};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::SessionContext;
use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, System};
use fossil_graph_schema::Primitive;
use fossil_hir::ty::{Record, RecordField};
use fossil_hir::{BinOp, Ty, TyKind};
use fossil_mir::{Expr, JoinKind, JoinSide, Op, ProjectedColumn, SinkRef, SourceFormat, VProp};

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

/// `on = left.lk == right.rk`, spelled as the lowering spells it:
/// `BinOp { Eq, ColRef(left.lk), ColRef(right.rk) }`. The two column names need
/// not agree — `eq` is what the surface writes and the backend plans.
fn eq<'db>(db: &'db dyn fossil_base::Db, lhs: Expr<'static>, rhs: Expr<'static>) -> Expr<'db> {
    Expr::BinOp {
        op: BinOp::Eq,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
        ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
    }
}

/// The same key name on both sides — the shape the retired `on = .k` sugar
/// produced, and now just one equality among the many that are legal.
fn on_key<'db>(db: &'db dyn fossil_base::Db, left: &str, right: &str, key: &str) -> Expr<'db> {
    eq(db, col(left, key), col(right, key))
}

/// One side of a join: the op index it reads and the name its columns are
/// addressed by, with no `as` alias.
fn side(input: usize, relation: &str) -> JoinSide {
    JoinSide {
        input,
        relation: SmolStr::from(relation),
        alias: None,
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

/// The column names of the PLAN's schema.
///
/// Not [`names`], which reads them off a batch — **an empty result is zero
/// batches, not one empty batch**, so a join that matches nothing has no batch
/// to read a schema from and [`one`] fails on it with `left: 0`. Two of the
/// tests below match nothing ON PURPOSE (that is what they assert: the join
/// PLANS), and the schema they are about is the plan's either way.
fn plan_names(df: &datafusion::prelude::DataFrame) -> Vec<String> {
    df.schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect()
}

/// Rows across every batch — `0` for the empty result, which `batches[0]` cannot
/// ask for.
fn total_rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(RecordBatch::num_rows).sum()
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

/// `join(teams, on = users.id == teams.id)`: the two rows COMPOSE, whole, and a
/// row with no partner on either side is not there.
///
/// This asserted `["id", "name", "team"]` until 2026-08-19 — the join was
/// `USING (id)` and identified the key, so the right side's copy was renamed
/// away and dropped. It does not any more: both sides keep every column, and
/// `users.id` and `teams.id` are two columns that happen to share a bare name.
/// The rule that made them one is the same rule that could not plan
/// `on = a.parent == b.id`, and it went with it.
#[tokio::test]
async fn an_inner_join_composes_both_rows_whole() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "teams.csv", "teams", &["id", "team"]),
        Op::Join {
            left: side(0, "users"),
            right: side(1, "teams"),
            on: on_key(&db, "users", "teams", "id"),
            kind: JoinKind::Inner,
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
        .await
        .expect("the join plans")
        .sort_by(vec![datafusion::prelude::col(
            datafusion::common::Column::new(
                Some(datafusion::common::TableReference::bare("users")),
                "id",
            ),
        )])
        .expect("a deterministic order to assert against");
    let batch = one(df.collect().await.expect("the join runs"));

    assert_eq!(
        names(&batch),
        ["id", "name", "id", "team"],
        "fila(izq) ++ fila(der): `id` twice, told apart by the relation, not by the name"
    );
    // users has 1,2,3; teams has 1,3,4. Inner → 1 and 3, and nothing else.
    assert_eq!(ints(&batch, 0), [1, 3]);
    assert_eq!(strings(&batch, 1), ["Alice", "Carol"]);
    assert_eq!(ints(&batch, 2), [1, 3]);
    assert_eq!(strings(&batch, 3), ["Blue", "Red"]);
}

/// **The defect this whole change is.** `on = users.id == teams.id` was the ONLY
/// join this engine planned; two differently-named keys were refused with "a
/// join key is one name on both sides … Two keys with different names are an
/// extension this engine does not build". They are not an extension — the
/// backend simply could not see which side a column came from, because the
/// lowering discarded the binding. It carries it now, and this plans.
#[tokio::test]
async fn a_join_whose_two_keys_are_named_differently_plans() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "teams.csv", "teams", &["id", "team"]),
        Op::Join {
            left: side(0, "users"),
            right: side(1, "teams"),
            // `users.name == teams.team` — nothing matches, which is the point:
            // the assertion is that it PLANS, not that it finds rows.
            on: eq(&db, col("users", "name"), col("teams", "team")),
            kind: JoinKind::Inner,
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
        .await
        .expect("two differently-named keys are an ordinary equi-join");
    // The PLAN's schema, because this join matches nothing by construction and
    // an empty result is zero batches — there is no row to read a name off.
    assert_eq!(plan_names(&df), ["id", "name", "id", "team"]);
    let batches = df.collect().await.expect("the join runs");
    assert_eq!(
        total_rows(&batches),
        0,
        "no user is named after a team, and finding rows was never the claim"
    );
}

/// A compound key — `on = a.k == b.k and a.j == b.j` — is a conjunction of
/// equalities, and DataFusion plans one hash join on two keys. It costs nothing
/// beyond splitting the condition on `and`, which is why it is in this change
/// rather than after it.
#[tokio::test]
async fn a_compound_key_joins_on_both_columns() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "teams.csv", "teams", &["id", "team"]),
        Op::Join {
            left: side(0, "users"),
            right: side(1, "teams"),
            on: Expr::BinOp {
                op: BinOp::And,
                lhs: Box::new(on_key(&db, "users", "teams", "id")),
                rhs: Box::new(eq(&db, col("users", "name"), col("teams", "team"))),
                ty: Ty::new(&db, TyKind::Primitive(Primitive::Bool)),
            },
            kind: JoinKind::Inner,
        },
    ];

    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
        .await
        .expect("a conjunction of equalities plans");
    assert_eq!(
        plan_names(&df),
        ["id", "name", "id", "team"],
        "the second conjunct narrows the rows, not the schema"
    );
    let batches = df.collect().await.expect("the join runs");
    assert_eq!(
        total_rows(&batches),
        0,
        "no user is named after their team, so the second equality admits nothing"
    );
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
            left: side(0, "users"),
            right: side(1, "teams"),
            on: on_key(&db, "users", "teams", "id"),
            kind: JoinKind::Inner,
        },
        Op::Filter {
            input: 2,
            pred: Expr::BinOp {
                op: BinOp::Eq,
                // Deliberately UNQUALIFIED — the retired bare `.team`, which is
                // still what a `FieldRef` lowers to. It resolves because only
                // one side has a `team`; a bare `id` here would be ambiguous
                // and DataFusion would name both candidates.
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
            left: side(0, "users"),
            right: side(1, "teams"),
            on: on_key(&db, "users", "teams", "id"),
            kind: JoinKind::Inner,
        },
        Op::EmitVertex {
            input: 2,
            type_name: SmolStr::new_static("Person"),
            rdf_type: Some(SmolStr::new_static("https://example.org/Person")),
            id: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static(
                    "https://example.org/user/",
                ))),
                // `users.id` and not a bare `id`: both sides carry one now, and
                // the binding is what says which.
                Box::new(col("users", "id")),
            ),
            dedup: true,
            props: vec![
                VProp {
                    name: SmolStr::new_static("name"),
                    value: col("users", "name"),
                    ty: string_ty,
                    rdf_uri: Some(SmolStr::new_static("https://example.org/name")),
                    single_valued: true,
                },
                VProp {
                    name: SmolStr::new_static("team"),
                    value: col("teams", "team"),
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
                left: side(0, "users"),
                right: side(1, "teams"),
                on: on_key(&db, "users", "teams", "id"),
                kind,
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

/// What still refuses a join, now that the equal-names rule is gone.
///
/// The three that survive are the three the qualification never stood in for: a
/// join is an EQUIJOIN (a theta join or a constant comparison would silently
/// become a nested loop over the product of two corpora), and a reference has
/// to name a side that exists and a column that side has. The fourth case this
/// used to carry — two differently-named keys — is
/// `a_join_whose_two_keys_are_named_differently_plans` now.
#[tokio::test]
async fn a_condition_that_is_not_an_equijoin_is_refused() {
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
            "equality between columns",
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
        // Not a `BinOp` at all.
        (Expr::LitBool(true), "boolean literal"),
        // A relation that is neither input. This check predates the binding
        // and could never fire while every source was empty: an empty source
        // is never unequal to both names.
        (
            eq(&db, col("users", "id"), col("nobody", "id")),
            "neither input",
        ),
        // A column the side it names does not have.
        (
            eq(&db, col("users", "id"), col("teams", "captain")),
            "the right side has",
        ),
        // One bad conjunct is enough: the split on `and` does not admit a
        // conjunction by admitting half of it.
        (
            Expr::BinOp {
                op: BinOp::And,
                lhs: Box::new(on_key(&db, "users", "teams", "id")),
                rhs: Box::new(Expr::LitBool(true)),
                ty: bool_ty,
            },
            "boolean literal",
        ),
    ];

    for (on, expected) in cases {
        let mut ops = two_sources();
        ops.push(Op::Join {
            left: side(0, "users"),
            right: side(1, "teams"),
            on,
            kind: JoinKind::Inner,
        });
        let ctx = SessionContext::new();
        let err = fossil_df::plan_relation(&ctx, &ops, 2, anchor())
            .await
            .expect_err("only a conjunction of column equalities is admitted")
            .to_string();
        assert!(
            err.contains(expected),
            "the error names what arrived (wanted {expected:?}): {err}"
        );
    }
}

/// `Node.join(Node as Other, on = Node.parent == Other.id)` — the self-join, in
/// the two ways it used to be impossible at once: the two keys are named
/// differently, and every column collides.
///
/// This asserted the collision refusal until 2026-08-19 ("`users` and `again` both
/// carry [\"name\"]"), on the reasoning that a join identifies the key and
/// nothing else so any other shared name is an error. `fossil-hir` deleted that
/// rule on 2026-08-14 (ruling 17) on the promise that qualification would do
/// the work; this is the backend keeping it. The alias is what re-qualifies the
/// right side — without it both are `users` and neither `users.name` resolves.
#[tokio::test]
async fn a_self_join_keeps_both_sides_apart_by_their_alias() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "users.csv", "users", &["id", "name"]),
        Op::Join {
            left: side(0, "users"),
            right: JoinSide {
                input: 1,
                relation: SmolStr::new_static("users"),
                alias: Some(SmolStr::new_static("again")),
            },
            on: eq(&db, col("users", "id"), col("again", "id")),
            kind: JoinKind::Inner,
        },
        Op::Project {
            input: 2,
            cols: vec![
                ProjectedColumn {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("name"),
                },
                ProjectedColumn {
                    source: SmolStr::new_static("again"),
                    column: SmolStr::new_static("name"),
                },
            ],
        },
    ];
    let ctx = SessionContext::new();
    let df = fossil_df::plan_relation(&ctx, &ops, 3, anchor())
        .await
        .expect("a self-join plans")
        .sort_by(vec![datafusion::prelude::col(
            datafusion::common::Column::new(
                Some(datafusion::common::TableReference::bare("users")),
                "name",
            ),
        )])
        .expect("a deterministic order");
    let batch = one(df.collect().await.expect("the self-join runs"));

    assert_eq!(
        strings(&batch, 0),
        ["Alice", "Bob", "Carol"],
        "every row joins itself on `id`"
    );
    assert_eq!(
        strings(&batch, 1),
        ["Alice", "Bob", "Carol"],
        "`again.name` is a SECOND column, selected under the alias"
    );
}

/// An operator that is defined and not executed fails as itself. `GroupBy`
/// stands for the rest: it is the next verb of the six and it is not here yet,
/// which is what makes it the honest stand-in rather than a retired one.
#[tokio::test]
async fn an_unexecuted_operator_fails_as_itself() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        Op::GroupBy {
            input: 0,
            keys: vec![SmolStr::from("name")],
        },
    ];
    let ctx = SessionContext::new();
    let err = fossil_df::plan_relation(&ctx, &ops, 1, anchor())
        .await
        .expect_err("GroupBy is defined and unreached")
        .to_string();
    assert!(
        err.contains("GroupBy"),
        "the error names the operator: {err}"
    );
}

/// `distinct()` keeps one of each identical row, and it keeps every column: the
/// row type is its input's, which is what `schema_of` says and what makes it
/// the cheap verb beside `select`.
///
/// `users.csv` has three distinct rows and `people_extra.csv` shares one of
/// them, so the union is six rows and the distinct over it is five. Asserting
/// on the union alone would not tell a `distinct` that works from one that is
/// a no-op — both give six.
#[tokio::test]
async fn distinct_over_a_union_drops_the_shared_row() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "people_extra.csv", "extra", &["id", "name"]),
        Op::Union {
            left: 0,
            right: 1,
            relation: SmolStr::from("everyone"),
        },
        Op::Distinct { input: 2 },
    ];
    let ctx = SessionContext::new();

    let both = collect_names(&ops, &ctx, 2).await;
    assert_eq!(
        both,
        ["Alice", "Bob", "Carol", "Carol", "Dave", "Eve"],
        "`union` is UNION ALL: Carol is in both files and appears twice"
    );

    let deduped = collect_names(&ops, &ctx, 3).await;
    assert_eq!(
        deduped,
        ["Alice", "Bob", "Carol", "Dave", "Eve"],
        "`distinct` keeps one of the two identical Carol rows"
    );
}

/// The union's columns answer to the relation the op names, and to NEITHER
/// side's — which is the whole reason `Op::Union` carries one.
///
/// The projection is written against `everyone`, a name no `Op::Source` below
/// it uses. Before the field existed the plan kept the left side's qualifier
/// and this select resolved against nothing.
#[tokio::test]
async fn a_union_is_addressed_by_its_own_relation() {
    let db = db();
    let ops = vec![
        source(&db, "users.csv", "users", &["id", "name"]),
        source(&db, "people_extra.csv", "extra", &["id", "name"]),
        Op::Union {
            left: 0,
            right: 1,
            relation: SmolStr::from("everyone"),
        },
        Op::Project {
            input: 2,
            cols: vec![ProjectedColumn {
                source: SmolStr::from("everyone"),
                column: SmolStr::from("name"),
            }],
        },
    ];
    let ctx = SessionContext::new();
    assert_eq!(
        collect_names(&ops, &ctx, 3).await,
        ["Alice", "Bob", "Carol", "Carol", "Dave", "Eve"],
        "`everyone.name` resolves against the union, not against `users`"
    );

    let err = fossil_df::plan_relation(&ctx, &ops[..3], 2, anchor())
        .await
        .expect("the union plans")
        .select(vec![datafusion::prelude::col(
            datafusion::common::Column::new(
                Some(datafusion::common::TableReference::bare("users")),
                "name",
            ),
        )])
        .expect_err("the left side's name did not survive the union")
        .to_string();
    assert!(
        err.contains("users.name"),
        "the refusal names the column nobody can reach: {err}"
    );
}

/// The names a relation produces, sorted — the two verbs above are set
/// operations and neither promises an order.
async fn collect_names(ops: &[Op<'_>], ctx: &SessionContext, index: usize) -> Vec<String> {
    let df = fossil_df::plan_relation(ctx, ops, index, anchor())
        .await
        .expect("the relation plans");
    let batches = df.collect().await.expect("the relation runs");
    let name_at = |b: &RecordBatch| {
        let i = b.schema().fields().len() - 1;
        strings(b, i)
    };
    let mut names: Vec<String> = batches.iter().flat_map(name_at).collect();
    names.sort();
    names
}
