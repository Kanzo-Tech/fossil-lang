//! MIR → `DuckDB` SQL lowering — Phase 1 hand-formatted templates.
//!
//! Per RESEARCH.md Pitfall 6, do **not** round-trip the full `DuckDB`
//! `COPY (...) TO '...' (FORMAT PARQUET)` statement through `sqlparser` — the
//! 0.59 release does not preserve the parenthesised options form on
//! `Statement::Copy::to_string()`. The hybrid pattern documented in the
//! research is: build the inner `SELECT` via sqlparser AST, then wrap it in
//! a hand-formatted COPY template.
//!
//! For Phase 1 the inner `SELECT` is trivially simple (3 select-list items,
//! 1 `FROM` clause), so even the inner-SELECT path is hand-formatted. The
//! `sqlparser` workspace dep is wired in [`Cargo.toml`] for Phase 4 readiness
//! when the 30-mapping corpus warrants AST construction; Phase 1 deliberately
//! exercises zero `sqlparser` API surface (de-risk).
//!
//! # Public Salsa query signature (Phase 2-9 contract — locked)
//!
//! ```ignore
//! #[salsa::tracked]
//! pub struct SqlPlan<'db> {
//!     #[returns(ref)] pub sql: String,
//!     #[returns(ref)] pub manifest_yaml: String,
//! }
//!
//! #[salsa::tracked]
//! pub fn codegen_sql<'db>(
//!     db: &'db dyn fossil_base::Db,
//!     mapping: fossil_hir::MappingLoc<'db>,
//! ) -> SqlPlan<'db>;
//! ```

use std::fmt::Write as _;

use fossil_hir::MappingLoc;
use fossil_mir::op::CmpOp;
use fossil_mir::{Expr, Op, lower_to_mir};

use crate::manifest::manifest_template;

#[salsa::tracked]
pub struct SqlPlan<'db> {
    #[returns(ref)]
    pub sql: String,
    #[returns(ref)]
    pub manifest_yaml: String,
}

/// Lower one `MappingLoc` to a [`SqlPlan`] containing the `DuckDB` SQL script
/// (`CREATE VIEW` + `COPY`) and the `GraphAr` manifest YAML.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn codegen_sql<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> SqlPlan<'db> {
    let mir = lower_to_mir(db, mapping);
    let mut sql = String::new();

    // Per-mapping codegen state. Phase 4 promotes these to a proper visitor
    // when the 11-operator algebra needs richer threading (e.g. Project
    // pushdown, Join's two-input shape).
    let mut source_view: Option<String> = None;
    let mut extends: Vec<(String, String)> = Vec::new();
    // (subject_expr, predicate_iri, object_expr, source_view) — buffered by
    // TripleEmit and consumed by Sink so the COPY wrapper sees the resolved
    // subject expression rather than a column reference into a non-existent
    // view.
    let mut emit: Option<(String, String, String, String)> = None;

    for op in mir.ops(db) {
        match op {
            Op::Source {
                uri,
                format: _,
                row_type: _,
            } => {
                let view_name = derive_view_name(uri);
                writeln!(
                    sql,
                    "CREATE VIEW {view_name} AS\nSELECT * FROM read_csv_auto('{uri}', sample_size=-1);"
                )
                .expect("writing to a String never fails");
                source_view = Some(view_name);
            }
            Op::Extend {
                input: _,
                field,
                expr,
            } => {
                let default_source = source_view.as_deref().unwrap_or("source");
                let expr_sql = render_expr(expr, default_source);
                extends.push((field.to_string(), expr_sql));
            }
            Op::TripleEmit {
                input: _,
                subject,
                predicate,
                object,
                graph: _,
            } => {
                let view = source_view.clone().unwrap_or_else(|| "source".to_string());
                // Resolve `subject` / `object`. A bare `ColRef { source: "",
                // column }` that names a buffered Extend (e.g. the shared
                // `iri` column) is substituted with that Extend's rendered SQL
                // so the COPY wrapper sees the resolved expression rather than
                // a reference into a non-existent view column. This keeps the
                // hello.fossil output byte-identical with Phase 1.
                let subject_expr = render_emit_operand(subject, &extends, &view);
                let object_expr = render_emit_operand(object, &extends, &view);
                emit = Some((subject_expr, predicate.to_string(), object_expr, view));
            }
            Op::Sink { input: _, sink: _ } => {
                if let Some((subject, predicate, object, view)) = &emit {
                    writeln!(
                        sql,
                        "COPY (\n    SELECT\n        {subject} AS subject,\n        '{predicate}' AS predicate,\n        {object} AS object\n    FROM {view}\n) TO 'output.parquet' (FORMAT PARQUET);"
                    )
                    .expect("writing to a String never fails");
                }
            }
            // The remaining 7 operators + Empty are NOT produced by source
            // lowering this phase (ADR-0009 — they are exercised via direct
            // MirGraph construction); their codegen lands in plans 04-04/04-05.
            // The corpus does not hit these arms yet, so a clearly-marked empty
            // fragment keeps the match exhaustive without a panic in the
            // production path.
            Op::Project { .. }
            | Op::Rename { .. }
            | Op::Filter { .. }
            | Op::Join { .. }
            | Op::Union { .. }
            | Op::GroupBy { .. }
            | Op::Aggregate { .. }
            | Op::Distinct { .. }
            | Op::Empty { .. } => {
                // TODO(04-04/04-05): real codegen for the non-source-reachable
                // operators. No corpus test exercises these arms this phase.
            }
        }
    }

    SqlPlan::new(db, sql, manifest_template())
}

/// Render a `TripleEmit` subject/object operand to SQL. A bare `ColRef` whose
/// `source` is empty and whose `column` names a buffered [`Op::Extend`] is
/// substituted with that Extend's rendered expression (so the shared `iri`
/// subject column resolves to its full template). Otherwise the operand renders
/// via [`render_expr`] qualified by the source view.
fn render_emit_operand(operand: &Expr<'_>, extends: &[(String, String)], view: &str) -> String {
    if let Expr::ColRef { source, column } = operand
        && source.is_empty()
        && let Some((_, expr_sql)) = extends.iter().find(|(name, _)| name == column.as_str())
    {
        return expr_sql.clone();
    }
    render_expr(operand, view)
}

/// Render an [`Expr`] tree to a `DuckDB` SQL fragment.
///
/// `default_source` is used as the qualifier for [`Expr::ColRef`] entries whose
/// `source` field is empty (the shared `iri` subject reference, and Phase 4's
/// multi-source joins where a `ColRef` may name a binding the codegen must
/// disambiguate).
///
/// `LitString` / `ColRef` / `Concat` render byte-identically with Phase 1. The
/// Phase 4..6 additions (`LitBool` / `Call` / `BinOp` / `Assert`) render as:
/// - `LitBool` → `TRUE` / `FALSE`
/// - `Call` → passthrough `func(arg, ...)` (stdlib → SQL mapping is Phase 5)
/// - `BinOp` → `lhs <op> rhs` ([`CmpOp`] → SQL operator)
/// - `Assert` → its `inner` (no-op until plan 04-06 fills the `CASE WHEN` shape)
fn render_expr(expr: &Expr<'_>, default_source: &str) -> String {
    match expr {
        Expr::LitString(s) => format!("'{}'", s.replace('\'', "''")),
        Expr::LitBool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Expr::ColRef { source, column } => {
            let qualifier = if source.is_empty() {
                default_source
            } else {
                source.as_str()
            };
            format!("{qualifier}.{column}")
        }
        Expr::Concat(lhs, rhs) => {
            format!(
                "{} || {}",
                render_expr(lhs, default_source),
                render_expr(rhs, default_source),
            )
        }
        Expr::Call { func, args, ty: _ } => {
            let rendered: Vec<String> = args
                .iter()
                .map(|a| render_expr(a, default_source))
                .collect();
            format!("{func}({})", rendered.join(", "))
        }
        Expr::BinOp {
            op,
            lhs,
            rhs,
            ty: _,
        } => {
            format!(
                "{} {} {}",
                render_expr(lhs, default_source),
                cmp_op_sql(*op),
                render_expr(rhs, default_source),
            )
        }
        // Plan 04-06 fills the named-runtime-assertion `CASE WHEN` shape; until
        // then `Assert` is a transparent wrapper.
        Expr::Assert {
            name: _,
            span_line: _,
            inner,
        } => render_expr(inner, default_source),
    }
}

/// Map a [`CmpOp`] to its `DuckDB` SQL operator spelling.
const fn cmp_op_sql(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "<>",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::And => "AND",
        CmpOp::Or => "OR",
    }
}

/// Derive a SQL view name from a source URI: `examples/users.csv` → `users`.
///
/// Phase 1 uses the filename stem. Phase 4 may add explicit binding-name
/// overrides when multiple sources share a stem (`users.csv` from two
/// directories, etc.).
fn derive_view_name(uri: &str) -> String {
    std::path::Path::new(uri)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("source")
        .to_string()
}
