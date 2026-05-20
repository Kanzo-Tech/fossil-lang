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
use fossil_mir::op::{AggFn, CmpOp, JoinKind};
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
// `elidable_lifetime_names`: explicit 'db documents the Phase 2-9 contract.
// `too_many_lines`: the single op-dispatch match over the 12-variant Op enum is
// one coherent unit; splitting per-arm helpers would scatter the shared rel_ref
// / qualifier / extends / emit threading and hurt readability. Plan 04-05 adds
// the Join/GroupBy/Aggregate arms — revisit extraction then if it grows further.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names, clippy::too_many_lines)]
pub fn codegen_graph<'db>(db: &'db dyn fossil_base::Db, mir: MirGraph<'db>) -> SqlPlan<'db> {
    let ops = mir.ops(db);
    let mut sql = String::new();

    // The relation reference for each op index (the FROM-able SQL a downstream
    // op should read). `None` for terminal ops (TripleEmit/Sink) that produce
    // no consumable relation. For a Source this is the bare view name; for a
    // single-input SELECT op it is `(<body>) AS step_<idx>` (a nested subquery).
    let mut rel_ref: Vec<Option<String>> = vec![None; ops.len()];

    // The *column qualifier* (relation name) for each op index — what a
    // downstream `ColRef` should prefix with. For a Source it is the view name;
    // for a subquery op it is the subquery alias (`step_<idx>`). This is
    // distinct from `rel_ref` (the FROM text): a `ColRef` qualifies on the
    // alias, while the `FROM` clause carries the full `(<body>) AS step_<idx>`.
    let mut qualifier: Vec<Option<String>> = vec![None; ops.len()];

    // For a buffered Extend, the relation it reads FROM (so a consuming
    // TripleEmit collapses into a COPY over that same relation rather than a
    // redundant subquery — the byte-identical hello.fossil path) + the column
    // qualifier to use for that collapsed relation.
    let mut collapse_from: Vec<Option<String>> = vec![None; ops.len()];
    let mut collapse_qual: Vec<Option<String>> = vec![None; ops.len()];

    // Buffered Extends (field name → rendered expr SQL). An Extend feeding a
    // TripleEmit collapses into the COPY's inner SELECT (byte-identical
    // hello.fossil); it never becomes a standalone SELECT.
    let mut extends: Vec<(String, String)> = Vec::new();
    // (subject_expr, predicate_iri, object_expr, from_relation) — buffered by
    // each TripleEmit and consumed by Sink. A mapping with N TripleEmits
    // (multi-property, produced by 04-01's generalised lower_to_mir) collects N
    // entries; the Sink UNION-ALLs their triple-projections into one COPY. For
    // N = 1 the wrapper collapses to exactly the Phase-1 single-projection COPY
    // (byte-identical hello.fossil — guarded in the Sink arm).
    let mut emits: Vec<(String, String, String, String)> = Vec::new();

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
                rel_ref[idx] = Some(view_name.clone());
                qualifier[idx] = Some(view_name);
            }
            Op::Extend { input, field, expr } => {
                // Buffer the computed field. Unqualified ColRefs in the expr
                // qualify on the input relation's column qualifier (the source
                // view name in the hello.fossil path).
                let input_qual = input_qualifier(&qualifier, *input);
                let from = input_relation(&rel_ref, *input);
                let expr_sql = render_expr(expr, &input_qual);
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
                qualifier[idx] = Some(step_alias(idx));
                // A buffered Extend that collapses into a COPY exposes its
                // input relation (FROM text) + qualifier so the emit FROMs the
                // source view directly and qualifies columns on it.
                collapse_from[idx] = Some(from);
                collapse_qual[idx] = Some(input_qual);
            }
            Op::Project { input, cols } => {
                let body = ast::select_cols_from(cols, &input_relation(&rel_ref, *input));
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Rename { input, old, new } => {
                let body = ast::select_rename(&input_relation(&rel_ref, *input), old, new);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Filter { input, pred } => {
                let pred_sql = render_expr(pred, &input_qualifier(&qualifier, *input));
                let body = ast::select_filter(&input_relation(&rel_ref, *input), &pred_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Distinct { input, by } => {
                let body = ast::select_distinct(&input_relation(&rel_ref, *input), by.as_deref());
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Union { left, right } => {
                let left_sql = ast::select_all_from(&input_relation(&rel_ref, *left));
                let right_sql = ast::select_all_from(&input_relation(&rel_ref, *right));
                let body = ast::union(&left_sql, &right_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Empty { schema } => {
                // ADR-0011 R9 target: a `WHERE false` shell over a notional
                // input view so the empty relation has the right column shape.
                let body = ast::select_empty(schema, "source");
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
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
                //
                // `from` is the FROM-clause text (view name OR `(<body>) AS
                // step_<i>` subquery); `qual` is the column qualifier (the view
                // name OR the subquery alias `step_<i>`) — distinct, since a
                // bare ColRef must prefix the alias, not the whole subquery.
                let from = collapse_from
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_relation(&rel_ref, *input));
                let qual = collapse_qual
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_qualifier(&qualifier, *input));
                // Resolve `subject` / `object`. A bare `ColRef { source: "",
                // column }` naming a buffered Extend (the shared `iri` column)
                // is substituted with that Extend's rendered SQL so the COPY
                // sees the resolved expression. Keeps hello.fossil
                // byte-identical with Phase 1.
                let subject_expr = render_emit_operand(subject, &extends, &qual);
                let object_expr = render_emit_operand(object, &extends, &qual);
                emits.push((subject_expr, predicate.to_string(), object_expr, from));
            }
            Op::Sink { input: _, sink: _ } => {
                // Generalised SinkOp/COPY: wrap the buffered TripleEmit
                // projection(s) into one `COPY (...) TO 'output.parquet'
                // (FORMAT PARQUET)`. For a single TripleEmit the inner SELECT is
                // the Phase-1 shape verbatim (byte-identical hello.fossil); for
                // N > 1 the N triple-projections are `UNION ALL`'d (flat-triple
                // GraphAr Phase-1 contract — Phase 5 owns vertex/edge
                // decomposition). The COPY stays HAND-WRAPPED (Pitfall 1 — never
                // round-trip through sqlparser).
                if let Some(inner) = sink_inner_select(&emits) {
                    writeln!(
                        sql,
                        "COPY (\n{inner}\n) TO 'output.parquet' (FORMAT PARQUET);"
                    )
                    .expect("writing to a String never fails");
                }
            }
            Op::Join {
                left,
                right,
                on,
                kind,
                left_name,
                right_name,
            } => {
                // Two-input join. Each side's relation reference becomes a
                // parenthesised subquery aliased on the binding name
                // (`left_name` / `right_name`) so columns disambiguate across
                // the union of the two schemas (operator-algebra.md §2.6).
                let left_sql = input_relation(&rel_ref, *left);
                let right_sql = input_relation(&rel_ref, *right);
                // The ON predicate's `ColRef`s already carry their binding name
                // (`orders` / `users`) as `source`; the `default_source` only
                // covers a bare (empty-source) ColRef, which a join ON should
                // never use. Pass the left binding name as the conventional
                // fallback.
                let on_sql = render_expr(on, left_name.as_str());
                let body = ast::select_join(
                    &left_sql,
                    left_name.as_str(),
                    &right_sql,
                    right_name.as_str(),
                    join_kind_sql(*kind),
                    &on_sql,
                );
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::GroupBy { input, keys } => {
                // A GroupBy establishes the grouping keys. If the immediately
                // following op is an Aggregate, that arm renders the paired
                // `SELECT <keys>, <aggs> ... GROUP BY <keys>` (operator-algebra
                // treats GroupBy + Aggregate as one SELECT). A GroupBy NOT
                // followed by an Aggregate is just the key projection.
                let aggregated = matches!(ops.get(idx + 1), Some(Op::Aggregate { input: agg_in, .. }) if *agg_in == idx);
                if !aggregated {
                    let body = ast::select_group_by(&input_relation(&rel_ref, *input), keys, &[]);
                    rel_ref[idx] = Some(subquery(&body, idx));
                    qualifier[idx] = Some(step_alias(idx));
                }
                // When the next op aggregates this GroupBy, defer to that arm —
                // leave this op's rel_ref unset; the Aggregate reads `keys` back
                // off this GroupBy node.
            }
            Op::Aggregate { input, aggs } => {
                // Pair with the upstream GroupBy (if any) so the keys + agg
                // exprs render into one `GROUP BY` SELECT. If `input` is not a
                // GroupBy, this is a bare aggregate (no keys → global aggregate).
                let (keys, group_input) = match ops.get(*input) {
                    Some(Op::GroupBy { input: gb_in, keys }) => (keys.clone(), *gb_in),
                    _ => (Vec::new(), *input),
                };
                let agg_exprs: Vec<String> = aggs
                    .iter()
                    .map(|spec| {
                        format!(
                            "{}({}) AS \"{}\"",
                            agg_fn_sql(spec.agg_fn),
                            spec.in_field,
                            spec.out_field,
                        )
                    })
                    .collect();
                let body =
                    ast::select_group_by(&input_relation(&rel_ref, group_input), &keys, &agg_exprs);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
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

/// Build the inner SELECT of the terminal COPY from the buffered `TripleEmit`
/// projections.
///
/// - 0 emits → `None` (no COPY emitted — a Sink with no upstream `TripleEmit`).
/// - 1 emit → the Phase-1 single-projection SELECT verbatim (byte-identical
///   `hello.fossil`).
/// - N emits → the N triple-projections combined with `UNION ALL` (multi-
///   property mapping; flat-triple `GraphAr` Phase-1 contract).
///
/// The COPY wrapper itself stays hand-formatted in the caller (Pitfall 1).
fn sink_inner_select(emits: &[(String, String, String, String)]) -> Option<String> {
    if emits.is_empty() {
        return None;
    }
    let projection = |(subject, predicate, object, from): &(String, String, String, String)| {
        format!(
            "    SELECT\n        {subject} AS subject,\n        '{predicate}' AS predicate,\n        {object} AS object\n    FROM {from}"
        )
    };
    let inner = emits
        .iter()
        .map(projection)
        .collect::<Vec<_>>()
        .join("\n    UNION ALL\n");
    Some(inner)
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

/// Resolve op index `i`'s column qualifier (the relation name a downstream
/// `ColRef` prefixes), falling back to `"source"` for a malformed graph.
fn input_qualifier(qualifier: &[Option<String>], i: usize) -> String {
    qualifier
        .get(i)
        .and_then(Clone::clone)
        .unwrap_or_else(|| "source".to_string())
}

/// The subquery alias for op index `idx`: `step_<idx>`.
fn step_alias(idx: usize) -> String {
    format!("step_{idx}")
}

/// Wrap a SELECT body as a nested-subquery relation reference for downstream
/// ops: `(<body>) AS step_<idx>` (ADR-0012 — subquery over CTE chain).
fn subquery(body: &str, idx: usize) -> String {
    format!("({body}) AS {}", step_alias(idx))
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

/// Map a [`JoinKind`] to its DuckDB-portable JOIN keyword
/// (operator-algebra.md §2.6).
///
/// Rendered as a string rather than a `sqlparser::ast::JoinOperator` because
/// sqlparser 0.59 Display's `JoinOperator::FullOuter` as `FULL JOIN`, dropping
/// the explicit `OUTER` the corpus snapshots want. Both `FULL JOIN` and
/// `FULL OUTER JOIN` are DuckDB-equivalent; we emit the explicit spelling for
/// every kind for uniformity. These keywords are DuckDB-portable (RESEARCH
/// Pitfall 6 — portable JOIN syntax only).
const fn join_kind_sql(kind: JoinKind) -> &'static str {
    match kind {
        JoinKind::Inner => "INNER JOIN",
        JoinKind::LeftOuter => "LEFT OUTER JOIN",
        JoinKind::RightOuter => "RIGHT OUTER JOIN",
        JoinKind::Full => "FULL OUTER JOIN",
    }
}

/// Map an [`AggFn`] to its `DuckDB` aggregate-function spelling
/// (operator-algebra.md §2.9).
const fn agg_fn_sql(agg: AggFn) -> &'static str {
    match agg {
        AggFn::Count => "COUNT",
        AggFn::Sum => "SUM",
        AggFn::Min => "MIN",
        AggFn::Max => "MAX",
        AggFn::Avg => "AVG",
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
