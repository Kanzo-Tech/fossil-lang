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
use fossil_mir::{Expr, MirGraph, Op, lower_to_mir};

use crate::ast;
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
///
/// Thin wrapper over the [`codegen_graph`] test seam: lowers the mapping to its
/// [`MirGraph`] then defers all MIR-walking to `codegen_graph`. Keeping the walk
/// behind a `MirGraph`-keyed helper lets the `tests/codegen_ops.rs` suite drive
/// directly-constructed graphs for the source-unreachable operators (ADR-0009)
/// without needing a `.fossil` source.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn codegen_sql<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> SqlPlan<'db> {
    codegen_graph(db, lower_to_mir(db, mapping))
}

/// The MIR → `DuckDB` SQL walk, keyed on a [`MirGraph`] rather than a mapping.
///
/// This is the codegen test seam (RESEARCH Code Examples / plan note): the 7
/// source-unreachable operators (`Project` / `Rename` / `Filter` / `Distinct` /
/// `Union` / `Empty`, + `Join` / `GroupBy` / `Aggregate` in plan 04-05) are
/// exercised by hand-building a `MirGraph` and calling
/// [`codegen_graph_for_test`] (the public test wrapper) — no surface syntax
/// required.
///
/// # Relation referencing
///
/// Each op produces a *relation reference* (`rel_ref[i]`) consumed by its
/// downstream ops:
/// - `Source` — a `CREATE VIEW` statement; its reference is the bare view name.
/// - the single-input SELECT ops (`Project` / `Rename` / `Filter` / `Distinct`
///   / `Empty`) and `Union` — build their SELECT body via the [`crate::ast`]
///   helpers; their reference is the body wrapped as `(<body>) AS step_<i>`
///   (nested subquery — chosen over a `WITH` CTE chain for self-containment and
///   snapshot stability; documented in ADR-0012).
/// - `Extend` — buffered, NOT emitted as a standalone SELECT, so its reference
///   is a *passthrough* of its input's reference. This is the byte-identical
///   `hello.fossil` path: the `iri` Extend collapses into the COPY's inner
///   SELECT exactly as in Phase 1.
///
/// `TripleEmit` / `Sink` keep the Phase 1 hand-wrapped COPY (Pitfall 1).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn codegen_graph<'db>(db: &'db dyn fossil_base::Db, mir: MirGraph<'db>) -> SqlPlan<'db> {
    let ops = mir.ops(db);
    let mut sql = String::new();

    // The relation reference for each op index (the FROM-able SQL a downstream
    // op should read). `None` for terminal ops (TripleEmit/Sink) that produce
    // no consumable relation.
    let mut rel_ref: Vec<Option<String>> = vec![None; ops.len()];

    // For a buffered Extend, the relation it reads FROM (so a consuming
    // TripleEmit collapses into a COPY over that same relation rather than a
    // redundant subquery — the byte-identical hello.fossil path).
    let mut collapse_from: Vec<Option<String>> = vec![None; ops.len()];

    // Buffered Extends (field name → rendered expr SQL). An Extend feeding a
    // TripleEmit collapses into the COPY's inner SELECT (byte-identical
    // hello.fossil); it never becomes a standalone SELECT.
    let mut extends: Vec<(String, String)> = Vec::new();
    // (subject_expr, predicate_iri, object_expr, from_relation) — buffered by
    // TripleEmit and consumed by Sink.
    let mut emit: Option<(String, String, String, String)> = None;

    for (idx, op) in ops.iter().enumerate() {
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
                rel_ref[idx] = Some(view_name);
            }
            Op::Extend { input, field, expr } => {
                // Buffer the computed field. The qualifier for unqualified
                // ColRefs is the input relation's view name (the source view in
                // the hello.fossil path).
                let from = input_relation(&rel_ref, *input);
                let expr_sql = render_expr(expr, &from);
                extends.push((field.to_string(), expr_sql.clone()));
                // Dual exposure (ADR-0012):
                // - A downstream *TripleEmit* inlines the buffered expr into the
                //   COPY's inner SELECT (the byte-identical hello.fossil path —
                //   no standalone SELECT, no extra subquery).
                // - A downstream *relational* op (e.g. Extend→Filter) instead
                //   FROMs a standalone `SELECT *, <expr> AS "field"` subquery so
                //   the computed column is materialised. `select_extend` builds
                //   that body; the COPY collapse simply prefers the buffer.
                let body = ast::select_extend(&from, field, &expr_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                collapse_from[idx] = Some(from);
            }
            Op::Project { input, cols } => {
                let body = ast::select_cols_from(cols, &input_relation(&rel_ref, *input));
                rel_ref[idx] = Some(subquery(&body, idx));
            }
            Op::Rename { input, old, new } => {
                let body = ast::select_rename(&input_relation(&rel_ref, *input), old, new);
                rel_ref[idx] = Some(subquery(&body, idx));
            }
            Op::Filter { input, pred } => {
                let pred_sql = render_expr(pred, &input_relation(&rel_ref, *input));
                let body = ast::select_filter(&input_relation(&rel_ref, *input), &pred_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
            }
            Op::Distinct { input, by } => {
                let body = ast::select_distinct(&input_relation(&rel_ref, *input), by.as_deref());
                rel_ref[idx] = Some(subquery(&body, idx));
            }
            Op::Union { left, right } => {
                let left_sql = ast::select_all_from(&input_relation(&rel_ref, *left));
                let right_sql = ast::select_all_from(&input_relation(&rel_ref, *right));
                let body = ast::union(&left_sql, &right_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
            }
            Op::Empty { schema } => {
                // ADR-0011 R9 target: a `WHERE false` shell over a notional
                // input view so the empty relation has the right column shape.
                let body = ast::select_empty(schema, "source");
                rel_ref[idx] = Some(subquery(&body, idx));
            }
            Op::TripleEmit {
                input,
                subject,
                predicate,
                object,
                graph: _,
            } => {
                // When the input is a buffered Extend, COPY over the relation
                // that Extend reads (collapse the computed column inline) — the
                // byte-identical hello.fossil path. Otherwise FROM the input op's
                // own relation (e.g. a Project/Filter subquery feeding emit).
                let from = collapse_from
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_relation(&rel_ref, *input));
                // Resolve `subject` / `object`. A bare `ColRef { source: "",
                // column }` naming a buffered Extend (the shared `iri` column)
                // is substituted with that Extend's rendered SQL so the COPY
                // sees the resolved expression. Keeps hello.fossil
                // byte-identical with Phase 1.
                let subject_expr = render_emit_operand(subject, &extends, &from);
                let object_expr = render_emit_operand(object, &extends, &from);
                emit = Some((subject_expr, predicate.to_string(), object_expr, from));
            }
            Op::Sink { input: _, sink: _ } => {
                if let Some((subject, predicate, object, from)) = &emit {
                    writeln!(
                        sql,
                        "COPY (\n    SELECT\n        {subject} AS subject,\n        '{predicate}' AS predicate,\n        {object} AS object\n    FROM {from}\n) TO 'output.parquet' (FORMAT PARQUET);"
                    )
                    .expect("writing to a String never fails");
                }
            }
            // Join / GroupBy / Aggregate land in plan 04-05. No corpus test or
            // direct-MIR snapshot exercises these arms this plan; a clearly
            // marked no-op keeps the match exhaustive without a production
            // panic.
            Op::Join { .. } | Op::GroupBy { .. } | Op::Aggregate { .. } => {
                // TODO(04-05): two-input Join + GroupBy/Aggregate codegen.
            }
        }
    }

    SqlPlan::new(db, sql, manifest_template())
}

/// Test-only entry point into the codegen seam.
///
/// Drives [`codegen_graph`] with a hand-built [`MirGraph`] from the `tests/`
/// integration crate. The Salsa query itself stays `pub` for the wrapper, but
/// tests call through this stable name so the seam is explicit. Used to snapshot
/// the source-unreachable operators (ADR-0009).
#[allow(clippy::elidable_lifetime_names)]
pub fn codegen_graph_for_test<'db>(
    db: &'db dyn fossil_base::Db,
    mir: MirGraph<'db>,
) -> SqlPlan<'db> {
    codegen_graph(db, mir)
}

/// Resolve op index `i`'s relation reference, falling back to the conventional
/// `"source"` view name when an op references an index that produced no
/// relation (a malformed graph — never in practice). `&rel_ref[i]` is the
/// view name (Source) or `(<body>) AS step_<i>` subquery (single-input ops).
fn input_relation(rel_ref: &[Option<String>], i: usize) -> String {
    rel_ref
        .get(i)
        .and_then(Clone::clone)
        .unwrap_or_else(|| "source".to_string())
}

/// Wrap a SELECT body as a nested-subquery relation reference for downstream
/// ops: `(<body>) AS step_<idx>` (ADR-0012 — subquery over CTE chain).
fn subquery(body: &str, idx: usize) -> String {
    format!("({body}) AS step_{idx}")
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
