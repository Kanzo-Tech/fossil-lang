//! Scalar stdlib lowering — DIRECT `Expr::Call` construction; no surface call
//! syntax in v0.1 (ADR-0009).
//!
//! There is NO surface `clean.trim(.name)` call form in Fossil v0.1 (`HirExpr`
//! has only four leaf forms — no pipeline / call syntax; the reachability gap,
//! ADR-0009). So the registry-driven `Expr::Call` lowering (05-02) is exercised
//! exactly as Phase 4 tested the 7 source-unreachable ops: by hand-building a
//! [`MirGraph`] whose `Extend` carries an [`Expr::Call`] for a representative
//! function from each namespace, driving it through the
//! [`fossil_codegen::codegen_graph`] seam, and snapshotting the emitted SQL.
//!
//! Each case is `Source(users[id,name,raw]) Extend("val", Call(...)) TripleEmit
//! Sink`: the computed `val` is the COPY's `object` projection, so the
//! lowered Call expression appears verbatim in the snapshot. The cases cover
//! every render mode the registry produces:
//!
//! - `Builtin{duckdb_name}` — `clean.trim`/`clean.lower`/`str.length`/
//!   `math.abs`/`math.round`/`anon.hash` (`anon.hash` → `sha256(...)`),
//!   `parse.datetime` (→ `strptime(...)`, a `DuckDB` builtin).
//! - `Inline(form)` — `parse.integer` (`CAST(... AS BIGINT)`), `parse.json`
//!   (`json_extract`), `core.literal` (`Identity`), `anon.redact`
//!   (`LiteralStr`).
//! - `Udf{udf_name}` — `clean.slug`/`validate.email`/`clean.normalize_unicode`/
//!   `clean.strip_html` render their `fossil_*` UDF marker so the native
//!   runtime (05-03) resolves it and the playground disables it.
//!
//! A no-leak assert (below) guards that no snapshot contains `Unknown(` or
//! `InferenceId` debug text (RESEARCH Pitfall 5 / STATE.md no-leak rule).

#![cfg(not(target_arch = "wasm32"))]
// The `'db` lifetime is written explicitly on the tracked seam (documenting the
// salsa frame) and the op-builder helpers, uniform with codegen_ops.rs; clippy's
// elision lint is noise for this scaffolding.
#![allow(clippy::elidable_lifetime_names)]

use std::sync::Arc;

use fossil_codegen::codegen_graph;
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{Expr, Op, SinkRef, SourceFormat};
use smol_str::SmolStr;

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

/// Selects which stdlib-call case [`codegen_case`] builds.
#[salsa::input]
struct Case {
    which: u8,
}

/// Build the chosen stdlib-call `MirGraph` and codegen it — all inside a tracked
/// frame so `MirGraph::new` / `Ty` interning are legal and the
/// `fossil_codegen::codegen_graph` query runs normally.
#[salsa::tracked]
fn codegen_case<'db>(db: &'db dyn fossil_base::Db, case: Case) -> fossil_codegen::SqlPlan<'db> {
    // The carried `Call.ty` is a `String` placeholder: codegen is type-agnostic
    // (ADR-0013) and the Call arm never reads `ty`, so any concrete interned Ty
    // serves. Built here where `db` is in scope (mirrors codegen_ops.rs).
    let ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let call = |func: &str, args: &[Expr<'db>]| Expr::Call {
        func: SmolStr::from(func),
        args: args.to_vec(),
        ty,
    };
    let call_expr = match case.which(db) {
        // ── pure_sql Builtin ─────────────────────────────────────────────
        0 => call("clean.trim", &[col("raw")]),
        1 => call("clean.lower", &[col("raw")]),
        2 => call("str.length", &[col("raw")]),
        3 => call("math.abs", &[col("raw")]),
        4 => call("math.round", &[col("raw")]),
        5 => call("anon.hash", &[col("raw")]),
        // ── pure_sql Inline ──────────────────────────────────────────────
        6 => call("parse.integer", &[col("raw")]),
        7 => call("parse.datetime", &[col("raw"), lit("%Y-%m-%d")]),
        8 => call("parse.json", &[col("raw"), lit("$.name")]),
        9 => call("core.literal", &[col("raw"), col("id")]),
        10 => call("anon.redact", &[col("raw")]),
        // ── native_udf_only ──────────────────────────────────────────────
        11 => call("clean.slug", &[col("raw")]),
        12 => call("validate.email", &[col("raw")]),
        13 => call("clean.normalize_unicode", &[col("raw"), lit("NFC")]),
        14 => call("clean.strip_html", &[col("raw")]),
        other => panic!("unknown case {other}"),
    };
    codegen_graph(db, MirGraph::new(db, call_graph(db, call_expr)))
}

fn sql_for(which: u8) -> String {
    let db = db();
    let case = Case::new(&db, which);
    let plan = codegen_case(&db, case);
    plan.sql(&db).clone()
}

// --------------------------------------------------------------------------
// Expr / Op construction helpers.
// --------------------------------------------------------------------------

/// An unqualified column reference `<col>` (qualified on the source view by
/// codegen's `default_source`).
fn col(name: &str) -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::default(),
        column: SmolStr::from(name),
    }
}

/// A string literal `'<value>'`.
fn lit(value: &str) -> Expr<'static> {
    Expr::LitString(SmolStr::from(value))
}

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

/// `Source(users[id,name,raw]) Extend("val", <call>) TripleEmit(subject=.id,
/// predicate, object=.val) Sink`.
///
/// The `Extend` buffers the call expr; the `TripleEmit` `object` references the
/// buffered `val`, so the lowered Call SQL is inlined into the COPY's inner
/// SELECT — the snapshot subject.
fn call_graph<'db>(db: &'db dyn fossil_base::Db, call_expr: Expr<'db>) -> Vec<Op<'db>> {
    vec![
        Op::Source {
            uri: SmolStr::new_static("examples/users.csv"),
            format: SourceFormat::Csv,
            row_type: string_record(db, &["id", "name", "raw"]),
            binding: SmolStr::new_static("examples/users.csv"),
        },
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("val"),
            expr: call_expr,
        },
        Op::TripleEmit {
            input: 1,
            subject: Expr::ColRef {
                source: SmolStr::new_static("users"),
                column: SmolStr::new_static("id"),
            },
            predicate: SmolStr::new_static("https://example.org/value"),
            object: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("val"),
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
// Snapshots — one per representative function / render mode.
// --------------------------------------------------------------------------

#[test]
fn clean_trim_builtin() {
    insta::assert_snapshot!("clean_trim_builtin", sql_for(0));
}

#[test]
fn clean_lower_builtin() {
    insta::assert_snapshot!("clean_lower_builtin", sql_for(1));
}

#[test]
fn str_length_builtin() {
    insta::assert_snapshot!("str_length_builtin", sql_for(2));
}

#[test]
fn math_abs_builtin() {
    insta::assert_snapshot!("math_abs_builtin", sql_for(3));
}

#[test]
fn math_round_builtin() {
    insta::assert_snapshot!("math_round_builtin", sql_for(4));
}

#[test]
fn anon_hash_sha256_builtin() {
    insta::assert_snapshot!("anon_hash_sha256_builtin", sql_for(5));
}

#[test]
fn parse_integer_cast_inline() {
    insta::assert_snapshot!("parse_integer_cast_inline", sql_for(6));
}

#[test]
fn parse_datetime_strptime() {
    insta::assert_snapshot!("parse_datetime_strptime", sql_for(7));
}

#[test]
fn parse_json_extract_inline() {
    insta::assert_snapshot!("parse_json_extract_inline", sql_for(8));
}

#[test]
fn core_literal_identity_inline() {
    insta::assert_snapshot!("core_literal_identity_inline", sql_for(9));
}

#[test]
fn anon_redact_literal_inline() {
    insta::assert_snapshot!("anon_redact_literal_inline", sql_for(10));
}

#[test]
fn clean_slug_udf_marker() {
    insta::assert_snapshot!("clean_slug_udf_marker", sql_for(11));
}

#[test]
fn validate_email_udf_marker() {
    insta::assert_snapshot!("validate_email_udf_marker", sql_for(12));
}

#[test]
fn clean_normalize_unicode_udf_marker() {
    insta::assert_snapshot!("clean_normalize_unicode_udf_marker", sql_for(13));
}

#[test]
fn clean_strip_html_udf_marker() {
    insta::assert_snapshot!("clean_strip_html_udf_marker", sql_for(14));
}

// --------------------------------------------------------------------------
// No-leak guard (RESEARCH Pitfall 5 / STATE.md): no rendered stdlib-call SQL
// may contain `TyKind::Unknown(` or `InferenceId` debug text. The Call arm
// reads only `func`/`args`, never `ty`, so this must hold for every case.
// --------------------------------------------------------------------------

#[test]
fn no_call_sql_leaks_unknown_or_inference_id() {
    for which in 0u8..=14 {
        let sql = sql_for(which);
        assert!(
            !sql.contains("Unknown("),
            "TyKind::Unknown leaked into call SQL (case {which}):\n{sql}"
        );
        assert!(
            !sql.contains("InferenceId"),
            "InferenceId leaked into call SQL (case {which}):\n{sql}"
        );
    }
}
