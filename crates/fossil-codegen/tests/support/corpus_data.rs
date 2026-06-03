// Shared 30-mapping MIR-level golden corpus (SC#1).
//
// This file is `include!`d (NOT `mod`-imported) by both `tests/corpus.rs`
// (snapshots the generated SQL) and `tests/type_preservation.rs` (asserts
// `codegen(g) == codegen(erase_types(g))` over every entry — SC#3). Keeping the
// 30 graphs single-sourced means the two suites can never drift out of sync.
// Because it is textually included, it carries NO module doc comment, NO inner
// attributes, and NO imports — the including file provides `use` + `#![allow]`.
//
// Why a MIR-level corpus (ADR-0009): `HirExpr` has only four leaf forms, so 7 of
// the 11 operators are NOT reachable from `.fossil` source this milestone. The
// corpus hand-builds `MirGraph`s and drives them through the
// `fossil_codegen::codegen_graph` seam. The lone source-driven anchor is the
// locked `hello.fossil` SQL snapshot (`compile_hello.rs`), not duplicated here.
//
// Coverage (the 8 mandatory categories — plan 04-07 Task 1): every operator
// (`Source`/`Project`/`Extend`/`Rename`/`Filter`/`Join`×4 kinds/`Union`/
// `GroupBy`/`Aggregate`/`Distinct`×2/`TripleEmit`/`Sink` + `Op::Empty`); each
// R1–R10 rewrite OUTCOME (a trigger graph is built, run through
// `fossil_mir::rewrite`, and the OPTIMISED graph is the snapshot subject);
// multi-`TripleEmit` mappings; the SC#4 named-assertion case.

/// How a corpus entry's op vec is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Build {
    /// The op vec is the snapshot subject verbatim (direct-MIR construction).
    Direct,
    /// The op vec is a pre-rewrite TRIGGER; the corpus runs
    /// [`fossil_mir::rewrite`] over it and the OUTCOME is the snapshot subject
    /// (locks the R1–R10 result SQL).
    RewriteOutcome,
}

/// One corpus mapping: a name (snapshot key) + a builder + provenance flags.
struct Entry {
    name: &'static str,
    build: Build,
    /// `true` if the native-execution tier runs this entry's SQL on an on-disk
    /// fixture CSV (`corpus_exec.rs`). The remaining entries are snapshot-only
    /// (their data is source-unreachable / synthetic — ADR-0009).
    executable: bool,
    make: fn(&dyn fossil_base::Db) -> Vec<Op<'_>>,
}

// ---------------------------------------------------------------------------
// Op-construction helpers (shared with the historical codegen_ops.rs shapes).
// ---------------------------------------------------------------------------

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
        binding: SmolStr::from(uri),
    }
}

fn bool_ty<'db>(db: &'db dyn fossil_base::Db) -> Ty<'db> {
    Ty::new(db, TyKind::Primitive(Primitive::Bool))
}

fn int_ty<'db>(db: &'db dyn fossil_base::Db) -> Ty<'db> {
    Ty::new(db, TyKind::Primitive(Primitive::Integer))
}

fn colref(source: &str, column: &str) -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::from(source),
        column: SmolStr::from(column),
    }
}

fn iri_col() -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::default(),
        column: SmolStr::new_static("iri"),
    }
}

/// `TripleEmit(subject = iri, predicate, object = <obj_src>.name)` + `Sink`.
fn emit_and_sink<'db>(input: usize, obj_src: &str) -> Vec<Op<'db>> {
    vec![
        Op::TripleEmit {
            input,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref(obj_src, "name"),
            graph: None,
        },
        Op::Sink {
            input: input + 1,
            sink: SinkRef::GraphAr,
        },
    ]
}

// ---------------------------------------------------------------------------
// Single-input operator graphs.
// ---------------------------------------------------------------------------

fn project_two_cols(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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

fn extend_concat(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("https://example.org/"))),
                Box::new(colref("users", "id")),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref("users", "name"),
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn rename_col(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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

fn filter_eq(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name", "status"]),
        Op::Filter {
            input: 0,
            pred: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(colref("users", "status")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static("active"))),
                ty: bool_ty(db),
            },
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn distinct_all(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Distinct { input: 0, by: None },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

fn distinct_by(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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

fn union_two_sources(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/a.csv", &["id", "name"]),
        source(db, "examples/b.csv", &["id", "name"]),
        Op::Union { left: 0, right: 1 },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

fn empty_relation(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Empty {
            schema: vec![SmolStr::new_static("id"), SmolStr::new_static("name")],
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

// ---------------------------------------------------------------------------
// Multi-input / aggregating operator graphs.
// ---------------------------------------------------------------------------

fn join_graph(db: &dyn fossil_base::Db, kind: JoinKind) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/orders.csv", &["id", "user_id"]),
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Join {
            left: 0,
            right: 1,
            on: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(colref("orders", "user_id")),
                rhs: Box::new(colref("users", "id")),
                ty: bool_ty(db),
            },
            kind,
            left_name: SmolStr::new_static("orders"),
            right_name: SmolStr::new_static("users"),
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

fn join_inner(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    join_graph(db, JoinKind::Inner)
}
fn join_left_outer(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    join_graph(db, JoinKind::LeftOuter)
}
fn join_right_outer(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    join_graph(db, JoinKind::RightOuter)
}
fn join_full_outer(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    join_graph(db, JoinKind::Full)
}

fn group_by_count(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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
                ty: int_ty(db),
            }],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

fn aggregate_sum(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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
                ty: int_ty(db),
            }],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

fn aggregate_min_max(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    // Min + Max + Avg over a grouped relation (full AggFn coverage with
    // group_by_count's Count and aggregate_sum's Sum).
    let mut ops = vec![
        source(db, "examples/orders.csv", &["user_id", "amount"]),
        Op::GroupBy {
            input: 0,
            keys: vec![SmolStr::new_static("user_id")],
        },
        Op::Aggregate {
            input: 1,
            aggs: vec![
                AggSpec {
                    out_field: SmolStr::new_static("lo"),
                    agg_fn: AggFn::Min,
                    in_field: SmolStr::new_static("amount"),
                    ty: int_ty(db),
                },
                AggSpec {
                    out_field: SmolStr::new_static("hi"),
                    agg_fn: AggFn::Max,
                    in_field: SmolStr::new_static("amount"),
                    ty: int_ty(db),
                },
                AggSpec {
                    out_field: SmolStr::new_static("mean"),
                    agg_fn: AggFn::Avg,
                    in_field: SmolStr::new_static("amount"),
                    ty: int_ty(db),
                },
            ],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

// ---------------------------------------------------------------------------
// Multi-TripleEmit + named-assertion graphs.
// ---------------------------------------------------------------------------

fn multi_triple_emit_sink(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    vec![
        source(db, "examples/users.csv", &["id", "name", "email"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("https://example.org/"))),
                Box::new(colref("users", "id")),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref("users", "name"),
            graph: None,
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/email"),
            object: colref("users", "email"),
            graph: None,
        },
        Op::Sink {
            input: 3,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn triple_emit_with_graph(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    // RDF 1.2 named-graph quad — `graph` is Some(..). Codegen flattens to the
    // triple projection this milestone (Phase 9 wires quad sinks).
    vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("https://example.org/"))),
                Box::new(colref("users", "id")),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref("users", "name"),
            graph: Some(SmolStr::new_static("https://example.org/g")),
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn assert_iri_template_unbound(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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
                    inner: Box::new(colref("users", "id")),
                }),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref("users", "name"),
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn extend_call(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    // `Expr::Call` passthrough (stdlib → SQL mapping is Phase 5).
    vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("upper_name"),
            expr: Expr::Call {
                func: SmolStr::new_static("upper"),
                args: vec![colref("users", "name")],
                ty: Ty::new(db, TyKind::Primitive(Primitive::String)),
            },
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref("step_1", "upper_name"),
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

fn filter_litbool(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    // A Filter whose predicate is a non-foldable column comparison OR'd with a
    // literal — exercises `Expr::LitBool` rendering WITHOUT tripping R8/R9
    // (the OR keeps it non-statically-decidable so it survives rewrite).
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Filter {
            input: 0,
            pred: Expr::BinOp {
                op: CmpOp::Or,
                lhs: Box::new(Expr::BinOp {
                    op: CmpOp::Ne,
                    lhs: Box::new(colref("users", "name")),
                    rhs: Box::new(Expr::LitString(SmolStr::new_static(""))),
                    ty: bool_ty(db),
                }),
                rhs: Box::new(Expr::LitBool(false)),
                ty: bool_ty(db),
            },
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

// ---------------------------------------------------------------------------
// R1–R10 rewrite-OUTCOME trigger graphs.
//
// Each returns the PRE-rewrite trigger; the corpus runs `rewrite` and snapshots
// the OUTCOME. The trigger pattern is documented per entry.
// ---------------------------------------------------------------------------

/// R1 — filter fusion trigger: `filter(filter(s, p), q)` → fused `AND`.
fn r1_filter_fusion(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name", "status"]),
        Op::Filter {
            input: 0,
            pred: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(colref("users", "status")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static("active"))),
                ty: bool_ty(db),
            },
        },
        Op::Filter {
            input: 1,
            pred: Expr::BinOp {
                op: CmpOp::Ne,
                lhs: Box::new(colref("users", "name")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static(""))),
                ty: bool_ty(db),
            },
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

/// R2 — project fusion trigger: `project(project(s, c1), c2)` → `c1 ∩ c2`.
fn r2_project_fusion(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name", "status"]),
        Op::Project {
            input: 0,
            cols: vec![
                SmolStr::new_static("id"),
                SmolStr::new_static("name"),
                SmolStr::new_static("status"),
            ],
        },
        Op::Project {
            input: 1,
            cols: vec![SmolStr::new_static("id"), SmolStr::new_static("name")],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

/// R3 — project-below-filter pushdown trigger (`free(p) ⊆ c`).
fn r3_project_pushdown(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name", "status"]),
        Op::Filter {
            input: 0,
            pred: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(colref("users", "id")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static("1"))),
                ty: bool_ty(db),
            },
        },
        Op::Project {
            input: 1,
            cols: vec![SmolStr::new_static("id"), SmolStr::new_static("name")],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

/// R4 — filter-over-union distribution trigger.
fn r4_filter_union(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/a.csv", &["id", "name"]),
        source(db, "examples/b.csv", &["id", "name"]),
        Op::Union { left: 0, right: 1 },
        Op::Filter {
            input: 2,
            pred: Expr::BinOp {
                op: CmpOp::Ne,
                lhs: Box::new(colref("step_2", "name")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static(""))),
                ty: bool_ty(db),
            },
        },
    ];
    ops.extend(emit_and_sink(3, "step_3"));
    ops
}

/// R5 — filter-into-join pushdown trigger (`free(p) ⊆ schema(s1)`).
fn r5_filter_join(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/orders.csv", &["id", "user_id"]),
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Join {
            left: 0,
            right: 1,
            on: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(colref("orders", "user_id")),
                rhs: Box::new(colref("users", "id")),
                ty: bool_ty(db),
            },
            kind: JoinKind::Inner,
            left_name: SmolStr::new_static("orders"),
            right_name: SmolStr::new_static("users"),
        },
        Op::Filter {
            input: 2,
            pred: Expr::BinOp {
                op: CmpOp::Eq,
                lhs: Box::new(colref("orders", "user_id")),
                rhs: Box::new(Expr::LitString(SmolStr::new_static("1"))),
                ty: bool_ty(db),
            },
        },
    ];
    ops.extend(emit_and_sink(3, "step_3"));
    ops
}

/// R6 — rename expansion trigger: `rename` → `extend(project(...))`.
fn r6_rename_expansion(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
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

/// R7 — partial-eval an `Extend` expr: `"a" || "b"` folds to `"ab"`.
fn r7_const_fold_extend(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("https://example.org/"))),
                Box::new(Expr::LitString(SmolStr::new_static("user"))),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_col(),
            predicate: SmolStr::new_static("https://example.org/name"),
            object: colref("users", "name"),
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

/// R8 — drop a statically-true `Filter` (`true OR x` ⇒ true).
fn r8_drop_true_filter(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Filter {
            input: 0,
            pred: Expr::LitBool(true),
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

/// R9 — statically-false `Filter` → `Op::Empty { schema }`.
fn r9_empty_from_false_filter(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "name"]),
        Op::Filter {
            input: 0,
            pred: Expr::LitBool(false),
        },
    ];
    ops.extend(emit_and_sink(1, "step_1"));
    ops
}

/// R10 — group-by fusion: `group_by(group_by(s, k1), k2)` → `k1 ∪ k2`.
fn r10_groupby_fusion(db: &dyn fossil_base::Db) -> Vec<Op<'_>> {
    let mut ops = vec![
        source(db, "examples/users.csv", &["id", "country", "city"]),
        Op::GroupBy {
            input: 0,
            keys: vec![SmolStr::new_static("country")],
        },
        Op::GroupBy {
            input: 1,
            keys: vec![SmolStr::new_static("city")],
        },
    ];
    ops.extend(emit_and_sink(2, "step_2"));
    ops
}

// ---------------------------------------------------------------------------
// The canonical 30-mapping corpus table.
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const CORPUS: &[Entry] = &[
    // -- single-input ops (executable subset reads examples/users.csv) -------
    Entry { name: "project_two_cols",     build: Build::Direct, executable: true,  make: project_two_cols },
    Entry { name: "extend_concat",        build: Build::Direct, executable: true,  make: extend_concat },
    Entry { name: "rename_col",           build: Build::Direct, executable: true,  make: rename_col },
    Entry { name: "filter_eq",            build: Build::Direct, executable: false, make: filter_eq },
    Entry { name: "filter_or_litbool",    build: Build::Direct, executable: false, make: filter_litbool },
    Entry { name: "distinct_all",         build: Build::Direct, executable: true,  make: distinct_all },
    Entry { name: "distinct_by",          build: Build::Direct, executable: true,  make: distinct_by },
    Entry { name: "union_two_sources",    build: Build::Direct, executable: false, make: union_two_sources },
    Entry { name: "empty_relation",       build: Build::Direct, executable: false, make: empty_relation },
    // -- multi-input / aggregating ops --------------------------------------
    Entry { name: "join_inner",           build: Build::Direct, executable: false, make: join_inner },
    Entry { name: "join_left_outer",      build: Build::Direct, executable: false, make: join_left_outer },
    Entry { name: "join_right_outer",     build: Build::Direct, executable: false, make: join_right_outer },
    Entry { name: "join_full_outer",      build: Build::Direct, executable: false, make: join_full_outer },
    Entry { name: "group_by_count",       build: Build::Direct, executable: false, make: group_by_count },
    Entry { name: "aggregate_sum",        build: Build::Direct, executable: false, make: aggregate_sum },
    Entry { name: "aggregate_min_max_avg", build: Build::Direct, executable: false, make: aggregate_min_max },
    // -- Expr coverage (Call / LitBool) + named-graph quad ------------------
    Entry { name: "extend_call",          build: Build::Direct, executable: false, make: extend_call },
    Entry { name: "triple_emit_with_graph", build: Build::Direct, executable: true, make: triple_emit_with_graph },
    // -- multi-TripleEmit + named assertion (SC#4) --------------------------
    Entry { name: "multi_triple_emit_sink", build: Build::Direct, executable: true, make: multi_triple_emit_sink },
    Entry { name: "assert_iri_template_unbound", build: Build::Direct, executable: true, make: assert_iri_template_unbound },
    // -- R1–R10 rewrite OUTCOMES --------------------------------------------
    Entry { name: "r1_filter_fusion",     build: Build::RewriteOutcome, executable: false, make: r1_filter_fusion },
    Entry { name: "r2_project_fusion",    build: Build::RewriteOutcome, executable: false, make: r2_project_fusion },
    Entry { name: "r3_project_pushdown",  build: Build::RewriteOutcome, executable: false, make: r3_project_pushdown },
    Entry { name: "r4_filter_union",      build: Build::RewriteOutcome, executable: false, make: r4_filter_union },
    Entry { name: "r5_filter_join",       build: Build::RewriteOutcome, executable: false, make: r5_filter_join },
    Entry { name: "r6_rename_expansion",  build: Build::RewriteOutcome, executable: false, make: r6_rename_expansion },
    Entry { name: "r7_const_fold_extend", build: Build::RewriteOutcome, executable: true,  make: r7_const_fold_extend },
    Entry { name: "r8_drop_true_filter",  build: Build::RewriteOutcome, executable: true,  make: r8_drop_true_filter },
    Entry { name: "r9_empty_from_false_filter", build: Build::RewriteOutcome, executable: false, make: r9_empty_from_false_filter },
    Entry { name: "r10_groupby_fusion",   build: Build::RewriteOutcome, executable: false, make: r10_groupby_fusion },
];

/// Build the op vec for one corpus entry, applying [`rewrite`] for the
/// `RewriteOutcome` entries so the snapshot subject is the OPTIMISED graph.
fn corpus_entry_ops<'db>(db: &'db dyn fossil_base::Db, entry: &Entry) -> Vec<Op<'db>> {
    let ops = (entry.make)(db);
    match entry.build {
        Build::Direct => ops,
        Build::RewriteOutcome => {
            let rewritten = rewrite(db, MirGraph::new(db, ops));
            rewritten.ops(db).clone()
        }
    }
}
