//! What the catalogue declares an aggregate returns, against what `DataFusion`
//! actually returns — the whole table, for every input type a source can carry.
//!
//! `math.sum` declares `Float -> Float` (`fossil_hir::stdlib`, the `math/`
//! block) and DataFusion's `sum` over an `Int64` column gives an `Int64`. The
//! disagreement is older than `group_by`: the row's template is `sum(%0)` and
//! the scalar rendering has always had it. What `group_by` added is a place
//! where the declared type is WRITTEN DOWN in the artefact —
//! `crates/fossil-df/src/lib.rs`'s `node_property` puts the checker's
//! `Primitive` into the manifest while `files.rs` encodes the engine's Arrow
//! type into the Parquet — so this file measures the pair rather than arguing
//! about it.
//!
//! Every number here is produced by the run, not transcribed: the aggregate
//! runs, its output schema is read, and the collected batch is checked to carry
//! the same type the schema promised.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, Decimal128Array, Float64Array, Int64Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::functions_aggregate::expr_fn::{avg, max, min, sum};
use datafusion::prelude::{DataFrame, SessionContext, col};
use fossil_hir::stdlib::{AggFn, FunctionRegistry, ScalarTy};

/// The four `math/` rows the catalogue calls aggregates, with the name each is
/// reached by. `RegistryEntry::agg_fn` is the map; this is its inverse, and
/// [`every_aggregate_row_is_reachable_and_declares_float`] holds them together.
const AGGREGATES: [(&str, AggFn); 4] = [
    ("math.sum", AggFn::Sum),
    ("math.avg", AggFn::Avg),
    ("math.min", AggFn::Min),
    ("math.max", AggFn::Max),
];

/// One measured cell: what came out, for this aggregate over this input type.
struct Cell {
    label: &'static str,
    input: DataType,
    agg: &'static str,
    declared: ScalarTy,
    actual: DataType,
}

/// The input column types a fossil source can hand an aggregate.
///
/// `Decimal128` is in the list because it is REACHABLE and unrepresentable:
/// `crates/fossil-introspect/src/lib.rs:90` maps any `DECIMAL…` to
/// `Primitive::Float`, so the checker calls the column a `Float` while a
/// Parquet source hands DataFusion a `Decimal128`. The language has no decimal
/// type (`fossil_graph_schema::Primitive` is nine variants and none is one).
fn input_columns() -> Vec<(&'static str, DataType, ArrayRef)> {
    vec![
        (
            "Int64",
            DataType::Int64,
            Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(3), Some(4)])) as ArrayRef,
        ),
        (
            "Int64 with nulls",
            DataType::Int64,
            // Same type, nulls in the middle of a group: NULL changes the VALUE
            // (it is skipped) and must not change the type.
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3), None])) as ArrayRef,
        ),
        (
            "Float64",
            DataType::Float64,
            Arc::new(Float64Array::from(vec![
                Some(1.5),
                Some(2.5),
                Some(3.5),
                Some(4.5),
            ])) as ArrayRef,
        ),
        (
            "Decimal128(10, 2)",
            DataType::Decimal128(10, 2),
            Arc::new(
                Decimal128Array::from(vec![Some(150), Some(250), Some(350), Some(450)])
                    .with_precision_and_scale(10, 2)
                    .expect("a decimal fixture that fits its own precision"),
            ) as ArrayRef,
        ),
    ]
}

/// Two rows per group, so an aggregate has something to fold.
fn table(ctx: &SessionContext, value: DataType, values: ArrayRef) -> DataFrame {
    let schema = Arc::new(Schema::new(vec![
        Field::new("k", DataType::Utf8, false),
        Field::new("v", value, true),
    ]));
    let keys: ArrayRef = Arc::new(StringArray::from(vec!["a", "a", "b", "b"]));
    let batch = RecordBatch::try_new(schema.clone(), vec![keys, values])
        .expect("four keys against four values");
    let mem = MemTable::try_new(schema, vec![vec![batch]]).expect("a one-batch table");
    ctx.read_table(Arc::new(mem)).expect("reading it back")
}

/// The DataFusion call `crates/fossil-df/src/plan.rs::agg_call` makes, spelled
/// the same way and by the same enum — a fifth aggregate added to the catalogue
/// stops compiling here too.
fn call(f: AggFn, arg: datafusion::prelude::Expr) -> datafusion::prelude::Expr {
    match f {
        AggFn::Sum => sum(arg),
        AggFn::Avg => avg(arg),
        AggFn::Min => min(arg),
        AggFn::Max => max(arg),
    }
}

/// Run every aggregate over every input type and read the output type off the
/// plan AND off the rows.
async fn measure() -> Vec<Cell> {
    let ctx = SessionContext::new();
    let registry = FunctionRegistry::stdlib_default();
    let mut cells = Vec::new();
    for (label, input, values) in input_columns() {
        for (name, agg) in AGGREGATES {
            let declared = registry
                .lookup(name)
                .and_then(|e| e.sig.ret.scalar())
                .expect("every aggregate row declares a scalar return");
            let df = table(&ctx, input.clone(), Arc::clone(&values))
                .aggregate(vec![col("k")], vec![call(agg, col("v")).alias("out")])
                .expect("one key, one aggregate");
            let planned = df
                .schema()
                .field_with_unqualified_name("out")
                .expect("the aliased aggregate")
                .data_type()
                .clone();
            let batches = df.collect().await.expect("the aggregate executes");
            for batch in &batches {
                let column = batch.column_by_name("out").expect("the aliased column");
                assert_eq!(
                    column.data_type(),
                    &planned,
                    "{name} over {input}: the plan promised one type and the rows carry another",
                );
            }
            cells.push(Cell {
                label,
                input: input.clone(),
                agg: name,
                declared,
                actual: planned,
            });
        }
    }
    cells
}

/// The measurement, printed as the table it is. `cargo test -p fossil-df
/// --test aggregate_result_types -- --nocapture` is where the numbers in any
/// argument about this come from.
#[tokio::test]
async fn the_table_of_declared_against_returned() {
    let cells = measure().await;
    println!("| input column | aggregate | catalogue declares | DataFusion returns | agree |");
    println!("|---|---|---|---|---|");
    let mut disagreements = 0;
    for c in &cells {
        let agrees = matches(c.declared, &c.actual);
        if !agrees {
            disagreements += 1;
        }
        println!(
            "| `{}` | `{}` | `{:?}` | `{:?}` | {} |",
            c.label,
            c.agg,
            c.declared,
            c.actual,
            if agrees { "yes" } else { "**NO**" }
        );
    }
    println!("\n{disagreements} of {} cells disagree.", cells.len());
    // Sixteen cells, TEN of them disagreeing. Not twelve: `avg` over an
    // `Int64` returns a `Float64`, so the six that agree are the four over a
    // `Float64` column plus `math.avg` over each of the two `Int64` ones. This
    // assertion is the CURRENT state and is expected to move the day the
    // disagreement is closed — whichever way it is closed — and a SILENT move
    // is what it exists to stop.
    assert_eq!(cells.len(), 16, "four aggregates over four input columns");
    assert_eq!(
        disagreements, 10,
        "the six that agree are the four over `Float64` plus `avg` over each `Int64`",
    );
}

/// `sum` is not the only one, and `min`/`max` are not the same shape of wrong.
///
/// - `sum` over `Int64` is `Int64` — the declared `Float` is a widening the
///   engine never performs.
/// - `min`/`max` are TYPE-PRESERVING by definition: their result is one of the
///   input values, so no coercion could make them return a `Float` without
///   inventing a value that was not in the column.
/// - `avg` over `Int64` IS `Float64` — the one arm where the declaration and
///   the engine already agree for an integer input.
#[tokio::test]
async fn each_aggregate_disagrees_in_its_own_way() {
    let cells = measure().await;
    let at = |agg: &str, input: &DataType| {
        cells
            .iter()
            .find(|c| c.agg == agg && &c.input == input)
            .map(|c| c.actual.clone())
            .expect("every pair was measured")
    };
    assert_eq!(at("math.sum", &DataType::Int64), DataType::Int64);
    assert_eq!(at("math.min", &DataType::Int64), DataType::Int64);
    assert_eq!(at("math.max", &DataType::Int64), DataType::Int64);
    assert_eq!(at("math.avg", &DataType::Int64), DataType::Float64);

    assert_eq!(at("math.sum", &DataType::Float64), DataType::Float64);
    assert_eq!(at("math.avg", &DataType::Float64), DataType::Float64);

    // A decimal widens its PRECISION and stays a decimal, so neither candidate
    // fix reaches it by declaring a scalar: `Primitive` has no decimal, and
    // `fossil_sinks::manifest::data_type_name` has no `Decimal128` arm either
    // (it would fall to `binary`, which GraphAr's reader rejects).
    assert_eq!(
        at("math.sum", &DataType::Decimal128(10, 2)),
        DataType::Decimal128(20, 2),
    );
    assert_eq!(
        at("math.avg", &DataType::Decimal128(10, 2)),
        DataType::Decimal128(14, 6),
    );
    assert_eq!(
        at("math.min", &DataType::Decimal128(10, 2)),
        DataType::Decimal128(10, 2),
    );
}

/// A NULL in the column changes the value and not the type — so a fix cannot
/// hide behind nullability, and the disagreement is the same one with or
/// without them.
#[tokio::test]
async fn nulls_do_not_move_the_type() {
    let ctx = SessionContext::new();
    let dense: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(3), Some(4)]));
    let sparse: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), None, Some(3), None]));
    for (name, agg) in AGGREGATES {
        let ty = |values: ArrayRef| {
            table(&ctx, DataType::Int64, values)
                .aggregate(vec![col("k")], vec![call(agg, col("v")).alias("out")])
                .expect("one key, one aggregate")
                .schema()
                .field_with_unqualified_name("out")
                .expect("the aliased aggregate")
                .data_type()
                .clone()
        };
        assert_eq!(
            ty(Arc::clone(&dense)),
            ty(Arc::clone(&sparse)),
            "{name}: a null moved the result type",
        );
    }
}

/// The declaration is REACHABLE, which is the question worth asking of any row:
/// three MIR operators were found this week carrying parameters nothing in the
/// workspace ever built.
///
/// All four rows resolve in the registry, all four declare `Float -> Float`,
/// and all four are named by `RegistryEntry::agg_fn` — so a program can write
/// any of them inside a `group_by` and the `Float` is what the row hands the
/// checker. `apps/docs/programs/group-by/group-by.fossil` is the program that
/// does it for `math.sum`.
#[test]
fn every_aggregate_row_is_reachable_and_declares_float() {
    let registry = FunctionRegistry::stdlib_default();
    for (name, agg) in AGGREGATES {
        let entry = registry
            .lookup(name)
            .unwrap_or_else(|| panic!("{name} is not in the catalogue"));
        assert_eq!(entry.agg_fn(), Some(agg), "{name} names its aggregate");
        assert_eq!(
            entry.sig.ret.scalar(),
            Some(ScalarTy::Float),
            "{name} declares a Float return",
        );
        assert_eq!(
            entry.sig.params.len(),
            1,
            "{name} takes exactly the column it folds",
        );
        assert_eq!(
            entry.sig.params[0].ty.scalar(),
            Some(ScalarTy::Float),
            "{name} declares a Float parameter — an Integer column reaches it by \
             S-IntFlt (`fossil_hir::check::subtypes`), which is why the Int64 \
             row of the table above is reachable at all",
        );
    }
}

/// Does the declared scalar cover this Arrow type?
fn matches(declared: ScalarTy, actual: &DataType) -> bool {
    match declared {
        ScalarTy::Float => matches!(
            actual,
            DataType::Float16 | DataType::Float32 | DataType::Float64
        ),
        ScalarTy::Integer => matches!(
            actual,
            DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64
        ),
        ScalarTy::String => matches!(actual, DataType::Utf8 | DataType::LargeUtf8),
        ScalarTy::Bool => matches!(actual, DataType::Boolean),
        ScalarTy::Date => matches!(actual, DataType::Date32 | DataType::Date64),
        ScalarTy::DateTime => matches!(actual, DataType::Timestamp(_, _)),
        ScalarTy::SeqString => matches!(actual, DataType::List(_) | DataType::LargeList(_)),
    }
}
