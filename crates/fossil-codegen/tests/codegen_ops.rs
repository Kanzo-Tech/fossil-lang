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
use fossil_mir::op::{CmpOp, Expr, Op, SinkRef, SourceFormat};
use smol_str::SmolStr;

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
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
    Op::Source {
        uri: SmolStr::from(uri),
        format: SourceFormat::Csv,
        row_type: string_record(db, cols),
    }
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
