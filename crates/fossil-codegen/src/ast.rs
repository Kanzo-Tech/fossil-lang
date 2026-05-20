//! `sqlparser` 0.59 AST construction helpers for the single-input operator
//! SELECT bodies.
//!
//! # Why a hybrid (ADR-0012)
//!
//! Codegen must produce byte-stable, well-quoted `DuckDB` SQL for the
//! 11-operator algebra. Phase 1 hand-`format!`'d the SQL; from Phase 4 the
//! *structured* parts of each inner `SELECT` (projection list, `FROM`, `WHERE`,
//! `DISTINCT`, `UNION`) are built as a `sqlparser::ast::Select` / `SetExpr` /
//! `Query` and rendered with `.to_string()` (`Display`). Leaf *expression*
//! fragments (the `||` concats, comparisons) keep flowing through the existing
//! string path in [`crate::sql::render_expr`] — re-parsed into a
//! `sqlparser::ast::Expr` via [`parse_expr_fragment`] and spliced into the
//! structured AST. This is the hybrid the research mandates: AST where it buys
//! stable quoting/structure, string fragments where hand-construction is more
//! verbose than valuable.
//!
//! # COPY is intentionally NOT built here (Pitfall 1)
//!
//! `Statement::Copy::to_string()` in sqlparser 0.59 drops the parenthesised
//! `(FORMAT PARQUET)` options form, so the terminal `COPY (...) TO '...'
//! (FORMAT PARQUET)` statement stays a hand-formatted template in
//! [`crate::sql`]. These helpers build only the inner `SELECT` bodies.
//!
//! # Verified `sqlparser` 0.59 field set (Task 1 / RESEARCH Open Question 4)
//!
//! Hand-construction of `ast::Select` requires its full non-exhaustive field
//! set, which drifts across minors. Verified against
//! `~/.cargo/registry/.../sqlparser-0.59.0/src/ast/query.rs` and a round-trip
//! parse-then-`Display` cross-check at implementation time:
//!
//! - `ast::Select` has 23 fields. The ones we set: `distinct: Option<Distinct>`,
//!   `projection: Vec<SelectItem>`, `from: Vec<TableWithJoins>`,
//!   `selection: Option<Expr>`. All others take their empty/`None`/`false`
//!   default (see [`base_select`]). `flavor: SelectFlavor::Standard`,
//!   `group_by: GroupByExpr::Expressions(vec![], vec![])`.
//! - `Distinct::Distinct` → `DISTINCT`; `Distinct::On(Vec<Expr>)` →
//!   `DISTINCT ON (...)`.
//! - `SelectItem::{Wildcard(WildcardAdditionalOptions), UnnamedExpr(Expr),
//!   ExprWithAlias { expr, alias }}`.
//! - `DuckDB` `EXCLUDE` rides on the wildcard via
//!   `WildcardAdditionalOptions.opt_exclude: Option<ExcludeSelectItem>`;
//!   `ExcludeSelectItem::Multiple(Vec<Ident>)` renders the parenthesised
//!   `EXCLUDE (col)` form (`DuckDB`'s documented spelling).
//! - `TableFactor::Table` carries 11 fields; only `name` is meaningful here.
//! - `SetExpr::SetOperation { op: SetOperator::Union, set_quantifier, left,
//!   right }` builds `UNION`.
//! - `AttachedToken::empty()` for the synthetic token fields.
//!
//! The compiler's non-exhaustive-struct error catches drift loudly on a
//! `sqlparser` bump; re-verify this field set then (and update the ADR).

// These builders are deliberately `pub(crate)` — they are the codegen-internal
// AST surface that `sql.rs` calls. The module is itself `pub(crate)`, so clippy
// flags the qualifier as redundant; we keep it for documentation intent (these
// are not crate-public API and must never leak out of `fossil-codegen`).
#![allow(clippy::redundant_pub_crate)]

use smol_str::SmolStr;
use sqlparser::ast::helpers::attached_token::AttachedToken;
use sqlparser::ast::{
    Distinct, ExcludeSelectItem, Expr as SqlExpr, GroupByExpr, Ident, ObjectName, ObjectNamePart,
    Query, Select, SelectFlavor, SelectItem, SetExpr, SetOperator, SetQuantifier, TableFactor,
    TableWithJoins, WildcardAdditionalOptions,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Tokenizer;

/// Parse a pre-rendered SQL expression fragment (produced by
/// [`crate::sql::render_expr`]) into a `sqlparser::ast::Expr` so it can be
/// spliced into a structured `Select`. The fragment is trusted codegen output
/// (never user text); a parse failure is a codegen bug, so we render the raw
/// fragment back via [`SqlExpr::Identifier`] as a last-resort passthrough
/// rather than panicking inside a Salsa query.
fn parse_expr_fragment(fragment: &str) -> SqlExpr {
    let dialect = GenericDialect {};
    let parsed = Tokenizer::new(&dialect, fragment)
        .tokenize()
        .ok()
        .and_then(|tokens| Parser::new(&dialect).with_tokens(tokens).parse_expr().ok());
    parsed.unwrap_or_else(|| SqlExpr::Identifier(Ident::new(fragment)))
}

/// A bare table reference `FROM <view>`.
fn table_from(view: &str) -> Vec<TableWithJoins> {
    vec![TableWithJoins {
        relation: TableFactor::Table {
            name: ObjectName(vec![ObjectNamePart::Identifier(Ident::new(view))]),
            alias: None,
            args: None,
            with_hints: vec![],
            version: None,
            with_ordinality: false,
            partitions: vec![],
            json_path: None,
            sample: None,
            index_hints: vec![],
        },
        joins: vec![],
    }]
}

/// Construct a `Select` with the four fields codegen varies; every other field
/// takes its empty default. Centralising the 23-field literal here means a
/// `sqlparser` bump that adds/renames a field fails to compile in exactly one
/// place.
fn base_select(
    projection: Vec<SelectItem>,
    view: &str,
    selection: Option<SqlExpr>,
    distinct: Option<Distinct>,
) -> Select {
    Select {
        select_token: AttachedToken::empty(),
        distinct,
        top: None,
        top_before_distinct: false,
        projection,
        exclude: None,
        into: None,
        from: table_from(view),
        lateral_views: vec![],
        prewhere: None,
        selection,
        group_by: GroupByExpr::Expressions(vec![], vec![]),
        cluster_by: vec![],
        distribute_by: vec![],
        sort_by: vec![],
        having: None,
        named_window: vec![],
        qualify: None,
        window_before_qualify: false,
        value_table_mode: None,
        connect_by: None,
        flavor: SelectFlavor::Standard,
    }
}

/// Render a `Select` body as a standalone query string.
fn render_select(select: Select) -> String {
    let query = Query {
        with: None,
        body: Box::new(SetExpr::Select(Box::new(select))),
        order_by: None,
        limit_clause: None,
        fetch: None,
        locks: vec![],
        for_clause: None,
        settings: None,
        format_clause: None,
        pipe_operators: vec![],
    };
    query.to_string()
}

/// A bare `*` wildcard select item with no extra options.
fn wildcard() -> SelectItem {
    SelectItem::Wildcard(WildcardAdditionalOptions::default())
}

/// `SELECT * FROM <view>`.
pub(crate) fn select_all_from(view: &str) -> String {
    render_select(base_select(vec![wildcard()], view, None, None))
}

/// `Project(cols)` — `SELECT <cols> FROM <view>`.
pub(crate) fn select_cols_from(cols: &[SmolStr], view: &str) -> String {
    let projection = cols
        .iter()
        .map(|c| SelectItem::UnnamedExpr(SqlExpr::Identifier(Ident::new(c.as_str()))))
        .collect();
    render_select(base_select(projection, view, None, None))
}

/// `Extend(field, expr)` — `SELECT *, <expr_sql> AS "<field>" FROM <view>`.
/// `expr_sql` is a pre-rendered fragment from [`crate::sql::render_expr`].
pub(crate) fn select_extend(view: &str, field: &str, expr_sql: &str) -> String {
    let projection = vec![
        wildcard(),
        SelectItem::ExprWithAlias {
            expr: parse_expr_fragment(expr_sql),
            alias: Ident::with_quote('"', field),
        },
    ];
    render_select(base_select(projection, view, None, None))
}

/// `Rename(old, new)` — `SELECT * EXCLUDE (<old>), <old> AS "<new>" FROM <view>`
/// (`DuckDB` `EXCLUDE`).
pub(crate) fn select_rename(view: &str, old: &str, new: &str) -> String {
    let wildcard_with_exclude = SelectItem::Wildcard(WildcardAdditionalOptions {
        opt_exclude: Some(ExcludeSelectItem::Multiple(vec![Ident::new(old)])),
        ..WildcardAdditionalOptions::default()
    });
    let renamed = SelectItem::ExprWithAlias {
        expr: SqlExpr::Identifier(Ident::new(old)),
        alias: Ident::with_quote('"', new),
    };
    render_select(base_select(
        vec![wildcard_with_exclude, renamed],
        view,
        None,
        None,
    ))
}

/// `Filter(pred)` — `SELECT * FROM <view> WHERE <pred_sql>`. `pred_sql` is a
/// pre-rendered fragment from [`crate::sql::render_expr`].
pub(crate) fn select_filter(view: &str, pred_sql: &str) -> String {
    render_select(base_select(
        vec![wildcard()],
        view,
        Some(parse_expr_fragment(pred_sql)),
        None,
    ))
}

/// `Distinct(by)` — `SELECT DISTINCT [ON (<by>)] * FROM <view>`.
pub(crate) fn select_distinct(view: &str, by: Option<&[SmolStr]>) -> String {
    let distinct = by.map_or(Distinct::Distinct, |cols| {
        Distinct::On(
            cols.iter()
                .map(|c| SqlExpr::Identifier(Ident::new(c.as_str())))
                .collect(),
        )
    });
    render_select(base_select(vec![wildcard()], view, None, Some(distinct)))
}

/// `Union(l, r)` — `<l_sql> UNION <r_sql>`. The two inputs are pre-rendered
/// query strings (already valid `SELECT ...` bodies); they are parsed back into
/// `SetExpr` bodies and combined via `SetExpr::SetOperation`.
pub(crate) fn union(left_sql: &str, right_sql: &str) -> String {
    let set = SetExpr::SetOperation {
        op: SetOperator::Union,
        set_quantifier: SetQuantifier::None,
        left: Box::new(parse_set_expr(left_sql)),
        right: Box::new(parse_set_expr(right_sql)),
    };
    let query = Query {
        with: None,
        body: Box::new(set),
        order_by: None,
        limit_clause: None,
        fetch: None,
        locks: vec![],
        for_clause: None,
        settings: None,
        format_clause: None,
        pipe_operators: vec![],
    };
    query.to_string()
}

/// Parse a query string into its `SetExpr` body (for [`union`]). Falls back to a
/// `*`-wildcard select over the raw text if the fragment does not parse (a
/// codegen bug, never user text — never panic in a Salsa query).
fn parse_set_expr(query_sql: &str) -> SetExpr {
    let dialect = GenericDialect {};
    let parsed = Tokenizer::new(&dialect, query_sql)
        .tokenize()
        .ok()
        .and_then(|tokens| Parser::new(&dialect).with_tokens(tokens).parse_query().ok());
    parsed.map_or_else(
        || {
            SetExpr::Select(Box::new(base_select(
                vec![wildcard()],
                query_sql,
                None,
                None,
            )))
        },
        |query| *query.body,
    )
}

/// `Join(left, right, on, kind, ln, rn)` —
/// `SELECT <ln>.*, <rn>.* FROM (<left_sql>) AS <ln> <kind> JOIN (<right_sql>) AS <rn> ON <on_sql>`.
///
/// `left_sql` / `right_sql` are pre-rendered relation references (a bare view
/// name OR a `(<body>) AS step_<i>` subquery); `kind` is the already-mapped
/// JOIN keyword (`INNER JOIN` / `LEFT OUTER JOIN` / `RIGHT OUTER JOIN` /
/// `FULL OUTER JOIN`) produced by [`crate::sql::join_kind_sql`]; `on_sql` is a
/// pre-rendered predicate fragment from [`crate::sql::render_expr`] (the
/// `ColRef`s already qualified on `left_name` / `right_name`).
///
/// # Why the JOIN keyword is a string, not the AST `JoinOperator`
///
/// sqlparser 0.59 renders `JoinOperator::FullOuter` as `FULL JOIN` (dropping
/// `OUTER`); `Join`'s done-criterion + the corpus snapshots want the explicit
/// `FULL OUTER JOIN` spelling (both are DuckDB-equivalent). So the JOIN keyword
/// is threaded as a verified DuckDB-portable string while the table factors +
/// aliases keep their structured form. The select-list (`<ln>.*`, `<rn>.*`) and
/// `ON` predicate are likewise rendered via the string fragments codegen
/// already produces — `Join` is the one body where the string path buys exact,
/// portable keyword control over the AST `JoinOperator` Display.
pub(crate) fn select_join(
    left_sql: &str,
    left_name: &str,
    right_sql: &str,
    right_name: &str,
    kind: &str,
    on_sql: &str,
) -> String {
    format!(
        "SELECT {left_name}.*, {right_name}.* FROM ({left_sql}) AS {left_name} {kind} ({right_sql}) AS {right_name} ON {on_sql}"
    )
}

/// `GroupBy(keys) + Aggregate(aggs)` —
/// `SELECT <keys>, <agg_exprs> FROM (<input_sql>) GROUP BY <keys>`.
///
/// `input_sql` is a pre-rendered relation reference (view name or subquery);
/// `keys` are the group keys (also the leading projection columns); `agg_exprs`
/// are pre-rendered `<agg_fn>(<in_field>) AS "<out_field>"` fragments built by
/// [`crate::sql`] from the consuming `Aggregate`'s `AggSpec`s. A `GroupBy` with
/// no consuming `Aggregate` passes `agg_exprs = &[]` (just the key projection).
pub(crate) fn select_group_by(input_sql: &str, keys: &[SmolStr], agg_exprs: &[String]) -> String {
    let keys_csv = keys
        .iter()
        .map(SmolStr::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let mut projection = keys_csv.clone();
    if !agg_exprs.is_empty() {
        if !projection.is_empty() {
            projection.push_str(", ");
        }
        projection.push_str(&agg_exprs.join(", "));
    }
    if projection.is_empty() {
        projection.push('*');
    }
    if keys.is_empty() {
        format!("SELECT {projection} FROM ({input_sql})")
    } else {
        format!("SELECT {projection} FROM ({input_sql}) GROUP BY {keys_csv}")
    }
}

/// `Empty(schema)` — `SELECT <cols> FROM <view> WHERE false` (ADR-0011 R9
/// target). Carries the would-be schema so downstream ops see the right column
/// shape even though the relation is empty.
pub(crate) fn select_empty(cols: &[SmolStr], view: &str) -> String {
    let projection: Vec<SelectItem> = if cols.is_empty() {
        vec![wildcard()]
    } else {
        cols.iter()
            .map(|c| SelectItem::UnnamedExpr(SqlExpr::Identifier(Ident::new(c.as_str()))))
            .collect()
    };
    let false_lit = SqlExpr::Value(sqlparser::ast::Value::Boolean(false).with_empty_span());
    render_select(base_select(projection, view, Some(false_lit), None))
}
