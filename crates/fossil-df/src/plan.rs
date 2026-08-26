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
//! Six operators are executable here — [`Op::Filter`], [`Op::Project`],
//! [`Op::Distinct`], [`Op::Union`], [`Op::GroupBy`] and [`Op::Join`] (`Inner`
//! only). The rest stay unreachable, and reaching one is an error that names it
//! rather than a plan that quietly drops it.

use std::collections::HashMap;

use datafusion::common::TableReference;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr as DfExpr, JoinType, LogicalPlanBuilder};
use datafusion::prelude::{DataFrame, SessionContext};
use fossil_hir::BinOp;
use fossil_mir::{AggFn, Expr, JoinKind, JoinSide, Op};

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
        // `distinct()`: whole rows, and nothing to say about which one survives
        // because they are identical. `DISTINCT ON` is refused one layer up —
        // see `Op::Distinct`.
        Op::Distinct { input: i } => input(*i)?.distinct(),
        // `union(Contractor)`: `UNION ALL`, then the whole thing qualified under
        // the relation it answers to.
        //
        // DataFusion pairs the two sides BY POSITION and hands back a schema
        // whose fields carry NO qualifier at all — so the re-qualification has
        // to come after, and doing it to each side first would leave the result
        // unqualified anyway. That is why `Op::Union` names the relation instead
        // of borrowing the left side's: there is nothing to borrow.
        Op::Union {
            left: l,
            right: r,
            relation,
        } => qualify(input(*l)?.union(input(*r)?)?, relation),
        // `group_by(Order.customer, total = math.sum(Order.amount))`: one
        // DataFusion `aggregate` call, which is why the MIR carries one
        // operator and not the `GroupBy`+`Aggregate` pair it used to.
        //
        // The keys keep the qualifier they were written with. The aggregate
        // columns have none to keep — DataFusion names a field after the
        // expression, `sum(Order.amount)`, and an `alias` is unqualified — so
        // each is aliased UNDER the relation the op names, which is what makes
        // the body's `Totals.total` resolve.
        Op::GroupBy {
            input: i,
            keys,
            aggs,
            relation,
        } => input(*i)?.aggregate(
            keys.iter()
                .map(|k| column(&k.source, &k.column))
                .collect::<Vec<_>>(),
            aggs.iter()
                .map(|a| {
                    agg_call(a.agg_fn, column(&a.column.source, &a.column.column)).alias_qualified(
                        Some(TableReference::bare(relation.as_str())),
                        a.out_field.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
        ),
        other => Err(DataFusionError::Plan(format!(
            "`{}` is defined in the operator algebra and this engine does not execute it \
             (it executes `Filter`, `Project`, `Distinct`, `Union`, `GroupBy` and \
             `Join`/`Inner`)",
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
/// One [`AggFn`] as the DataFusion aggregate it is.
///
/// Total on purpose: the catalogue decides which rows are aggregates and a
/// fifth one added there stops compiling here rather than losing its column.
fn agg_call(f: AggFn, arg: DfExpr) -> DfExpr {
    use datafusion::functions_aggregate::expr_fn::{avg, max, min, sum};
    match f {
        AggFn::Sum => sum(arg),
        AggFn::Avg => avg(arg),
        AggFn::Min => min(arg),
        AggFn::Max => max(arg),
    }
}

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
        | Op::Distinct { input, .. }
        | Op::EmitVertex { input, .. }
        | Op::EmitEdge { input, .. }
        | Op::Sink { input, .. } => vec![*input],
        Op::Join { left, right, .. } => vec![left.input, right.input],
        Op::Union { left, right, .. } => vec![*left, *right],
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

// ───────────────────────────────────────────────── the join key, asserted here

/// [`join_equalities`] and [`conjuncts`], tested directly.
///
/// Both are private, so `tests/pipeline.rs` can only reach them through the rows
/// that come out, and until this module existed nothing in `src/plan.rs` was
/// tested at all. The case every test below is built on is the one `8184c1f`
/// measured, and it is the case **no count can see**:
/// `apps/docs/programs/compound-key` joins on two columns, and dropping the
/// `tenant` conjunct takes the join from four rows to seven while the seven mint
/// the same four subjects — so the vertex count, the property list and the
/// mapping census are all equal across the break. The predicate rendered into
/// that program's `expected/compiled.txt` was the only committed evidence that a
/// compound key is compound. It is not the only one now:
/// [`tests::a_compound_key_is_four_rows_and_one_conjunct_short_is_seven`] is the
/// number, and [`tests::the_seven_rows_mint_the_same_four_subjects`] is why the
/// number had to be asserted somewhere the counts are not.
///
/// The rows are read from that program's own CSVs rather than copied into a
/// fixture here. A transcribed fixture agrees with the artefact right up until
/// one of the two moves.
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use datafusion::arrow::array::{Array, StringArray};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::prelude::SessionContext;
    use fossil_base::test_support::NativeSystem;
    use fossil_base::{FossilDb, System};
    use fossil_graph_schema::Primitive;
    use fossil_hir::ty::{Record, RecordField};
    use fossil_hir::{Ty, TyKind};
    use fossil_mir::SourceFormat;
    use smol_str::SmolStr;

    use super::{
        BinOp, Expr, JoinKind, JoinSide, Op, SourceAnchor, conjuncts, join_equalities,
        plan_relation,
    };

    fn db() -> FossilDb {
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        FossilDb::new(system)
    }

    /// A `Record` row over `names`, all `String`. Only `fossil_mir::schema_of`
    /// reads it — the backend takes its columns from the file — but a `Source`
    /// is not well-formed without one.
    fn row<'db>(db: &'db dyn fossil_base::Db, names: &[&str]) -> Ty<'db> {
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

    fn source<'db>(
        db: &'db dyn fossil_base::Db,
        uri: &str,
        binding: &str,
        cols: &[&str],
    ) -> Op<'db> {
        Op::Source {
            uri: SmolStr::from(uri),
            format: SourceFormat::Csv,
            row_type: row(db, cols),
            binding: SmolStr::from(binding),
        }
    }

    fn col(source: &str, column: &str) -> Expr<'static> {
        Expr::ColRef {
            source: SmolStr::from(source),
            column: SmolStr::from(column),
        }
    }

    fn binop<'db>(
        db: &'db dyn fossil_base::Db,
        op: BinOp,
        lhs: Expr<'db>,
        rhs: Expr<'db>,
    ) -> Expr<'db> {
        Expr::BinOp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
        }
    }

    fn eq<'db>(db: &'db dyn fossil_base::Db, lhs: Expr<'db>, rhs: Expr<'db>) -> Expr<'db> {
        binop(db, BinOp::Eq, lhs, rhs)
    }

    fn and<'db>(db: &'db dyn fossil_base::Db, lhs: Expr<'db>, rhs: Expr<'db>) -> Expr<'db> {
        binop(db, BinOp::And, lhs, rhs)
    }

    fn side(input: usize, relation: &str) -> JoinSide {
        JoinSide {
            input,
            relation: SmolStr::from(relation),
            alias: None,
        }
    }

    /// `LineRow.order_id == OrderRow.id` — the conjunct a single-column join
    /// would have, and the one the break keeps.
    fn order_key(db: &dyn fossil_base::Db) -> Expr<'_> {
        eq(db, col("LineRow", "order_id"), col("OrderRow", "id"))
    }

    /// `LineRow.tenant == OrderRow.tenant` — the conjunct the break drops, and
    /// the reason `compound-key` exists.
    fn tenant_key(db: &dyn fossil_base::Db) -> Expr<'_> {
        eq(db, col("LineRow", "tenant"), col("OrderRow", "tenant"))
    }

    /// The whole condition `compound-key.fossil` writes.
    fn compound_key(db: &dyn fossil_base::Db) -> Expr<'_> {
        and(db, order_key(db), tenant_key(db))
    }

    /// The conformance program's own directory. `data/lines.csv` resolves
    /// against it exactly as the program's own run resolves it, which is what
    /// makes these the artefact's rows and not a copy of them.
    fn compound_key_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/docs/programs/compound-key")
    }

    /// `LineRow.join(OrderRow, on = …)` over that program's two CSVs, executed.
    async fn joined(db: &FossilDb, on: Expr<'_>) -> Vec<RecordBatch> {
        let ops = vec![
            source(
                db,
                "data/lines.csv",
                "LineRow",
                &["tenant", "id", "order_id", "quantity"],
            ),
            source(
                db,
                "data/orders.csv",
                "OrderRow",
                &["tenant", "id", "placed_on"],
            ),
            Op::Join {
                left: side(0, "LineRow"),
                right: side(1, "OrderRow"),
                on,
                kind: JoinKind::Inner,
            },
        ];
        let ctx = SessionContext::new();
        let dir = compound_key_dir();
        plan_relation(&ctx, &ops, 2, SourceAnchor::beside(&dir))
            .await
            .expect("the join plans")
            .collect()
            .await
            .expect("the join runs")
    }

    fn total_rows(batches: &[RecordBatch]) -> usize {
        batches.iter().map(RecordBatch::num_rows).sum()
    }

    fn strings(batch: &RecordBatch, i: usize) -> Vec<String> {
        let a = batch
            .column(i)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("a string column");
        (0..a.len()).map(|r| a.value(r).to_string()).collect()
    }

    /// The subjects the joined rows mint, deduplicated —
    /// `"https://shop.example/{LineRow.tenant}/line/{LineRow.id}"`, the template
    /// `compound-key.fossil` writes. Columns 0 and 1 are the left side's
    /// `tenant` and `id`: a join composes `fila(izq) ++ fila(der)`.
    fn subjects(batches: &[RecordBatch]) -> Vec<String> {
        let mut out: Vec<String> = batches
            .iter()
            .flat_map(|b| {
                strings(b, 0)
                    .into_iter()
                    .zip(strings(b, 1))
                    .map(|(tenant, line)| format!("https://shop.example/{tenant}/line/{line}"))
                    .collect::<Vec<_>>()
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// `and` is the only structure a condition is taken apart by, and it is
    /// taken apart whole: `(a and b) and c` is three conjuncts, not two.
    #[test]
    fn a_conjunction_is_taken_apart_and_a_leaf_is_not() {
        let db = db();
        assert_eq!(
            conjuncts(&order_key(&db)).len(),
            1,
            "one equality is one conjunct"
        );
        assert_eq!(
            conjuncts(&compound_key(&db)).len(),
            2,
            "`a and b` is its two conjuncts, never the `and` itself"
        );
        let three = and(
            &db,
            compound_key(&db),
            eq(
                &db,
                col("LineRow", "quantity"),
                col("OrderRow", "placed_on"),
            ),
        );
        assert_eq!(
            conjuncts(&three).len(),
            3,
            "the split is recursive, however the parser associated the `and`s"
        );
        let disjunction = binop(&db, BinOp::Or, order_key(&db), tenant_key(&db));
        assert_eq!(
            conjuncts(&disjunction).len(),
            1,
            "`or` is a leaf here — it is refused by join_equalities, not split by this"
        );
    }

    /// The compound key reaches the plan as TWO equalities, each naming the
    /// columns it came from.
    ///
    /// This is the assertion that goes red the moment a conjunct stops reaching
    /// the plan, and it goes red by the number: one rendered equality where the
    /// program wrote two.
    #[test]
    fn a_compound_key_reaches_the_plan_as_two_equalities() {
        let db = db();
        let equalities = join_equalities(
            &compound_key(&db),
            &side(0, "LineRow"),
            &side(1, "OrderRow"),
        )
        .expect("a conjunction of column equalities is the admitted form");
        let rendered: Vec<String> = equalities.iter().map(ToString::to_string).collect();
        assert_eq!(
            rendered,
            [
                "LineRow.order_id = OrderRow.id",
                "LineRow.tenant = OrderRow.tenant"
            ],
            "both conjuncts are planned, in written order, qualified by the side each reads"
        );
    }

    /// A conjunct that is not an equality between two column references is
    /// refused, and the message says which form arrived.
    #[test]
    fn a_conjunct_that_is_not_a_column_equality_is_refused() {
        let db = db();
        let theta = binop(&db, BinOp::Lt, col("LineRow", "quantity"), Expr::LitInt(10));
        let not_an_equality = and(&db, order_key(&db), theta);
        let err = join_equalities(&not_an_equality, &side(0, "LineRow"), &side(1, "OrderRow"))
            .expect_err("a theta join is not an equijoin");
        assert!(
            err.to_string()
                .contains("a binary operator that is not `==`"),
            "the refusal names the form that arrived: {err}"
        );

        let against_a_literal = eq(
            &db,
            col("LineRow", "tenant"),
            Expr::LitString("north".into()),
        );
        let err = join_equalities(
            &against_a_literal,
            &side(0, "LineRow"),
            &side(1, "OrderRow"),
        )
        .expect_err("a constant comparison is a filter, not a key");
        assert!(
            err.to_string().contains("a string literal"),
            "the refusal names the side that is not a column: {err}"
        );
    }

    /// A reference qualified by a relation that is neither input is refused —
    /// the check the docblock says had never once fired, because until
    /// `CODEGEN-LOWERING-01` every `ColRef` reaching here carried an empty
    /// source and an empty source is equal to neither name.
    #[test]
    fn a_conjunct_qualified_by_neither_input_is_refused() {
        let db = db();
        let elsewhere = eq(&db, col("Invoice", "tenant"), col("OrderRow", "tenant"));
        let err = join_equalities(&elsewhere, &side(0, "LineRow"), &side(1, "OrderRow"))
            .expect_err("`Invoice` is not an input of this join");
        let message = err.to_string();
        assert!(
            message.contains("`Invoice`") && message.contains("`LineRow`, `OrderRow`"),
            "the refusal names the stranger and both inputs: {message}"
        );
    }

    /// **The measurement `8184c1f` made, executed.** Four rows on the key the
    /// program wrote; seven on the key one conjunct short.
    ///
    /// `data/lines.csv` holds four lines over two tenants and `data/orders.csv`
    /// three orders over the same two, and order number `1001` exists in both —
    /// so `order_id == id` alone pairs every `north` line with the `south` order
    /// and back. That is the whole reason `compound-key` is in the conformance
    /// set, and this is the number that says so.
    #[tokio::test]
    async fn a_compound_key_is_four_rows_and_one_conjunct_short_is_seven() {
        let db = db();
        assert_eq!(
            total_rows(&joined(&db, compound_key(&db)).await),
            4,
            "on = LineRow.order_id == OrderRow.id and LineRow.tenant == OrderRow.tenant"
        );
        assert_eq!(
            total_rows(&joined(&db, order_key(&db)).await),
            7,
            "the same join with the `tenant` conjunct gone — and nothing downstream notices"
        );
    }

    /// **Why the row count had to be asserted here.** The seven rows mint the
    /// same four subjects as the four, so every number the conformance harness
    /// keeps — the vertex count, the property list, the mapping census — is
    /// equal across the break.
    ///
    /// This one passes on both sides of the defect on purpose. It is the
    /// falsifiable form of the claim the artefact, `programs.rs` and
    /// `fossil-hir`'s `display` docblock all repeat in prose, and it is what
    /// makes the test above a guard rather than a duplicate of the census.
    #[tokio::test]
    async fn the_seven_rows_mint_the_same_four_subjects() {
        let db = db();
        let whole = subjects(&joined(&db, compound_key(&db)).await);
        let broken = subjects(&joined(&db, order_key(&db)).await);
        assert_eq!(
            whole,
            [
                "https://shop.example/north/line/L-1",
                "https://shop.example/north/line/L-2",
                "https://shop.example/north/line/L-3",
                "https://shop.example/south/line/L-1",
            ],
            "the four lines the program writes"
        );
        assert_eq!(
            broken, whole,
            "seven rows, four subjects — the same four, so no count moves"
        );
    }
}
