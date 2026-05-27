//! R1–R10 before/after MIR snapshots + idempotency assertions (SC#2).
//!
//! For each of the ten rewrite rules (operator-algebra.md §4.1 structural
//! R1–R6 + §4.2 typed R7–R10) this file:
//! 1. constructs a `MirGraph` whose ops match the rule's TRIGGER pattern via
//!    direct construction (the trigger ops are NOT reachable from `.fossil`
//!    source per ADR-0009, so we build them directly);
//! 2. snapshots the before-MIR (`rN_before`);
//! 3. runs `rewrite(&db, graph)`;
//! 4. snapshots the after-MIR (`rN_after`) — proving the rule fired;
//! 5. asserts idempotency: `rewrite(rewrite(g)).ops == rewrite(g).ops`
//!    (RESEARCH Pitfall 2 — the fixpoint converges).
//!
//! 20 snapshots (10 before + 10 after) + 10 idempotency asserts.
//!
//! # The `#[salsa::tracked]` test seam
//!
//! `MirGraph::new` is a Salsa tracked struct, so it may only be created INSIDE
//! a tracked function (salsa panics otherwise: "cannot create a tracked struct
//! disambiguator outside of a tracked function"). The trigger graphs are
//! therefore built inside [`trigger_graph`], a tracked function keyed by a
//! [`Case`] input that selects which rule's trigger pattern to construct.
//! Reads (`graph.ops(db)`) and `rewrite` itself (which constructs its result
//! `MirGraph` — but only when called transitively from a tracked frame… see
//! [`rewrite_graph`]) are wrapped likewise.

#![cfg(not(target_arch = "wasm32"))]
// The `'db` lifetime is written explicitly on the tracked seam (documenting the
// salsa frame) and on the op-builder helpers (uniform with the seam); clippy's
// elision lint is noise for this test scaffolding.
#![allow(clippy::elidable_lifetime_names)]

use std::sync::Arc;

use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{CmpOp, Expr, JoinKind, Op, SinkRef, SourceFormat};
use fossil_mir::rewrite::rewrite;
use smol_str::SmolStr;

// --------------------------------------------------------------------------
// Test-db harness (mirrors the fossil-mir / fossil-codegen setup).
// --------------------------------------------------------------------------

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

/// Selects which rule's trigger pattern [`trigger_graph`] builds.
#[salsa::input]
struct Case {
    rule: u8,
}

/// Build the trigger-pattern `MirGraph` for `case.rule` (1..=10). Tracked so
/// the `MirGraph::new` / `Source.row_type` interning runs inside a tracked
/// frame.
#[salsa::tracked]
fn trigger_graph<'db>(db: &'db dyn fossil_base::Db, case: Case) -> MirGraph<'db> {
    let ops = match case.rule(db) {
        1 => r1_trigger(db),
        2 => r2_trigger(db),
        3 => r3_trigger(db),
        4 => r4_trigger(db),
        5 => r5_trigger(db),
        6 => r6_trigger(db),
        7 => r7_trigger(db),
        8 => r8_trigger(db),
        9 => r9_trigger(db),
        10 => r10_trigger(db),
        other => panic!("unknown rule case {other}"),
    };
    MirGraph::new(db, ops)
}

/// Run the rewriting engine inside a tracked frame (so the result `MirGraph`
/// it constructs is created legally).
#[salsa::tracked]
fn rewrite_graph<'db>(db: &'db dyn fossil_base::Db, graph: MirGraph<'db>) -> MirGraph<'db> {
    rewrite(db, graph)
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
    let rec = Record::new(db, fields);
    Ty::new(db, TyKind::Record(rec))
}

fn source<'db>(db: &'db dyn fossil_base::Db, uri: &str, cols: &[&str]) -> Op<'db> {
    Op::Source {
        uri: SmolStr::from(uri),
        format: SourceFormat::Csv,
        row_type: string_record(db, cols),
    }
}

/// `col == "literal"` predicate over a column (free(p) = {col}).
fn eq_pred<'db>(db: &'db dyn fossil_base::Db, col: &str, lit: &str) -> Expr<'db> {
    let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
    Expr::BinOp {
        op: CmpOp::Eq,
        lhs: Box::new(Expr::ColRef {
            source: SmolStr::default(),
            column: SmolStr::from(col),
        }),
        rhs: Box::new(Expr::LitString(SmolStr::from(lit))),
        ty: bool_ty,
    }
}

/// `"lhs" == "rhs"` predicate over two string literals — statically decidable
/// (`true` when the literals are equal, `false` otherwise). Drives R8/R9.
fn lit_eq_pred<'db>(db: &'db dyn fossil_base::Db, lhs: &str, rhs: &str) -> Expr<'db> {
    let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
    Expr::BinOp {
        op: CmpOp::Eq,
        lhs: Box::new(Expr::LitString(SmolStr::from(lhs))),
        rhs: Box::new(Expr::LitString(SmolStr::from(rhs))),
        ty: bool_ty,
    }
}

const fn sink(input: usize) -> Op<'static> {
    Op::Sink {
        input,
        sink: SinkRef::GraphAr,
    }
}

// --------------------------------------------------------------------------
// Per-rule trigger patterns.
// --------------------------------------------------------------------------

/// R1 — filter(filter(s, p), q).
fn r1_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "status", "tier"]),
        Op::Filter {
            input: 0,
            pred: eq_pred(db, "status", "active"),
        },
        Op::Filter {
            input: 1,
            pred: eq_pred(db, "tier", "gold"),
        },
        sink(2),
    ]
}

/// R2 — project(project(s, c1), c2).
fn r2_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "name", "age", "city"]),
        Op::Project {
            input: 0,
            cols: vec![
                SmolStr::from("id"),
                SmolStr::from("name"),
                SmolStr::from("age"),
            ],
        },
        Op::Project {
            input: 1,
            cols: vec![SmolStr::from("name"), SmolStr::from("id")],
        },
        sink(2),
    ]
}

/// R3 — project(filter(s, p), c) with free(p) = {status} ⊆ {id, status}.
fn r3_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "status", "extra"]),
        Op::Filter {
            input: 0,
            pred: eq_pred(db, "status", "active"),
        },
        Op::Project {
            input: 1,
            cols: vec![SmolStr::from("id"), SmolStr::from("status")],
        },
        sink(2),
    ]
}

/// R4 — filter(union(s1, s2), p).
fn r4_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "a.csv", &["id", "status"]),
        source(db, "b.csv", &["id", "status"]),
        Op::Union { left: 0, right: 1 },
        Op::Filter {
            input: 2,
            pred: eq_pred(db, "status", "active"),
        },
        sink(3),
    ]
}

/// R5 — filter(join(s1, s2, c), p) with free(p) = {status} ⊆ schema(s1).
fn r5_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "status"]),
        source(db, "orders.csv", &["user_id", "total"]),
        Op::Join {
            left: 0,
            right: 1,
            on: eq_pred(db, "id", "user_id"),
            kind: JoinKind::Inner,
            left_name: SmolStr::from("users"),
            right_name: SmolStr::from("orders"),
        },
        Op::Filter {
            input: 2,
            pred: eq_pred(db, "status", "active"),
        },
        sink(3),
    ]
}

/// R6 — rename(s, a, b).
fn r6_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "name"]),
        Op::Rename {
            input: 0,
            old: SmolStr::from("name"),
            new: SmolStr::from("full_name"),
        },
        sink(1),
    ]
}

/// R7 — extend(s, "iri", "a" ++ "b") with a foldable Concat of two literals.
/// After R7 the Concat folds to `LitString("ab")`.
fn r7_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id"]),
        Op::Extend {
            input: 0,
            field: SmolStr::from("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::from("a"))),
                Box::new(Expr::LitString(SmolStr::from("b"))),
            ),
        },
        sink(1),
    ]
}

/// R8 — filter(s, "x" == "x"): a statically-TRUE predicate. After R8 the
/// Filter is dropped and the Sink rewires onto the Source.
fn r8_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "status"]),
        Op::Filter {
            input: 0,
            pred: lit_eq_pred(db, "x", "x"),
        },
        sink(1),
    ]
}

/// R9 — filter(s, "a" == "b"): a statically-FALSE predicate. After R9 the
/// Filter is replaced by `Op::Empty { schema: schema(s) }`.
fn r9_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "users.csv", &["id", "status"]),
        Op::Filter {
            input: 0,
            pred: lit_eq_pred(db, "a", "b"),
        },
        sink(1),
    ]
}

/// R10 — a `GroupBy` on `[region]` feeding a `GroupBy` on `[region, year]`:
/// nested group-bys. After R10 a single `GroupBy` with the fused key set
/// `[region, year]` remains (k₁ = {region} is a subset of the source schema).
fn r10_trigger<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        source(db, "sales.csv", &["region", "year", "amount"]),
        Op::GroupBy {
            input: 0,
            keys: vec![SmolStr::from("region")],
        },
        Op::GroupBy {
            input: 1,
            keys: vec![SmolStr::from("region"), SmolStr::from("year")],
        },
        sink(2),
    ]
}

// --------------------------------------------------------------------------
// The shared check: before snapshot, rewrite, after snapshot, idempotency.
// --------------------------------------------------------------------------

fn check_rule(rule: u8, name_before: &str, name_after: &str) {
    let db = db();
    let case = Case::new(&db, rule);
    let graph = trigger_graph(&db, case);
    insta::assert_snapshot!(name_before, format!("{:#?}", graph.ops(&db)));

    let rewritten = rewrite_graph(&db, graph);
    insta::assert_snapshot!(name_after, format!("{:#?}", rewritten.ops(&db)));

    // Idempotency: rewrite ∘ rewrite == rewrite (SC#2 / RESEARCH Pitfall 2).
    let twice = rewrite_graph(&db, rewritten);
    assert_eq!(
        twice.ops(&db),
        rewritten.ops(&db),
        "{name_after}: rewrite is not idempotent (the fixpoint did not converge)"
    );
}

#[test]
fn r1_filter_fusion() {
    check_rule(1, "r1_before", "r1_after");
}

#[test]
fn r2_project_fusion() {
    check_rule(2, "r2_before", "r2_after");
}

#[test]
fn r3_project_below_filter_pushdown() {
    check_rule(3, "r3_before", "r3_after");
}

#[test]
fn r4_filter_over_union_distribution() {
    check_rule(4, "r4_before", "r4_after");
}

#[test]
fn r5_filter_into_join_pushdown() {
    check_rule(5, "r5_before", "r5_after");
}

#[test]
fn r6_rename_expansion() {
    check_rule(6, "r6_before", "r6_after");
}

#[test]
fn r7_partial_eval_extend() {
    check_rule(7, "r7_before", "r7_after");
}

#[test]
fn r8_drop_statically_true_filter() {
    check_rule(8, "r8_before", "r8_after");
}

#[test]
fn r9_statically_false_filter_to_empty() {
    check_rule(9, "r9_before", "r9_after");

    // R9 must produce an `Op::Empty` (the statically-false Filter is gone).
    let db = db();
    let case = Case::new(&db, 9);
    let graph = trigger_graph(&db, case);
    let rewritten = rewrite_graph(&db, graph);
    assert!(
        rewritten
            .ops(&db)
            .iter()
            .any(|op| matches!(op, Op::Empty { .. })),
        "R9 must rewrite the statically-false Filter into an Op::Empty"
    );
    assert!(
        !rewritten
            .ops(&db)
            .iter()
            .any(|op| matches!(op, Op::Filter { .. })),
        "R9 must remove the Filter"
    );
}

#[test]
fn r10_group_by_fusion() {
    check_rule(10, "r10_before", "r10_after");
}
