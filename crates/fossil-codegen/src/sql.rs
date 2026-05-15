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
use fossil_mir::{ExprLowered, Op, lower_to_mir};

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
                subject_col,
                predicate,
                object_col,
            } => {
                let view = source_view.clone().unwrap_or_else(|| "source".to_string());
                // Resolve the subject via the buffered Extend (Phase 1 always
                // emits Extend(field="iri") before TripleEmit). If the
                // mapping had no Extend (malformed for Phase 1, but tolerated)
                // we fall back to a column reference into the source view.
                let subject_expr = extends
                    .iter()
                    .find(|(name, _)| name == subject_col.as_str())
                    .map_or_else(
                        || format!("{view}.{subject_col}"),
                        |(_, expr_sql)| expr_sql.clone(),
                    );
                let object_expr = format!("{view}.{object_col}");
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
        }
    }

    SqlPlan::new(db, sql, manifest_template())
}

/// Render an [`ExprLowered`] tree to a `DuckDB` SQL fragment.
///
/// `default_source` is used as the qualifier for [`ExprLowered::ColRef`]
/// entries whose `source` field is empty (Phase 1 always populates it, so the
/// argument is kept for Phase 4's multi-source joins where a `ColRef` may
/// name a binding that the codegen needs to disambiguate).
fn render_expr(expr: &ExprLowered, default_source: &str) -> String {
    match expr {
        ExprLowered::LitString(s) => format!("'{}'", s.replace('\'', "''")),
        ExprLowered::ColRef { source, column } => {
            let qualifier = if source.is_empty() {
                default_source
            } else {
                source.as_str()
            };
            format!("{qualifier}.{column}")
        }
        ExprLowered::Concat(lhs, rhs) => {
            format!(
                "{} || {}",
                render_expr(lhs, default_source),
                render_expr(rhs, default_source),
            )
        }
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
