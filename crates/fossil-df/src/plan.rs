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

use datafusion::common::TableReference;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr as DfExpr, JoinType, LogicalPlanBuilder};
use datafusion::prelude::{DataFrame, SessionContext};
use fossil_hir::BinOp;
use fossil_mir::{Expr, JoinKind, JoinSide, Op};

use fossil_base::SourceAnchor;

use crate::{read_source, render};

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
/// - a [`Op::Join`] whose `kind` is not `Inner`, or whose `on` is not a
///   conjunction of equalities between columns of the two inputs — see
///   [`join_equalities`];
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
        // A source relation is qualified by the binding that names it, which is
        // the name every `ColRef` reading it carries (CODEGEN-LOWERING-01). It
        // is what lets a self-join keep two `label` columns apart, and what
        // lets `Purchase.id` and `User.id` coexist in one joined row.
        Op::Source {
            uri,
            format,
            binding,
            ..
        } => qualify(
            read_source(ctx, &anchor.locator(uri), format, binding).await?,
            binding,
        ),
        // `where(User.age >= 18)`: the predicate is a MIR expression like any
        // other, so the render is the one every property already goes through.
        Op::Filter { input: i, pred } => input(*i)?.filter(render(pred)),
        // `select(users.a, users.b)`: restrict the row to the named columns, in
        // the order named — under the relation each was written against, which
        // after a join is the only thing that says which side `id` came from.
        Op::Project { input: i, cols } => input(*i)?.select(
            cols.iter()
                .map(|c| column(&c.source, &c.column))
                .collect::<Vec<_>>(),
        ),
        Op::Join {
            left,
            right,
            on,
            kind,
        } => join(
            input(left.input)?,
            input(right.input)?,
            left,
            right,
            on,
            *kind,
        ),
        other => Err(DataFusionError::Plan(format!(
            "`{}` is defined in the operator algebra and this engine does not execute it \
             (it executes `Filter`, `Project` and `Join`/`Inner`)",
            op_name(other)
        ))),
    }
}

/// `fila(izq) ++ fila(der)`: both sides keep every column they have, under the
/// relation each is addressed by.
///
/// **Nothing is identified and nothing is dropped.** Until 2026-08-19 this was
/// `USING (k)` — one key name, present once in the result, with the right side's
/// copy renamed out of the way and dropped after the equality — and any other
/// name the two sides shared was refused as a collision. Both rules were the
/// same rule: the row was flat, so two columns called `id` could not both be
/// there. The row is not flat any more. `fossil-hir` deleted its half on
/// 2026-08-14 (ruling 17: `RowScope` keeps one entry per binding and a shared
/// column name means nothing), and this is the other half — the sides are
/// qualified rather than flattened, so `Purchase.id` and `User.id` are two
/// columns and the join has nothing to arbitrate.
///
/// A side is re-qualified only when the surface wrote `X as Y`: an alias is a
/// second name for the same source and the only thing that can tell the two
/// halves of a self-join apart. A side WITHOUT an alias is left alone, because
/// what qualifies it is already the name its columns are written under —
/// `Purchase.join(Adults, …)` reads a pipeline called `Adults` whose body says
/// `User.email`.
fn join(
    left: DataFrame,
    right: DataFrame,
    left_side: &JoinSide,
    right_side: &JoinSide,
    on: &Expr<'_>,
    kind: JoinKind,
) -> datafusion::error::Result<DataFrame> {
    if kind != JoinKind::Inner {
        return Err(DataFusionError::Plan(format!(
            "join kind `{kind:?}` is not executable: only `Inner` is (an outer join fabricates \
             NULL, and no output shape can declare a nullable property yet)"
        )));
    }
    let equalities = join_equalities(on, left_side, right_side)?;
    for (side, df, spec) in [("left", &left, left_side), ("right", &right, right_side)] {
        for (source, column) in condition_refs(on) {
            if source == spec.name().as_str()
                && !df.schema().has_column_with_unqualified_name(column)
            {
                return Err(DataFusionError::Plan(format!(
                    "the join condition names `{source}.{column}`, and the {side} side has {:?}",
                    df.schema().field_names()
                )));
            }
        }
    }

    let left = alias_of(left, left_side)?;
    let right = alias_of(right, right_side)?;
    left.join_on(right, JoinType::Inner, equalities)
}

/// Re-qualify `df` under a side's alias, or hand it back untouched when the
/// side has none. See [`join`] for why "none" is not the same as "its own name".
fn alias_of(df: DataFrame, side: &JoinSide) -> datafusion::error::Result<DataFrame> {
    match &side.alias {
        Some(alias) => qualify(df, alias),
        None => Ok(df),
    }
}

/// The equalities a join condition is made of, rendered — or the error that says
/// what arrived instead.
///
/// The admitted form is a conjunction of equalities between column references:
/// `on = a.k == b.k`, and `on = a.k == b.k and a.j == b.j` for a compound key.
/// The two columns need NOT share a name — `on = Purchase.user_id == User.id`
/// is the ordinary case, and the rule that refused it (*"a join key is one name
/// on both sides"*) was never a decision. It was the only thing this function
/// could do while every `ColRef` reaching it carried an empty source: with no
/// binding there was nothing to say which side a name belonged to, so equal
/// names were the one shape whose plan could be guessed. The binding arrives
/// now (CODEGEN-LOWERING-01), and the rule went with the ignorance that forced
/// it.
///
/// What is still refused, and why each survives the deletion:
///
/// - a conjunct that is not `==` between two column references — a join is an
///   equijoin here, and a theta join or a constant comparison would silently
///   become a nested loop over the product of two corpora;
/// - a reference qualified by a relation that is neither input — this check was
///   already written and never once fired, because an empty source can never be
///   unequal to both names;
/// - a reference naming a column its own side does not have ([`join`]).
///
/// Which side is written first does not matter: the equality is a filter over
/// the joined schema, so `a.k == b.k` and `b.k == a.k` plan identically.
fn join_equalities(
    on: &Expr<'_>,
    left: &JoinSide,
    right: &JoinSide,
) -> datafusion::error::Result<Vec<DfExpr>> {
    let mut out = Vec::new();
    for conjunct in conjuncts(on) {
        let Expr::BinOp {
            op: BinOp::Eq,
            lhs,
            rhs,
            ..
        } = conjunct
        else {
            return Err(DataFusionError::Plan(format!(
                "a join condition is an equality between columns (`on = a.k == b.k`, or several \
                 joined by `and`); this one is {}",
                expr_name(conjunct)
            )));
        };
        let (Expr::ColRef { source: ls, .. }, Expr::ColRef { source: rs, .. }) =
            (lhs.as_ref(), rhs.as_ref())
        else {
            return Err(DataFusionError::Plan(format!(
                "a join condition equates two column references (`on = a.k == b.k`); this one \
                 equates {} and {}",
                expr_name(lhs),
                expr_name(rhs)
            )));
        };
        for (side, source) in [("left", ls), ("right", rs)] {
            if !source.is_empty() && source != left.name() && source != right.name() {
                return Err(DataFusionError::Plan(format!(
                    "the {side} side of the join condition qualifies a column with `{source}`, \
                     which is neither input (`{}`, `{}`)",
                    left.name(),
                    right.name()
                )));
            }
        }
        out.push(render(conjunct));
    }
    Ok(out)
}

/// Split a condition on `and` — one element for a single key, N for a compound
/// one. A conjunction is the only structure a join condition is taken apart by;
/// everything else is a leaf for [`join_equalities`] to admit or refuse.
fn conjuncts<'e, 'db>(on: &'e Expr<'db>) -> Vec<&'e Expr<'db>> {
    match on {
        Expr::BinOp {
            op: BinOp::And,
            lhs,
            rhs,
            ..
        } => {
            let mut out = conjuncts(lhs);
            out.extend(conjuncts(rhs));
            out
        }
        other => vec![other],
    }
}

/// Every `(source, column)` a join condition names, qualified ones only — an
/// unqualified reference has no side to be checked against.
fn condition_refs<'e>(on: &'e Expr<'_>) -> Vec<(&'e str, &'e str)> {
    let mut out = Vec::new();
    let mut stack = vec![on];
    while let Some(e) = stack.pop() {
        match e {
            Expr::ColRef { source, column } if !source.is_empty() => {
                out.push((source.as_str(), column.as_str()));
            }
            Expr::BinOp { lhs, rhs, .. } | Expr::Concat(lhs, rhs) => {
                stack.push(lhs);
                stack.push(rhs);
            }
            _ => {}
        }
    }
    out
}

/// Re-qualify every column of `df` under `name` — the relational half of
/// `ColRef { source, column }`.
///
/// `TableReference::bare` (NOT `DataFrame::alias`, which takes a `&str` and
/// parses it as a SQL identifier): parsing folds an unquoted name to lowercase,
/// so `Other` would become `other` while the `ColRef` reading it still says
/// `Other`, and the column would resolve against nothing. It is the same trap
/// [`crate::render`] avoids by using `Column::new_unqualified` instead of
/// `col()`, and a source binding's case is the author's either way.
fn qualify(df: DataFrame, name: &str) -> datafusion::error::Result<DataFrame> {
    let (state, plan) = df.into_parts();
    let plan = LogicalPlanBuilder::from(plan)
        .alias(TableReference::bare(name))?
        .build()?;
    Ok(DataFrame::new(state, plan))
}

/// A `(source, column)` pair as a DataFusion column reference — qualified when
/// the source is known, bare when it is not (a retired `.column`, or a
/// hand-built op list). A bare reference resolves against whichever relation
/// carries the name, and is ambiguous if two do; that is DataFusion's rule and
/// its error names both candidates.
fn column(source: &str, column: &str) -> DfExpr {
    DfExpr::Column(if source.is_empty() {
        datafusion::common::Column::new_unqualified(column)
    } else {
        datafusion::common::Column::new(Some(TableReference::bare(source)), column)
    })
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
        Op::Join { left, right, .. } => vec![left.input, right.input],
        Op::Union { left, right } => vec![*left, *right],
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
        Expr::IsNull { .. } => "a null test",
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
