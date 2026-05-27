//! Direct-MIR-construction SQL snapshots for the single-input operators.
//!
//! Per ADR-0009 the operators `Project` / `Extend` / `Rename` / `Filter` /
//! `Distinct` / `Union` / `Empty` are NOT reachable from `.fossil` source
//! (`HirExpr` has only 4 leaf forms — no pipeline / call / filter / join
//! syntax). So this suite hand-builds [`MirGraph`]s and drives them through the
//! [`fossil_codegen::codegen_graph`] seam, then snapshots the emitted `DuckDB`
//! SQL.
//!
//! # The `#[salsa::tracked]` test seam
//!
//! `MirGraph::new` is a Salsa tracked struct, so it may only be created INSIDE
//! a tracked function (salsa panics otherwise). The per-op graphs are therefore
//! built inside [`codegen_case`], a tracked function keyed by a [`Case`] input
//! that selects which operator to construct; it builds the `MirGraph` and calls
//! `fossil_codegen::codegen_graph` from within the same tracked frame.
//!
//! These snapshots become part of the 30-mapping corpus (plan 04-07
//! consolidates / extends). They lock the sqlparser-AST hybrid (ADR-0012):
//! the inner SELECT bodies are AST-built, the terminal COPY is hand-wrapped.

#![cfg(not(target_arch = "wasm32"))]
// The `'db` lifetime is written explicitly on the tracked seam (documenting the
// salsa frame) and on the op-builder helpers (uniform with the seam); clippy's
// elision lint is noise for this test scaffolding.
#![allow(clippy::elidable_lifetime_names)]

use std::sync::Arc;

use fossil_codegen::codegen_graph;
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{AggFn, AggSpec, CmpOp, Expr, JoinKind, Op, SinkRef, SourceFormat};
use smol_str::SmolStr;

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

/// Selects which operator's graph [`codegen_case`] builds.
#[salsa::input]
struct Case {
    op: u8,
}

/// Build the chosen operator's `MirGraph` and codegen it — all inside a tracked
/// frame so `MirGraph::new` / `Ty` interning are legal and the
/// `fossil_codegen::codegen_graph` query runs normally.
#[salsa::tracked]
fn codegen_case<'db>(db: &'db dyn fossil_base::Db, case: Case) -> fossil_codegen::SqlPlan<'db> {
    let ops = match case.op(db) {
        0 => project_two_cols_ops(db),
        1 => extend_concat_ops(db),
        2 => rename_col_ops(db),
        3 => filter_eq_ops(db),
        4 => distinct_all_ops(db),
        5 => distinct_by_ops(db),
        6 => union_two_sources_ops(db),
        7 => empty_relation_ops(db),
        8 => join_inner_ops(db),
        9 => join_left_outer_ops(db),
        10 => group_by_count_ops(db),
        11 => aggregate_sum_ops(db),
        12 => multi_triple_emit_sink_ops(db),
        13 => assert_iri_template_unbound_ops(db),
        14 => json_source_ops(db),
        15 => parquet_source_ops(db),
        other => panic!("unknown case {other}"),
    };
    codegen_graph(db, MirGraph::new(db, ops))
}

fn sql_for(op: u8) -> String {
    let db = db();
    let case = Case::new(&db, op);
    let plan = codegen_case(&db, case);
    plan.sql(&db).clone()
}

// --------------------------------------------------------------------------
// Op-construction helpers.
// --------------------------------------------------------------------------

fn string_record<'db>(db: &'db dyn fossil_base::Db, names: &[&str]) -> Ty<'db> {
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

fn source<'db>(db: &'db dyn fossil_base::Db, uri: &str, cols: &[&str]) -> Op<'db> {
    source_with_format(db, uri, cols, SourceFormat::Csv)
}

fn source_with_format<'db>(
    db: &'db dyn fossil_base::Db,
    uri: &str,
    cols: &[&str],
    format: SourceFormat,
) -> Op<'db> {
    Op::Source {
        uri: SmolStr::from(uri),
        format,
        row_type: string_record(db, cols),
    }
}

/// STDL-06: a `Json`-format source feeding a `TripleEmit` + `Sink`. Snapshots
/// the `CREATE VIEW ... read_json_auto('a.json')` reader (the only difference
/// from the csv path).
fn json_source_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![source_with_format(
        db,
        "a.json",
        &["id", "name"],
        SourceFormat::Json,
    )];
    ops.extend(emit_and_sink(0, "a"));
    ops
}

/// STDL-06: a `Parquet`-format source feeding a `TripleEmit` + `Sink`.
/// Snapshots the `CREATE VIEW ... read_parquet('a.parquet')` reader.
fn parquet_source_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![source_with_format(
        db,
        "a.parquet",
        &["id", "name"],
        SourceFormat::Parquet,
    )];
    ops.extend(emit_and_sink(0, "a"));
    ops
}

/// A `TripleEmit(subject = iri, predicate, object = <obj_src>.name)` over
/// `input` + a `Sink`. Appended so each graph renders the full `CREATE VIEW` +
/// `COPY` script the codegen produces in practice; the `subject` references the
/// conventional `iri` column.
fn emit_and_sink<'db>(input: usize, obj_src: &str) -> Vec<Op<'db>> {
    vec![
        Op::TripleEmit {
            input,
            subject: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("iri"),
            },
            predicate: SmolStr::new_static("https://example.org/name"),
            object: Expr::ColRef {
                source: SmolStr::from(obj_src),
                column: SmolStr::new_static("name"),
            },
            graph: None,
        },
        Op::Sink {
            input: input + 1,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn project_two_cols_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name", "status"]),
        Op::Project {
            input: 0,
            cols: vec![SmolStr::new_static("id"), SmolStr::new_static("name")],
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn extend_concat_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("https://example.org/"))),
                Box::new(Expr::ColRef {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("id"),
                }),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("iri"),
            },
            predicate: SmolStr::new_static("https://example.org/name"),
            object: Expr::ColRef {
                source: SmolStr::new_static("users"),
                column: SmolStr::new_static("name"),
            },
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn rename_col_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Rename {
            input: 0,
            old: SmolStr::new_static("name"),
            new: SmolStr::new_static("label"),
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn filter_eq_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name", "status"]),
        Op::Filter {
            input: 0,
            pred: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(Expr::ColRef {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("status"),
                }),
                rhs: Box::new(Expr::LitString(SmolStr::new_static("active"))),
                ty: bool_ty,
            },
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn distinct_all_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Distinct { input: 0, by: None },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn distinct_by_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Distinct {
            input: 0,
            by: Some(vec![SmolStr::new_static("id")]),
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn union_two_sources_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![
        source(db, "examples/a.csv", &["id", "name"]),
        source(db, "examples/b.csv", &["id", "name"]),
        Op::Union { left: 0, right: 1 },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

fn empty_relation_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Empty {
            schema: vec![SmolStr::new_static("id"), SmolStr::new_static("name")],
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

/// `Source(orders) Source(users) Join(0,1,on=orders.user_id=users.id,kind)`
/// → `TripleEmit` → `Sink`. Locks the per-`JoinKind` JOIN keyword + auto-prefix
/// qualifiers (operator-algebra.md §2.6).
fn join_ops<'db>(db: &'db dyn fossil_base::Db, kind: JoinKind) -> Vec<Op<'db>> {
    let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
    let mut ops = vec![
        source(db, "examples/orders.csv", &["id", "user_id"]),
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Join {
            left: 0,
            right: 1,
            on: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(Expr::ColRef {
                    source: SmolStr::new_static("orders"),
                    column: SmolStr::new_static("user_id"),
                }),
                rhs: Box::new(Expr::ColRef {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("id"),
                }),
                ty: bool_ty,
            },
            kind,
            left_name: SmolStr::new_static("orders"),
            right_name: SmolStr::new_static("users"),
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

fn join_inner_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    join_ops(db, JoinKind::Inner)
}

fn join_left_outer_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    join_ops(db, JoinKind::LeftOuter)
}

/// `Source(users) GroupBy(["country"]) Aggregate([COUNT(id) AS n])` →
/// `TripleEmit` → `Sink`. The `GroupBy` + consuming `Aggregate` render into one
/// `GROUP BY` SELECT (operator-algebra.md §2.8/§2.9).
fn group_by_count_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let bigint_ty = Ty::new(db, TyKind::Primitive(Primitive::Integer));
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "country"]),
        Op::GroupBy {
            input: 0,
            keys: vec![SmolStr::new_static("country")],
        },
        Op::Aggregate {
            input: 1,
            aggs: vec![AggSpec {
                out_field: SmolStr::new_static("n"),
                agg_fn: AggFn::Count,
                in_field: SmolStr::new_static("id"),
                ty: bigint_ty,
            }],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

/// `Source(orders) GroupBy(["user_id"]) Aggregate([SUM(amount) AS total])`.
/// Locks the `SUM(...)` aggregate spelling.
fn aggregate_sum_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let num_ty = Ty::new(db, TyKind::Primitive(Primitive::Integer));
    let mut ops = vec![
        source(db, "examples/orders.csv", &["user_id", "amount"]),
        Op::GroupBy {
            input: 0,
            keys: vec![SmolStr::new_static("user_id")],
        },
        Op::Aggregate {
            input: 1,
            aggs: vec![AggSpec {
                out_field: SmolStr::new_static("total"),
                agg_fn: AggFn::Sum,
                in_field: SmolStr::new_static("amount"),
                ty: num_ty,
            }],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

/// `Source(users) Extend("iri", ...) TripleEmit(p1) TripleEmit(p2) Sink`.
/// Two `TripleEmit`s over the same `Extend` → the COPY inner SELECT
/// `UNION ALL`s the two triple-projections (generalised `SinkOp`; flat-triple
/// `GraphAr` contract).
fn multi_triple_emit_sink_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    let iri = || Expr::ColRef {
        source: SmolStr::default(),
        column: SmolStr::new_static("iri"),
    };
    vec![
        source(db, "examples/users.csv", &["id", "name", "email"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("https://example.org/"))),
                Box::new(Expr::ColRef {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("id"),
                }),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: Expr::ColRef {
                source: SmolStr::new_static("users"),
                column: SmolStr::new_static("name"),
            },
            graph: None,
        },
        Op::TripleEmit {
            input: 1,
            subject: iri(),
            predicate: SmolStr::new_static("https://example.org/email"),
            object: Expr::ColRef {
                source: SmolStr::new_static("users"),
                column: SmolStr::new_static("email"),
            },
            graph: None,
        },
        Op::Sink {
            input: 3,
            sink: SinkRef::GraphAr,
        },
    ]
}

/// `Source(users) Extend("iri", "p/" || Assert(users.id)) TripleEmit Sink`.
///
/// The `iri` template's `${.id}` field ref is wrapped in
/// `Expr::Assert { name: "iri_template_unbound", span_line: 42, .. }` (the SC#4
/// / P-CRIT-4 carrier — `lower_to_mir` populates this in practice; here we
/// construct it directly with a fixed line so the snapshot is stable). Codegen
/// must render the named runtime assertion `CASE WHEN <id> IS NOT NULL THEN
/// <id> ELSE error('fossil_assertion_iri_template_unbound:line=42') END`.
fn assert_iri_template_unbound_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static(
                    "https://example.org/user/",
                ))),
                Box::new(Expr::Assert {
                    name: SmolStr::new_static("iri_template_unbound"),
                    span_line: 42,
                    inner: Box::new(Expr::ColRef {
                        source: SmolStr::new_static("users"),
                        column: SmolStr::new_static("id"),
                    }),
                }),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("iri"),
            },
            predicate: SmolStr::new_static("https://example.org/name"),
            object: Expr::ColRef {
                source: SmolStr::new_static("users"),
                column: SmolStr::new_static("name"),
            },
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

// --------------------------------------------------------------------------
// Snapshots — one per single-input operator (ADR-0009 reachability gap).
// --------------------------------------------------------------------------

#[test]
fn project_two_cols() {
    insta::assert_snapshot!("project_two_cols", sql_for(0));
}

#[test]
fn extend_concat() {
    insta::assert_snapshot!("extend_concat", sql_for(1));
}

#[test]
fn rename_col() {
    insta::assert_snapshot!("rename_col", sql_for(2));
}

#[test]
fn filter_eq() {
    insta::assert_snapshot!("filter_eq", sql_for(3));
}

#[test]
fn distinct_all() {
    insta::assert_snapshot!("distinct_all", sql_for(4));
}

#[test]
fn distinct_by() {
    insta::assert_snapshot!("distinct_by", sql_for(5));
}

#[test]
fn union_two_sources() {
    insta::assert_snapshot!("union_two_sources", sql_for(6));
}

#[test]
fn empty_relation() {
    insta::assert_snapshot!("empty_relation", sql_for(7));
}

// --------------------------------------------------------------------------
// Snapshots — multi-input / aggregating ops + multi-TripleEmit Sink (04-05).
// --------------------------------------------------------------------------

#[test]
fn join_inner() {
    insta::assert_snapshot!("join_inner", sql_for(8));
}

#[test]
fn join_left_outer() {
    insta::assert_snapshot!("join_left_outer", sql_for(9));
}

#[test]
fn group_by_count() {
    insta::assert_snapshot!("group_by_count", sql_for(10));
}

#[test]
fn aggregate_sum() {
    insta::assert_snapshot!("aggregate_sum", sql_for(11));
}

#[test]
fn multi_triple_emit_sink() {
    insta::assert_snapshot!("multi_triple_emit_sink", sql_for(12));
}

// --------------------------------------------------------------------------
// Snapshot — STDL-06 io source formats (read_json_auto / read_parquet).
// --------------------------------------------------------------------------

/// `io.json` source → `CREATE VIEW a AS SELECT * FROM read_json_auto('a.json')`.
#[test]
fn json_source_reader() {
    let sql = sql_for(14);
    assert!(
        sql.contains("read_json_auto('a.json')"),
        "expected read_json_auto reader, got:\n{sql}"
    );
    assert!(
        !sql.contains("read_csv_auto"),
        "csv reader leaked into json source SQL:\n{sql}"
    );
    insta::assert_snapshot!("json_source", sql);
}

/// `io.parquet` source → `CREATE VIEW a AS SELECT * FROM
/// read_parquet('a.parquet')`.
#[test]
fn parquet_source_reader() {
    let sql = sql_for(15);
    assert!(
        sql.contains("read_parquet('a.parquet')"),
        "expected read_parquet reader, got:\n{sql}"
    );
    insta::assert_snapshot!("parquet_source", sql);
}

// --------------------------------------------------------------------------
// Snapshot — SC#4 named runtime assertion (P-CRIT-4 / CORE-10, plan 04-06).
// --------------------------------------------------------------------------

/// Locks the `CASE WHEN <id> IS NOT NULL THEN <id> ELSE
/// error('fossil_assertion_iri_template_unbound:line=42') END` shape verbatim,
/// and guards (below) that NO `TyKind::Unknown` / `InferenceId` text leaked
/// into the SQL (RESEARCH Pitfall 5).
#[test]
fn assert_iri_template_unbound() {
    insta::assert_snapshot!("assert_iri_template_unbound", sql_for(13));
}

/// SC#4 no-leak guard (RESEARCH Pitfall 5): the rendered assertion SQL must
/// contain the named, line-located message but NEVER any `TyKind::Unknown(` or
/// `InferenceId` debug text. The assertion name is a fixed `snake_case`
/// identifier; the only number embedded is the resolved `line=<N>`.
#[test]
fn assert_sql_never_leaks_unknown_or_inference_id() {
    let sql = sql_for(13);
    assert!(
        sql.contains("error('fossil_assertion_iri_template_unbound:line=42')"),
        "expected the named runtime assertion in the SQL, got:\n{sql}"
    );
    assert!(
        sql.contains("CASE WHEN users.id IS NOT NULL THEN users.id ELSE"),
        "expected the CASE WHEN ... guard shape, got:\n{sql}"
    );
    assert!(
        !sql.contains("Unknown("),
        "TyKind::Unknown leaked into the assertion SQL:\n{sql}"
    );
    assert!(
        !sql.contains("InferenceId"),
        "InferenceId leaked into the assertion SQL:\n{sql}"
    );
}
