//! The corpus's own facts: the writer counts what it wrote.
//!
//! Per column the nulls, the distinct values, the least and greatest value and
//! the commonest ones; per table its candidate keys; and the inclusions between
//! tables — the profile `fossil-profile` takes of a source, taken of the tables
//! of the corpus instead, over every row (`/docs/design/facts`).
//!
//! **The plan is `fossil-profile`'s and only the rendering is here.** The
//! profiler hands out each query as an [`Ask`]; `DuckDB`'s rendering of one is
//! SQL, and this crate builds `DataFusion` without its SQL frontend, so
//! [`render`] builds the same question as a `DataFrame`. Which queries to run,
//! how to prune them and what a count means are not stated a second time.
//!
//! **Under the run's budget.** Every query plans in the run's
//! [`SessionContext`], so it reserves against the same pool as every operator
//! before it, and a profile that does not fit is `run/over-budget` like any
//! other — before `fossil.json`, so no corpus is committed without its facts.

use std::sync::Arc;

use datafusion::arrow::array::{Array, RecordBatch, StringArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, SchemaRef};
use datafusion::common::Column;
use datafusion::datasource::MemTable;
use datafusion::error::{DataFusionError, Result};
use datafusion::functions_aggregate::expr_fn::{count, count_distinct, max, min};
use datafusion::logical_expr::{Expr, JoinType, cast as cast_expr};
use datafusion::prelude::{DataFrame, SessionContext, lit};
use fossil_profile::{Ask, Options, Profiler, Relation, Rows, Table, TOP_VALUES};

/// One table as the writer wrote it, to be counted.
#[derive(Debug)]
pub(crate) struct Written {
    /// The table's name in `fossil.json`.
    pub name: String,
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
    /// What each column may be in an inclusion, and its primitive, in schema
    /// order.
    pub columns: Vec<fossil_profile::TableColumn>,
}

/// What the writer counted, by the manifest's table names.
#[derive(Debug, Default)]
pub(crate) struct Facts {
    /// Per table, in the order given: its candidate keys, and each column's
    /// counts in schema order.
    pub tables: Vec<(Vec<String>, Vec<fossil_profile::ColumnStats>)>,
    /// `(from table, from column, to table, to column)`.
    pub inclusions: Vec<(String, String, String, String)>,
}

/// The engine's name for a written table while it is counted. The tables a run
/// registered while it executed are named by type; this prefix keeps the two
/// apart.
fn registered(name: &str) -> String {
    format!("__corpus__{name}")
}

/// Count `tables` on `ctx`, under its pool.
///
/// # Errors
/// What the engine refused — `run/over-budget` among it, which the executor
/// reads off the pool.
pub(crate) async fn count_written(ctx: &SessionContext, tables: &[Written]) -> Result<Facts> {
    for t in tables {
        let table = MemTable::try_new(Arc::clone(&t.schema), vec![t.batches.clone()])?;
        ctx.register_table(registered(&t.name), Arc::new(table))?;
    }
    let mut plan = Profiler::over_tables(
        tables.iter().map(|t| Table {
            name: registered(&t.name).into(),
            columns: t.columns.clone(),
        }),
        Options::WHOLE,
    );
    while let Some((id, ask)) = plan.next_ask() {
        let rows = answer(ctx, &ask).await?;
        plan.feed(id, &rows);
    }
    for t in tables {
        ctx.deregister_table(registered(&t.name))?;
    }
    let profile = plan.finish();
    let name_of = |registered_name: &str| {
        tables
            .iter()
            .find(|t| registered(&t.name) == registered_name)
            .map(|t| t.name.clone())
            .unwrap_or_default()
    };
    Ok(Facts {
        tables: profile
            .sources
            .into_iter()
            .map(|s| {
                (
                    s.candidate_keys.into_iter().map(|k| k.to_string()).collect(),
                    s.columns,
                )
            })
            .collect(),
        inclusions: profile
            .inclusions
            .into_iter()
            .map(|i| {
                (
                    name_of(&i.from.source),
                    i.from.column.to_string(),
                    name_of(&i.to.source),
                    i.to.column.to_string(),
                )
            })
            .collect(),
    })
}

fn column(name: &str) -> Expr {
    Expr::Column(Column::new_unqualified(name))
}

fn text(e: Expr) -> Expr {
    cast_expr(e, DataType::Utf8)
}

async fn relation(ctx: &SessionContext, relation: &Relation) -> Result<DataFrame> {
    match relation {
        Relation::Table(name) => ctx.table(name.as_str()).await,
        Relation::Read(read) => Err(DataFusionError::Plan(format!(
            "the writer counts the tables it holds, and `{read}` is a source"
        ))),
    }
}

/// An [`Ask`] answered on `DataFusion`: the rows its SQL would answer.
async fn answer(ctx: &SessionContext, ask: &Ask) -> Result<Rows> {
    if let Ask::Inclusion {
        dependent,
        column: c,
        referenced,
        key,
        ..
    } = ask
    {
        // Two counts: the dependent's distinct values, and those of them the
        // key does not hold.
        let values = relation(ctx, dependent)
            .await?
            .filter(column(c).is_not_null())?
            .select(vec![text(column(c)).alias("__v")])?
            .distinct()?;
        let keys = relation(ctx, referenced)
            .await?
            .select(vec![text(column(key)).alias("__k")])?
            .distinct()?;
        let missing = values
            .clone()
            .join(keys, JoinType::LeftAnti, &["__v"], &["__k"], None)?
            .count()
            .await?;
        let values = values.count().await?;
        return Ok(vec![vec![
            Some(values.to_string()),
            Some(missing.to_string()),
        ]]);
    }
    text_rows(render(ctx, ask).await?.collect().await?)
}

/// An [`Ask`] that is one query, as a `DataFusion` plan whose rows answer it as
/// its SQL would.
async fn render(ctx: &SessionContext, ask: &Ask) -> Result<DataFrame> {
    match ask {
        Ask::Counts {
            relation: r,
            columns,
            ..
        } => {
            let mut aggregates = vec![count(lit(1)).alias("__rows")];
            for (i, c) in columns.iter().enumerate() {
                aggregates.extend([
                    count_distinct(column(c)).alias(format!("__distinct_{i}")),
                    count(column(c)).alias(format!("__count_{i}")),
                    min(column(c)).alias(format!("__min_{i}")),
                    max(column(c)).alias(format!("__max_{i}")),
                ]);
            }
            relation(ctx, r).await?.aggregate(vec![], aggregates)
        }
        Ask::TopValues {
            relation: r,
            column: c,
            ..
        } => relation(ctx, r)
            .await?
            .filter(column(c).is_not_null())?
            .aggregate(vec![column(c)], vec![count(lit(1)).alias("__n")])?
            .filter(column("__n").gt_eq(lit(2_i64)))?
            .select(vec![text(column(c)).alias("__v"), column("__n")])?
            .sort(vec![
                column("__n").sort(false, false),
                column("__v").sort(true, false),
            ])?
            .limit(0, Some(TOP_VALUES)),
        Ask::Inclusion { .. } | Ask::Describe { .. } | Ask::Sample { .. } => Err(DataFusionError::Plan(
            "a written table's columns are known, none of its rows is kept, and an inclusion \
             is two counts"
                .to_string(),
        )),
    }
}

/// The batches as rows of text cells — every value cast to text, which is the
/// natural lexical form a corpus's `triples` gives it.
fn text_rows(batches: Vec<RecordBatch>) -> Result<Rows> {
    let mut rows = Vec::new();
    for batch in batches {
        let columns: Vec<StringArray> = batch
            .columns()
            .iter()
            .map(|c| {
                let text = cast(c, &DataType::Utf8)?;
                Ok(text
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .cloned()
                    .unwrap_or_else(|| StringArray::new_null(c.len())))
            })
            .collect::<Result<_>>()?;
        for r in 0..batch.num_rows() {
            rows.push(
                columns
                    .iter()
                    .map(|c| (!c.is_null(r)).then(|| c.value(r).to_string()))
                    .collect(),
            );
        }
    }
    Ok(rows)
}
