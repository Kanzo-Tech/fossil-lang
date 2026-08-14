//! The relational half of the MIR, executed — the walk from an emit op back to
//! the sources it reads.
//!
//! Until F5 the chain was a straight line: one [`Op::Source`] at index 0 and
//! every emit op's `input` was 0 by construction, so the backend could find the
//! source and read it without ever following an index. A source pipeline
//! (`Adults := User.where(User.age >= 18)`, and `join` with it) makes `input`
//! mean what it always said it meant: an index into the op list.
//! This module is that walk, and it is the only place that turns a relational
//! operator into a [`DataFrame`].
//!
//! Three operators are executable here — [`Op::Filter`], [`Op::Project`] and
//! [`Op::Join`] (`Inner` only). The rest of the eleven stay unreachable, and
//! reaching one is an error that names it rather than a plan that quietly
//! drops it.

use std::collections::HashMap;

use datafusion::common::Column;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr as DfExpr, JoinType, Operator, binary_expr};
use datafusion::prelude::{DataFrame, SessionContext};
use fossil_hir::BinOp;
use fossil_mir::{Expr, JoinKind, Op};

use fossil_base::SourceAnchor;

use crate::{read_source, render};

/// The name the right side of a join carries its key under while the join is
/// being built. A join identifies its key: `on = .k` is `USING (k)`, so the key
/// appears **once** in the result row and not twice, and DataFusion's join keeps
/// both sides' keys — so the right one is renamed out of the way and dropped
/// after the join. The `__fossil_` prefix is not a column any source can spell.
const JOIN_KEY: &str = "__fossil_join_key";

/// The relation the op at `index` produces, as an un-collected [`DataFrame`].
///
/// `ops` is a [`fossil_mir::MirGraph`]'s op list in topological order (an op
/// only ever reads a lower index); `index` is normally an emit op's `input`.
/// The dependency closure of `index` is built once, in ascending order — no
/// operator is planned twice and none outside the closure is planned at all.
///
/// # Errors
/// - `index` is out of range, or an op reads an index at or above its own
///   (the list is not topologically ordered);
/// - the op at `index` is not a relational operator (an emit op is read by
///   [`crate::execute_vertex_ops`], not planned here);
/// - a [`Op::Join`] whose `kind` is not `Inner`, or whose `on` is not an
///   equality between the same column name on both sides — the only join
///   condition this engine plans;
/// - any DataFusion read/plan error.
pub async fn plan_relation(
    ctx: &SessionContext,
    ops: &[Op<'_>],
    index: usize,
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<DataFrame> {
    let mut built: HashMap<usize, DataFrame> = HashMap::new();
    for i in evaluation_order(ops, index)? {
        let df = build(ctx, ops, i, &built, anchor).await?;
        built.insert(i, df);
    }
    built
        .remove(&index)
        .ok_or_else(|| DataFusionError::Plan(format!("op {index} produced no relation")))
}

/// Build one operator's relation, reading its inputs from the already-built
/// `built` map (its inputs are all at lower indices, so they are all there).
async fn build(
    ctx: &SessionContext,
    ops: &[Op<'_>],
    index: usize,
    built: &HashMap<usize, DataFrame>,
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<DataFrame> {
    let input = |i: usize| -> datafusion::error::Result<DataFrame> {
        built.get(&i).cloned().ok_or_else(|| {
            DataFusionError::Plan(format!(
                "op {index} reads op {i}, which is not a relation this engine plans"
            ))
        })
    };
    match &ops[index] {
        Op::Source {
            uri,
            format,
            binding,
            ..
        } => read_source(ctx, &anchor.locator(uri), format, binding).await,
        // `where(.edad >= 18)`: the predicate is a MIR expression like any
        // other, so the render is the one every property already goes through.
        Op::Filter { input: i, pred } => input(*i)?.filter(render(pred)),
        // `select(.a, .b)`: restrict the row to the named columns, in the order
        // named. `new_unqualified` (NOT `col()`) for the same reason `render`
        // uses it — a source column's case is the source's, not SQL's.
        Op::Project { input: i, cols } => input(*i)?.select(
            cols.iter()
                .map(|c| DfExpr::Column(Column::new_unqualified(c.as_str())))
                .collect::<Vec<_>>(),
        ),
        Op::Join {
            left,
            right,
            on,
            kind,
            left_name,
            right_name,
        } => join(
            input(*left)?,
            input(*right)?,
            on,
            *kind,
            left_name,
            right_name,
        ),
        other => Err(DataFusionError::Plan(format!(
            "`{}` is defined in the operator algebra and this engine does not execute it \
             (it executes `Filter`, `Project` and `Join`/`Inner`)",
            op_name(other)
        ))),
    }
}

/// `fila(izq) ⊎ fila(der)`: the key is identified once, and any OTHER name the
/// two sides share is an error rather than a shadowing or an auto-qualification.
///
/// Both sides are re-projected to unqualified names first: two `read_csv`
/// frames carry the same synthetic qualifier, so without this the join schema
/// would hold two `?table?.k` fields and the plan would not build. Once
/// unqualified the only name they share is the key — the checker rejects any
/// other collision — and the key is carried on the right under [`JOIN_KEY`] so
/// it can be dropped after the equality has been made.
fn join(
    left: DataFrame,
    right: DataFrame,
    on: &Expr<'_>,
    kind: JoinKind,
    left_name: &str,
    right_name: &str,
) -> datafusion::error::Result<DataFrame> {
    if kind != JoinKind::Inner {
        return Err(DataFusionError::Plan(format!(
            "join kind `{kind:?}` is not executable: only `Inner` is (an outer join fabricates \
             NULL, and no output shape can declare a nullable property yet)"
        )));
    }
    let key = join_key(on, left_name, right_name)?;

    for (df, side, name) in [(&left, "left", left_name), (&right, "right", right_name)] {
        if !df.schema().has_column_with_unqualified_name(&key) {
            return Err(DataFusionError::Plan(format!(
                "the join key `{key}` is not a column of the {side} side `{name}`: it has {:?}",
                df.schema().field_names()
            )));
        }
    }
    let shared: Vec<String> = left
        .schema()
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .filter(|n| *n != key && right.schema().has_column_with_unqualified_name(n))
        .collect();
    if !shared.is_empty() {
        return Err(DataFusionError::Plan(format!(
            "`{left_name}` and `{right_name}` both carry {shared:?}, and a join identifies only \
             the key `{key}` — the checker refuses this before it gets here"
        )));
    }

    let left = unqualified(left, None)?;
    let right = unqualified(right, Some(&key))?;
    left.join_on(
        right,
        JoinType::Inner,
        [binary_expr(
            DfExpr::Column(Column::new_unqualified(key.as_str())),
            Operator::Eq,
            DfExpr::Column(Column::new_unqualified(JOIN_KEY)),
        )],
    )?
    .drop_columns(&[JOIN_KEY])
}

/// The one key name a join condition names, or the error that says what
/// arrived instead.
///
/// The admitted form is `on = .k` ≡ `USING (k)`: `BinOp { Eq, ColRef, ColRef }`
/// with the **same** column on both sides. A `ColRef`'s `source` is either the
/// corresponding input's name or empty — today's lowering leaves it empty and
/// lets the backend supply the relation (`lower_property_value`,
/// CODEGEN-LOWERING-01) — and since the key name alone determines the SQL,
/// which side is written first does not change the join. Anything else fails
/// here rather than becoming a quietly different plan.
fn join_key(on: &Expr<'_>, left_name: &str, right_name: &str) -> datafusion::error::Result<String> {
    let Expr::BinOp {
        op: BinOp::Eq,
        lhs,
        rhs,
        ..
    } = on
    else {
        return Err(DataFusionError::Plan(format!(
            "a join condition is an equality by name (`on = .k` ≡ `USING (k)`); this one \
             is {}",
            expr_name(on)
        )));
    };
    let (
        Expr::ColRef {
            source: ls,
            column: lc,
        },
        Expr::ColRef {
            source: rs,
            column: rc,
        },
    ) = (lhs.as_ref(), rhs.as_ref())
    else {
        return Err(DataFusionError::Plan(format!(
            "a join condition equates two column references (`on = .k`); this one \
             equates {} and {}",
            expr_name(lhs),
            expr_name(rhs)
        )));
    };
    if lc != rc {
        return Err(DataFusionError::Plan(format!(
            "a join key is one name on both sides (`on = .k` ≡ `USING (k)`); this one names \
             `{lc}` and `{rc}`. Two keys with different names are an extension this engine \
             does not build"
        )));
    }
    for (side, source) in [("left", ls), ("right", rs)] {
        if !source.is_empty() && source != left_name && source != right_name {
            return Err(DataFusionError::Plan(format!(
                "the {side} side of the join condition qualifies `{lc}` with `{source}`, which \
                 is neither input (`{left_name}`, `{right_name}`)"
            )));
        }
    }
    Ok(lc.to_string())
}

/// Re-project every column of `df` under its bare name, optionally carrying
/// `rename_key` under [`JOIN_KEY`]. Aliasing drops the relation qualifier, so
/// the two sides of a join can be told apart by name alone.
fn unqualified(df: DataFrame, rename_key: Option<&str>) -> datafusion::error::Result<DataFrame> {
    let exprs: Vec<DfExpr> = df
        .schema()
        .columns()
        .into_iter()
        .map(|c| {
            let out = if rename_key == Some(c.name()) {
                JOIN_KEY.to_string()
            } else {
                c.name().to_string()
            };
            DfExpr::Column(c).alias(out)
        })
        .collect();
    df.select(exprs)
}

/// The op indices `index` depends on, itself included, in ascending order.
///
/// The list is topologically ordered by [`fossil_mir::MirGraph`]'s invariant,
/// so ascending order *is* dependency order and one backward pass marks the
/// closure. A back-reference that does not go backwards is the invariant
/// broken, and it is reported as that rather than looped on.
fn evaluation_order(ops: &[Op<'_>], index: usize) -> datafusion::error::Result<Vec<usize>> {
    if index >= ops.len() {
        return Err(DataFusionError::Plan(format!(
            "op index {index} is past the end of a {}-op graph",
            ops.len()
        )));
    }
    let mut needed = vec![false; index + 1];
    needed[index] = true;
    for i in (0..=index).rev() {
        if !needed[i] {
            continue;
        }
        for d in inputs(&ops[i]) {
            if d >= i {
                return Err(DataFusionError::Plan(format!(
                    "op {i} reads op {d}: the op list is not in topological order"
                )));
            }
            needed[d] = true;
        }
    }
    Ok((0..=index).filter(|i| needed[*i]).collect())
}

/// The op indices an operator reads. Total over the algebra, so a new operator
/// stops compiling here rather than losing its input silently.
fn inputs(op: &Op<'_>) -> Vec<usize> {
    match op {
        Op::Source { .. } | Op::Empty { .. } => Vec::new(),
        Op::Project { input, .. }
        | Op::Extend { input, .. }
        | Op::Rename { input, .. }
        | Op::Filter { input, .. }
        | Op::GroupBy { input, .. }
        | Op::Aggregate { input, .. }
        | Op::Distinct { input, .. }
        | Op::EmitVertex { input, .. }
        | Op::EmitEdge { input, .. }
        | Op::Sink { input, .. } => vec![*input],
        Op::Join { left, right, .. } | Op::Union { left, right } => vec![*left, *right],
    }
}

/// The operator's name, for an error that has to say which one it is.
fn op_name(op: &Op<'_>) -> &'static str {
    match op {
        Op::Source { .. } => "Source",
        Op::Project { .. } => "Project",
        Op::Extend { .. } => "Extend",
        Op::Rename { .. } => "Rename",
        Op::Filter { .. } => "Filter",
        Op::Join { .. } => "Join",
        Op::Union { .. } => "Union",
        Op::GroupBy { .. } => "GroupBy",
        Op::Aggregate { .. } => "Aggregate",
        Op::Distinct { .. } => "Distinct",
        Op::EmitVertex { .. } => "EmitVertex",
        Op::EmitEdge { .. } => "EmitEdge",
        Op::Sink { .. } => "Sink",
        Op::Empty { .. } => "Empty",
    }
}

/// The expression form's name, for the same reason.
fn expr_name(e: &Expr<'_>) -> &'static str {
    match e {
        Expr::LitString(_) => "a string literal",
        Expr::LitBool(_) => "a boolean literal",
        Expr::LitInt(_) => "an integer literal",
        Expr::LitFloat(_) => "a float literal",
        Expr::UnaryOp { .. } => "a unary operator",
        Expr::ColRef { .. } => "a column reference",
        Expr::Concat(..) => "a concatenation",
        Expr::Call { .. } => "a call",
        Expr::BinOp { .. } => "a binary operator that is not `==`",
        Expr::Ternary { .. } => "a conditional",
        Expr::Assert { .. } => "an assertion",
    }
}
