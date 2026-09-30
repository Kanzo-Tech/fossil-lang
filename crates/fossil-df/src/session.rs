//! The one `SessionContext` the browser runs a program in, and the rule that
//! keeps every plan it builds drivable by one future.
//!
//! The browser has no Tokio runtime: `wasm-bindgen-futures` polls the run from
//! JS's event loop, and a `DataFusion` operator that spawns a task panics with
//! «there is no reactor running» — which, under `panic = "abort"`, traps inside
//! a microtask and leaves the run's promise pending forever. `target_partitions
//! = 1` keeps the planner from ADDING partitions, and was once thought to be the
//! whole fix. It is not: a `UNION ALL` of two single-partition scans has two
//! partitions by construction, so two mappings of one type put a
//! `CoalescePartitionsExec` over the union, and that operator spawns one task
//! per input partition.
//!
//! [`OneTask`] runs last and rewrites the plan so nothing in it spawns:
//! - `CoalescePartitionsExec` becomes [`OneTaskCoalesceExec`], which polls
//!   every input partition from one stream (`futures::stream::select_all`) —
//!   the fix upstream merged as apache/datafusion#24890, after 55.1.0.
//! - `BufferExec` is a pure prefetch and is replaced by its input.
//! - `RepartitionExec` is REFUSED as a plan error. Its channels are fed by
//!   spawned tasks and there is no single-task equivalent; with one target
//!   partition the planner does not insert it, and if a future `DataFusion`
//!   does, the run says so instead of hanging.
//! - A root with more than one partition is coalesced the same way, because
//!   `collect` would otherwise wrap it in a `CoalescePartitionsExec` after the
//!   optimizer has run.
//!
//! What this cannot prove: the list of spawning operators is by name. It was
//! read off `datafusion-physical-plan` 54 — `SortPreservingMergeExec` and the
//! spill reader call `spawn_buffered`, which checks for a runtime and does not
//! spawn without one, and the spill files' `spawn_blocking` needs a disk the
//! browser does not have. `tests/execute_core.rs` drives a union with no
//! runtime at all, which is the check that does not depend on the list.
//!
//! What would reverse it: a `DataFusion` release carrying #24890 deletes
//! [`OneTaskCoalesceExec`] and the first arm; the refusal and the test stay.

use std::fmt;
use std::sync::Arc;

use datafusion::common::config::ConfigOptions;
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::common::{DataFusionError, Result};
use datafusion::execution::TaskContext;
use datafusion::execution::context::SessionContext;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::physical_optimizer::PhysicalOptimizerRule;
use datafusion::physical_plan::buffer::BufferExec;
use datafusion::physical_plan::coalesce_partitions::CoalescePartitionsExec;
use datafusion::physical_plan::limit::GlobalLimitExec;
use datafusion::physical_plan::repartition::RepartitionExec;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, ExecutionPlanProperties, Partitioning,
    PlanProperties, SendableRecordBatchStream,
};
use datafusion::prelude::SessionConfig;

/// The session every browser run executes in: one target partition, and
/// [`OneTask`] after every built-in rule.
#[must_use]
pub fn session() -> SessionContext {
    let state = SessionStateBuilder::new()
        .with_config(SessionConfig::new().with_target_partitions(1))
        .with_default_features()
        .with_physical_optimizer_rule(Arc::new(OneTask))
        .build();
    SessionContext::new_with_state(state)
}

/// Rewrites a physical plan so that executing it never spawns a Tokio task.
#[derive(Debug)]
pub struct OneTask;

impl PhysicalOptimizerRule for OneTask {
    fn optimize(
        &self,
        plan: Arc<dyn ExecutionPlan>,
        _: &ConfigOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let plan = plan
            .transform_up(|node| {
                if let Some(coalesce) = node.downcast_ref::<CoalescePartitionsExec>() {
                    let merged = one_task(Arc::clone(coalesce.input()), coalesce.fetch());
                    return Ok(Transformed::yes(merged));
                }
                if let Some(buffer) = node.downcast_ref::<BufferExec>() {
                    return Ok(Transformed::yes(Arc::clone(buffer.input())));
                }
                if node.is::<RepartitionExec>() {
                    return Err(DataFusionError::Plan(
                        "RepartitionExec spawns a Tokio task per input partition, and the \
                         browser has no runtime to spawn it on"
                            .to_owned(),
                    ));
                }
                Ok(Transformed::no(node))
            })?
            .data;
        if plan.output_partitioning().partition_count() > 1 {
            return Ok(one_task(plan, None));
        }
        Ok(plan)
    }

    fn name(&self) -> &'static str {
        "one_task"
    }

    fn schema_check(&self) -> bool {
        true
    }
}

fn one_task(input: Arc<dyn ExecutionPlan>, fetch: Option<usize>) -> Arc<dyn ExecutionPlan> {
    let merged: Arc<dyn ExecutionPlan> = Arc::new(OneTaskCoalesceExec::new(input));
    match fetch {
        Some(fetch) => Arc::new(GlobalLimitExec::new(merged, 0, Some(fetch))),
        None => merged,
    }
}

/// `CoalescePartitionsExec` without the spawn: every input partition is polled
/// from the one output stream, in whatever order they are ready.
#[derive(Debug)]
pub struct OneTaskCoalesceExec {
    input: Arc<dyn ExecutionPlan>,
    properties: Arc<PlanProperties>,
}

impl OneTaskCoalesceExec {
    fn new(input: Arc<dyn ExecutionPlan>) -> Self {
        let mut eq = input.equivalence_properties().clone();
        eq.clear_orderings();
        eq.clear_per_partition_constants();
        let properties = PlanProperties::new(
            eq,
            Partitioning::UnknownPartitioning(1),
            input.pipeline_behavior(),
            input.boundedness(),
        );
        Self {
            input,
            properties: Arc::new(properties),
        }
    }
}

impl DisplayAs for OneTaskCoalesceExec {
    fn fmt_as(&self, _: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OneTaskCoalesceExec")
    }
}

impl ExecutionPlan for OneTaskCoalesceExec {
    fn name(&self) -> &'static str {
        "OneTaskCoalesceExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
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
        if partition != 0 {
            return Err(DataFusionError::Internal(format!(
                "OneTaskCoalesceExec has one partition, asked for {partition}"
            )));
        }
        let streams = (0..self.input.output_partitioning().partition_count())
            .map(|i| self.input.execute(i, Arc::clone(&context)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            futures::stream::select_all(streams),
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::Int64Array;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::catalog::MemTable;
    use datafusion::physical_optimizer::PhysicalOptimizerRule;
    use datafusion::physical_plan::repartition::RepartitionExec;
    use datafusion::physical_plan::{ExecutionPlan, ExecutionPlanProperties, Partitioning};
    use datafusion::prelude::{SessionConfig, col};

    use super::{OneTask, session};

    fn names(plan: &Arc<dyn ExecutionPlan>, out: &mut Vec<String>) {
        out.push(plan.name().to_owned());
        for child in plan.children() {
            names(child, out);
        }
    }

    fn table(values: &[i64]) -> Arc<MemTable> {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(values.to_vec()))],
        )
        .expect("batch");
        Arc::new(MemTable::try_new(schema, vec![vec![batch]]).expect("table"))
    }

    #[test]
    fn a_deduplicated_union_plans_with_no_spawning_operator_and_runs_without_a_runtime() {
        let ctx = session();
        assert_eq!(ctx.copied_config().target_partitions(), 1);
        ctx.register_table("a", table(&[1, 2])).expect("a");
        ctx.register_table("b", table(&[2, 3])).expect("b");
        futures::executor::block_on(async {
            let union = ctx
                .table("a")
                .await
                .expect("a")
                .union(ctx.table("b").await.expect("b"))
                .expect("union")
                .distinct_on(
                    vec![col("n")],
                    vec![col("n")],
                    Some(vec![col("n").sort(true, false)]),
                )
                .expect("distinct");
            let plan = union.clone().create_physical_plan().await.expect("plan");
            let mut seen = Vec::new();
            names(&plan, &mut seen);
            assert!(
                seen.iter().any(|n| n == "OneTaskCoalesceExec")
                    && !seen.iter().any(|n| [
                        "CoalescePartitionsExec",
                        "RepartitionExec",
                        "BufferExec"
                    ]
                    .contains(&n.as_str())),
                "{seen:?}"
            );
            assert_eq!(plan.output_partitioning().partition_count(), 1);
            let rows: usize = union
                .collect()
                .await
                .expect("collect")
                .iter()
                .map(RecordBatch::num_rows)
                .sum();
            assert_eq!(rows, 3);
        });
    }

    #[test]
    fn a_repartition_is_refused_rather_than_left_to_panic() {
        let ctx = datafusion::prelude::SessionContext::new_with_config(
            SessionConfig::new().with_target_partitions(1),
        );
        ctx.register_table("a", table(&[1])).expect("a");
        let plan = futures::executor::block_on(async {
            ctx.table("a")
                .await
                .expect("a")
                .create_physical_plan()
                .await
                .expect("plan")
        });
        let repartitioned: Arc<dyn ExecutionPlan> =
            Arc::new(RepartitionExec::try_new(plan, Partitioning::RoundRobinBatch(2)).expect("r"));
        let err = OneTask
            .optimize(repartitioned, ctx.copied_config().options())
            .expect_err("refused");
        assert!(err.to_string().contains("RepartitionExec"), "{err}");
    }
}
