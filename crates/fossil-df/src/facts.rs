//! The graph's own facts: the writer counts what it is about to write.
//!
//! Per column the distinct values and the commonest ones, which Parquet's
//! footer has no field for. They are written in each vertex chunk's own footer
//! under `fossil:stats` (`/docs/format/conventions/statistics`); the nulls and
//! the bounds are the footer's `Statistics`, which the Parquet writer counts
//! itself. These are the counts `fossil-profile` takes of a source, taken of the
//! tables of the graph instead, over every row (`/docs/design/facts`).
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
//! other — before the type's chunk is written, so no graph is committed without
//! its facts.

// `pub(crate)` and not `pub`: `unreachable_pub` holds the crate's surface to
// what it exports, and this module exports nothing.
#![allow(clippy::redundant_pub_crate)]

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, RecordBatch, StringArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::common::Column;
use datafusion::datasource::MemTable;
use datafusion::error::{DataFusionError, Result};
use datafusion::functions_aggregate::expr_fn::{count, count_distinct, max, min};
use datafusion::logical_expr::{Expr, cast as cast_expr};
use datafusion::prelude::{DataFrame, SessionContext, lit};
use fossil_base::sources::InferredColumn;
use fossil_graph_schema::Primitive;
use fossil_profile::{Ask, ColumnStats, Options, Profiler, Relation, Rows, TOP_VALUES, Table};
use graphar_info::Cardinality;
use serde::{Serialize, Serializer};

use crate::layout::Vertex;

/// The footer key a vertex chunk's facts are under: fossil's, namespaced the way
/// Arrow namespaces its own (`ARROW:schema`).
pub(crate) const FOOTER_KEY: &str = "fossil:stats";

/// The version of the document under [`FOOTER_KEY`]. A reader checks it and
/// refuses another.
const VERSION: u32 = 1;

/// The value of [`FOOTER_KEY`]: `{ version, columns }`, `columns` keyed by the
/// property's name, in the property group's order.
#[derive(Serialize)]
struct Stats {
    version: u32,
    columns: Columns,
}

struct Columns(Vec<(String, PropertyFacts)>);

impl Serialize for Columns {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(name, stats)| (name, stats)))
    }
}

/// One property's facts. The commonest values are two arrays of one length, as
/// `PostgreSQL`'s `pg_stats` has them, and absent when no value repeats.
#[derive(Serialize)]
struct PropertyFacts {
    distinct_count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    most_common_vals: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    most_common_freqs: Vec<f64>,
}

/// Where a property's facts are counted: the table, the column of it, and the
/// number of values a frequency is a share of.
struct Place {
    table: usize,
    column: usize,
    of: u64,
}

/// The facts of one vertex type, as the JSON document under [`FOOTER_KEY`].
///
/// A single-valued property is counted over its column, and a frequency is a
/// share of the chunk's rows, nulls among them. A multi-valued one is counted
/// over its **elements** (Parquet counts leaf values), and a frequency is a
/// share of them. A tagged value is counted by its lexical form.
///
/// # Errors
/// What the engine refused — `run/over-budget` among it, which the executor
/// reads off the pool.
pub(crate) async fn footer(ctx: &SessionContext, vertex: &Vertex) -> Result<String> {
    let group = &vertex.info.property_groups[0];
    let rows = vertex.batch.num_rows() as u64;

    // The plain single-valued columns share a table, so one query counts them
    // all; every other property is a table of its elements.
    let mut tables: Vec<(Vec<Field>, Vec<ArrayRef>)> = vec![(Vec::new(), Vec::new())];
    let mut places = Vec::with_capacity(group.properties.len());
    for (i, property) in group.properties.iter().enumerate() {
        let array = vertex.batch.column(i);
        let plain = property.cardinality == Cardinality::Single
            && !matches!(array.data_type(), DataType::Struct(_));
        let (elements, of) = if plain {
            (Arc::clone(array), rows)
        } else {
            let elements = flatten(array);
            let of = elements.len() as u64;
            (elements, of)
        };
        let field = Field::new(&property.name, elements.data_type().clone(), true);
        let table = if plain {
            0
        } else {
            tables.push((Vec::new(), Vec::new()));
            tables.len() - 1
        };
        tables[table].0.push(field);
        tables[table].1.push(elements);
        places.push(Place {
            table,
            column: tables[table].0.len() - 1,
            of,
        });
    }

    let written: Vec<Written> = tables
        .into_iter()
        .enumerate()
        .map(|(n, (fields, arrays))| {
            let columns = fields
                .iter()
                .map(|f| InferredColumn {
                    name: f.name().as_str().into(),
                    primitive: primitive(f.data_type()),
                })
                .collect();
            let schema = Arc::new(Schema::new(fields));
            let batch = RecordBatch::try_new(Arc::clone(&schema), arrays)?;
            Ok(Written {
                name: n.to_string(),
                schema,
                batches: vec![batch],
                columns,
            })
        })
        .collect::<Result<_>>()?;
    // `places` names a table by its position here, and the main table is never
    // empty: a vertex has `subject`.
    let counted = count_written(ctx, &written).await?;

    let columns = group
        .properties
        .iter()
        .zip(&places)
        .map(|(property, place)| {
            let stats: &ColumnStats = &counted[place.table][place.column];
            let of = place.of.max(1);
            let (vals, freqs): (Vec<String>, Vec<f64>) = stats
                .top_values
                .iter()
                .map(|t| {
                    #[allow(clippy::cast_precision_loss)]
                    let share = t.count as f64 / of as f64;
                    (t.value.clone(), share)
                })
                .unzip();
            (
                property.name.clone(),
                PropertyFacts {
                    distinct_count: stats.distinct_count,
                    most_common_vals: vals,
                    most_common_freqs: freqs,
                },
            )
        })
        .collect();
    serde_json::to_string(&Stats {
        version: VERSION,
        columns: Columns(columns),
    })
    .map_err(|e| DataFusionError::External(Box::new(e)))
}

/// The values a property is counted over: a list's elements, and a tagged
/// value's lexical form.
fn flatten(array: &ArrayRef) -> ArrayRef {
    let elements = match array.data_type() {
        DataType::List(_) => Arc::clone(array.as_list::<i32>().values()),
        _ => Arc::clone(array),
    };
    match elements.data_type() {
        DataType::Struct(_) => Arc::clone(elements.as_struct().column(0)),
        _ => elements,
    }
}

/// What a column of this Arrow type is, as far as the counts care.
const fn primitive(data_type: &DataType) -> Primitive {
    match data_type {
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64 => Primitive::Integer,
        DataType::Float16 | DataType::Float32 | DataType::Float64 => Primitive::Float,
        DataType::Boolean => Primitive::Bool,
        DataType::Date32 | DataType::Date64 => Primitive::Date,
        DataType::Timestamp(..) => Primitive::DateTime,
        _ => Primitive::String,
    }
}

/// One table as it is to be counted.
#[derive(Debug)]
struct Written {
    /// The table's name while it is counted.
    name: String,
    schema: Arc<Schema>,
    batches: Vec<RecordBatch>,
    /// Each column and its primitive, in schema order.
    columns: Vec<InferredColumn>,
}

/// The engine's name for a table while it is counted. The tables a run
/// registered while it executed are named by type; this prefix keeps the two
/// apart.
fn registered(name: &str) -> String {
    format!("__facts__{name}")
}

/// Count `tables` on `ctx`, under its pool: each column's counts, in the order
/// given.
///
/// # Errors
/// What the engine refused.
async fn count_written(ctx: &SessionContext, tables: &[Written]) -> Result<Vec<Vec<ColumnStats>>> {
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
    Ok(plan
        .finish()
        .sources
        .into_iter()
        .map(|s| s.columns)
        .collect())
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
        Ask::Inclusion { .. } | Ask::Describe { .. } | Ask::Sample { .. } => {
            Err(DataFusionError::Plan(
                "a written table's columns are known, none of its rows is kept, and no inclusion \
             is tested between tables"
                    .to_string(),
            ))
        }
    }
}

/// The batches as rows of text cells — every value cast to text, which is the
/// natural lexical form a graph's `triples` gives it.
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
