//! **Experiment, not a feature a host builds.** Every plan the engine collects,
//! rendered as `DuckDB` SQL by `DataFusion`'s unparser and kept beside what it
//! read and what `DataFusion` answered, so `tests/unparse.rs` can run the SQL
//! on `DuckDB` and compare rows and column types.
//!
//! Off unless the `unparse` feature is on and `FOSSIL_UNPARSE_DIR` names a
//! directory. Each collect writes `<dir>/<pid>-<n>/`: `query.sql` (or
//! `error.txt` when the unparser refuses the plan), `result.parquet`, and one
//! `t<i>.parquet` per table the plan scans, named in `tables.tsv`.

use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
use datafusion::logical_expr::{LogicalPlan, LogicalPlanBuilder};
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::prelude::DataFrame;
use datafusion_sql::unparser::Unparser;
use datafusion_sql::unparser::dialect::DuckDBDialect;

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Record `df`, whose collected answer is `batches`. Never fails the run: an
/// experiment that breaks the engine measures nothing.
pub async fn record(df: DataFrame, batches: &[RecordBatch]) {
    let Ok(root) = std::env::var("FOSSIL_UNPARSE_DIR") else {
        return;
    };
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(&root).join(format!("{}-{n:04}", std::process::id()));
    if let Err(e) = write(df, batches, &dir).await {
        let _ = fs::write(dir.join("record-error.txt"), e);
    }
}

async fn write(df: DataFrame, batches: &[RecordBatch], dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let schema: SchemaRef = Arc::new(df.schema().as_arrow().clone());
    parquet(&dir.join("result.parquet"), schema, batches)?;
    let (state, plan) = df.into_parts();
    match Unparser::new(&DuckDBDialect::new()).plan_to_sql(&plan) {
        Ok(sql) => fs::write(dir.join("query.sql"), sql.to_string()),
        Err(e) => fs::write(dir.join("error.txt"), e.to_string()),
    }
    .map_err(|e| e.to_string())?;
    let mut scans = Vec::new();
    plan.apply(|node| {
        if let LogicalPlan::TableScan(scan) = node {
            scans.push((scan.table_name.clone(), Arc::clone(&scan.source)));
        }
        Ok(TreeNodeRecursion::Continue)
    })
    .map_err(|e| e.to_string())?;
    let mut tables = String::new();
    for (i, (name, source)) in scans.into_iter().enumerate() {
        let file = format!("t{i}.parquet");
        let scan = LogicalPlanBuilder::scan(name.clone(), Arc::clone(&source), None)
            .and_then(LogicalPlanBuilder::build)
            .map_err(|e| e.to_string())?;
        let rows = DataFrame::new(state.clone(), scan)
            .collect()
            .await
            .map_err(|e| e.to_string())?;
        parquet(&dir.join(&file), source.schema(), &rows)?;
        tables.push_str(&format!("{name}\t{file}\n"));
    }
    fs::write(dir.join("tables.tsv"), tables).map_err(|e| e.to_string())
}

fn parquet(path: &Path, schema: SchemaRef, batches: &[RecordBatch]) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let mut writer = ArrowWriter::try_new(file, schema, None).map_err(|e| e.to_string())?;
    for batch in batches {
        writer.write(batch).map_err(|e| e.to_string())?;
    }
    writer.close().map(|_| ()).map_err(|e| e.to_string())
}
