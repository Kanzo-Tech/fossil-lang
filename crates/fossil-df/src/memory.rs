//! What a run may hold: a budget sized for `wasm32`, and the compaction that
//! makes `DataFusion`'s accounting of it true.
//!
//! **The executor is a `wasm32` program.** Its linear memory ends at 4 GiB and
//! it has no disk, so nothing in a plan can spill.
//! [`Budget`](crate::memory::Budget) is the pool every run executes under:
//! [`BUDGET`](crate::memory::BUDGET) bytes, arithmetic that saturates instead
//! of wrapping, and a refusal that names the consumer, what it asked for and
//! what was already held. The run keeps that refusal and reports it as
//! `run/over-budget` (`Problem::OverBudget`) before the corpus is written — the
//! write starts only after `execute_graph` returns.
//!
//! **Why a bound and not the default.** `DataFusion`'s default pool is
//! unbounded: it adds every reservation into a `usize` and checks nothing. On
//! `wasm32` a `usize` is 32 bits, so a run whose reservations summed past
//! 4 GiB wrapped, and the next `MemoryReservation::split` found less than it
//! had put in and unwrapped a `None` — `memory_pool/mod.rs:494`, an
//! `unreachable` trap mid-run, with the run's promise never settled. The shop
//! program at 200,000 people and 600,000 orders did that. The same run natively
//! peaked at 5.6 GiB of *accounted* memory and finished in 1.5 s, which is why
//! only the browser saw it.
//!
//! **Why the accounting was seventy times the data.** An aggregate emits its
//! groups as one batch and hands it on in `batch_size` slices, and a slice
//! shares its parent's buffers. `get_record_batch_memory_size` charges a
//! buffer's whole capacity, so the `ExternalSorter` above the dedup charged
//! every one of seventy-three slices of `Order` for all 78 MiB of it.
//! [`Compact`](crate::memory::Compact) puts a
//! [`CompactExec`](crate::memory::CompactExec) under every operator that holds
//! its whole input — a sort, and a hash join's build side — and it copies a
//! batch out of a shared buffer before that operator counts it. The copy is of
//! what the slice spans, once.
//!
//! **The hash join stays.** The deleted native host cleared `prefer_hash_join`
//! under a budget, because a sort-merge join can spill and a hash join's build
//! side cannot. With no disk neither can, and a sort-merge join adds two sorts
//! to hold.
//!
//! What this cannot prove: the pool bounds what operators *reserve*, not what
//! the process holds. The source bytes, the collected graph and the encoded
//! Parquet are outside it, which is why
//! [`BUDGET`](crate::memory::BUDGET) is half the address space and not all of
//! it. Measured on the shop program in Node: 900,000 people finish at 2.64 GiB
//! of linear memory, and 1,000,000 are refused at 2.70 GiB, before the corpus
//! is written. `packages/corpus/integration/scale.test.ts` runs the scale that
//! used to trap.
//!
//! What would reverse it: a `DataFusion` whose sort charges a slice for what it
//! spans deletes [`CompactExec`](crate::memory::CompactExec); a `wasm64`
//! target, or a disk, moves [`BUDGET`](crate::memory::BUDGET).

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use datafusion::arrow::array::{Array, ArrayRef, MutableArrayData, make_array};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::config::ConfigOptions;
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::common::utils::memory::get_record_batch_memory_size;
use datafusion::common::{DataFusionError, Result};
use datafusion::execution::TaskContext;
use datafusion::execution::memory_pool::{MemoryLimit, MemoryPool, MemoryReservation};
use datafusion::physical_optimizer::PhysicalOptimizerRule;
use datafusion::physical_plan::joins::HashJoinExec;
use datafusion::physical_plan::sorts::sort::SortExec;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties, SendableRecordBatchStream,
};
use fossil_graph_schema::Problem;
use futures::StreamExt;

/// The bytes a run's operators may reserve: 2 GiB, half of `wasm32`'s linear
/// memory. The other half is for what no operator reserves — see the module.
pub const BUDGET: usize = 2 << 30;

/// An operator asked for more than the run's [`Budget`] had left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The `MemoryConsumer` that asked.
    pub consumer: String,
    /// The bytes it asked for.
    pub requested: usize,
    /// The bytes every consumer held when it asked.
    pub reserved: usize,
    /// The budget.
    pub budget: usize,
}

/// The refusal as the catalogue states it. Its sentence is the variant's, so
/// the engine's `ResourcesExhausted` and the run's failure say the same thing.
impl From<Refusal> for Problem {
    fn from(r: Refusal) -> Self {
        let bytes = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        Self::OverBudget {
            consumer: r.consumer,
            requested: bytes(r.requested),
            reserved: bytes(r.reserved),
            budget: bytes(r.budget),
        }
    }
}

/// A bounded [`MemoryPool`] whose arithmetic cannot wrap, and which keeps its
/// first refusal for the run to report.
#[derive(Debug)]
pub struct Budget {
    limit: usize,
    used: AtomicUsize,
    refused: Mutex<Option<Refusal>>,
}

impl Budget {
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self {
            limit,
            used: AtomicUsize::new(0),
            refused: Mutex::new(None),
        }
    }

    /// The first request this pool refused, if one was.
    #[must_use]
    pub fn refusal(&self) -> Option<Refusal> {
        self.refused.lock().ok().and_then(|r| r.clone())
    }
}

impl fmt::Display for Budget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "budget({} of {} bytes)",
            self.used.load(Ordering::Relaxed),
            self.limit
        )
    }
}

impl MemoryPool for Budget {
    fn name(&self) -> &'static str {
        "budget"
    }

    fn grow(&self, _: &MemoryReservation, additional: usize) {
        let _ = self
            .used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                Some(used.saturating_add(additional))
            });
    }

    fn shrink(&self, _: &MemoryReservation, shrink: usize) {
        let _ = self
            .used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                Some(used.saturating_sub(shrink))
            });
    }

    fn try_grow(&self, reservation: &MemoryReservation, additional: usize) -> Result<()> {
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(additional).filter(|&n| n <= self.limit)
            })
            .map(drop)
            .map_err(|reserved| {
                let refusal = Refusal {
                    consumer: reservation.consumer().name().to_owned(),
                    requested: additional,
                    reserved,
                    budget: self.limit,
                };
                let message = Problem::from(refusal.clone()).to_string();
                if let Ok(mut first) = self.refused.lock() {
                    first.get_or_insert(refusal);
                }
                DataFusionError::ResourcesExhausted(message)
            })
    }

    fn reserved(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    fn memory_limit(&self) -> MemoryLimit {
        MemoryLimit::Finite(self.limit)
    }
}

/// Puts a [`CompactExec`] under every operator that holds its whole input.
#[derive(Debug)]
pub struct Compact;

impl PhysicalOptimizerRule for Compact {
    fn optimize(
        &self,
        plan: Arc<dyn ExecutionPlan>,
        _: &ConfigOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(plan
            .transform_up(|node| {
                let held = if node.is::<SortExec>() || node.is::<HashJoinExec>() {
                    0
                } else {
                    return Ok(Transformed::no(node));
                };
                let mut children: Vec<_> = node.children().into_iter().cloned().collect();
                if children[held].is::<CompactExec>() {
                    return Ok(Transformed::no(node));
                }
                children[held] = Arc::new(CompactExec::new(Arc::clone(&children[held])));
                Ok(Transformed::yes(node.with_new_children(children)?))
            })?
            .data)
    }

    fn name(&self) -> &'static str {
        "compact"
    }

    fn schema_check(&self) -> bool {
        true
    }
}

/// Passes its input through, copying any batch that holds more than twice
/// the bytes it spans into buffers of its own.
#[derive(Debug)]
pub struct CompactExec {
    input: Arc<dyn ExecutionPlan>,
}

impl CompactExec {
    const fn new(input: Arc<dyn ExecutionPlan>) -> Self {
        Self { input }
    }
}

impl DisplayAs for CompactExec {
    fn fmt_as(&self, _: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CompactExec")
    }
}

impl ExecutionPlan for CompactExec {
    fn name(&self) -> &'static str {
        "CompactExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.input.properties()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        vec![true]
    }

    fn benefits_from_input_partitioning(&self) -> Vec<bool> {
        vec![false]
    }

    fn with_new_children(
        self: Arc<Self>,
        mut children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(Arc::new(Self::new(children.swap_remove(0))))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let input = self.input.execute(partition, context)?;
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            input.map(|batch| batch.and_then(compact)),
        )))
    }
}

/// `batch`, in buffers of its own when the ones it shares hold more than
/// twice what it spans.
fn compact(batch: RecordBatch) -> Result<RecordBatch> {
    let mut spans = 0;
    for column in batch.columns() {
        spans += column.to_data().get_slice_memory_size()?;
    }
    if get_record_batch_memory_size(&batch) <= spans.saturating_mul(2) {
        return Ok(batch);
    }
    let columns: Vec<ArrayRef> = batch.columns().iter().map(copy).collect();
    Ok(RecordBatch::try_new(batch.schema(), columns)?)
}

fn copy(column: &ArrayRef) -> ArrayRef {
    let data = column.to_data();
    let mut out = MutableArrayData::new(vec![&data], false, column.len());
    out.extend(0, 0, column.len());
    make_array(out.freeze())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::{Array, Int64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::catalog::MemTable;
    use datafusion::common::utils::memory::get_record_batch_memory_size;
    use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
    use datafusion::execution::memory_pool::{MemoryConsumer, MemoryPool};
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    use datafusion::execution::session_state::SessionStateBuilder;
    use datafusion::prelude::{SessionConfig, SessionContext, col};

    use super::{Budget, compact};
    use crate::session::{OneTask, session_within};

    const ROWS: i64 = 200_000;

    /// `ROWS` distinct subjects, deduplicated and then sorted — the shape of
    /// `finalize_vertex`. The dedup emits one batch and hands it on in slices.
    fn dedup_then_sort(ctx: &SessionContext) -> datafusion::error::Result<usize> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("n", DataType::Int64, false),
            Field::new("s", DataType::Utf8, false),
        ]));
        let batches = (0..ROWS)
            .step_by(8_192)
            .map(|start| {
                let end = (start + 8_192).min(ROWS);
                RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![
                        Arc::new(Int64Array::from_iter_values(start..end)),
                        Arc::new(
                            (start..end)
                                .map(|i| Some(format!("https://example.org/{i:08}")))
                                .collect::<StringArray>(),
                        ),
                    ],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        ctx.register_table("t", Arc::new(MemTable::try_new(schema, vec![batches])?))?;
        futures::executor::block_on(async {
            let sorted = ctx
                .table("t")
                .await?
                .distinct()?
                .sort(vec![col("s").sort(true, false)])?
                .collect()
                .await?;
            Ok(sorted.iter().map(RecordBatch::num_rows).sum())
        })
    }

    #[test]
    fn a_sort_after_a_dedup_is_charged_for_its_rows_and_not_for_its_slices() {
        const LIMIT: usize = 64 << 20;
        let rows = dedup_then_sort(&session_within(Arc::new(Budget::new(LIMIT))))
            .expect("fits once compacted");
        assert_eq!(rows, usize::try_from(ROWS).expect("small"));

        // The control: the same session without `Compact` charges the sort for
        // every slice's parent batch, and the same budget refuses it.
        let budget = Arc::new(Budget::new(LIMIT));
        let runtime = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::clone(&budget) as _)
            .with_disk_manager_builder(
                DiskManagerBuilder::default().with_mode(DiskManagerMode::Disabled),
            )
            .build_arc()
            .expect("runtime");
        let state = SessionStateBuilder::new()
            .with_config(SessionConfig::new().with_target_partitions(1))
            .with_runtime_env(runtime)
            .with_default_features()
            .with_physical_optimizer_rule(Arc::new(OneTask))
            .build();
        dedup_then_sort(&SessionContext::new_with_state(state)).expect_err("refused");
        let refusal = budget.refusal().expect("the budget refused it");
        assert!(
            refusal.consumer.starts_with("ExternalSorter"),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_request_past_the_budget_is_refused_and_kept_and_nothing_wraps() {
        let pool: Arc<dyn MemoryPool> = Arc::new(Budget::new(100));
        let held = MemoryConsumer::new("held").register(&pool);
        held.try_grow(60).expect("inside the budget");
        let asking = MemoryConsumer::new("asking").register(&pool);
        let err = asking.try_grow(usize::MAX).expect_err("past the budget");
        assert!(err.to_string().contains("asking asked for"), "{err}");
        asking.try_grow(41).expect_err("one byte past");
        asking.try_grow(40).expect("exactly the budget");
        assert_eq!(pool.reserved(), 100);
        held.grow(usize::MAX);
        assert_eq!(pool.reserved(), usize::MAX, "saturates, never wraps");
    }

    #[test]
    fn the_first_refusal_is_the_one_reported() {
        let budget = Arc::new(Budget::new(10));
        let pool: Arc<dyn MemoryPool> = Arc::clone(&budget) as _;
        let r = MemoryConsumer::new("first").register(&pool);
        r.try_grow(11).expect_err("refused");
        r.try_grow(12).expect_err("refused");
        let refusal = budget.refusal().expect("kept");
        assert_eq!(
            (refusal.consumer.as_str(), refusal.requested),
            ("first", 11)
        );
    }

    #[test]
    fn a_slice_is_copied_out_of_its_parent_and_a_whole_batch_is_not() {
        let n = 10_000;
        let schema = Arc::new(Schema::new(vec![
            Field::new("n", DataType::Int64, false),
            Field::new("s", DataType::Utf8, true),
        ]));
        let parent = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from_iter_values(0..n)),
                Arc::new(
                    (0..n)
                        .map(|i| (i % 7 != 0).then(|| format!("subject {i}")))
                        .collect::<StringArray>(),
                ),
            ],
        )
        .expect("batch");
        let whole = compact(parent.clone()).expect("whole");
        assert!(Arc::ptr_eq(parent.column(0), whole.column(0)), "untouched");

        let slice = parent.slice(100, 100);
        let copied = compact(slice.clone()).expect("slice");
        assert_eq!(copied, slice, "the same rows");
        assert!(
            get_record_batch_memory_size(&copied) * 20 < get_record_batch_memory_size(&slice),
            "{} against {}",
            get_record_batch_memory_size(&copied),
            get_record_batch_memory_size(&slice)
        );
        assert_eq!(copied.column(1).null_count(), slice.column(1).null_count());
    }
}
