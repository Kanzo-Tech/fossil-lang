//! DataFusion backend for the property-graph MIR (paso 3 — arranque vertex-only).
//!
//! Consumes a [`fossil_mir::lower_to_mir_pg`] graph and runs the VERTEX as a
//! DataFusion plan: read the CSV source, project `id AS subject` + each prop, and
//! collect the [`RecordBatch`]es. This validates the MIR-PG → DataFusion path (the
//! core of paso 3). Still TODO (next increments): the dense_id assignment +
//! CSR/CSC edge resolution + GraphAr manifests (`DataSink`), `EmitEdge`, the
//! `Call` UDFs, and the wasm build (cut `arrow ipc_compression`/zstd).

use datafusion::arrow::record_batch::RecordBatch;
use datafusion::logical_expr::{binary_expr, Expr as DfExpr, Operator};
use datafusion::prelude::{col, lit, CsvReadOptions, SessionContext};
use fossil_hir::MappingLoc;
use fossil_mir::{lower_to_mir_pg, Expr, Op};

/// Execute a mapping's vertex on DataFusion: read its CSV source, project
/// `id AS subject` + the props, and collect the rows. Vertex-only arranque.
///
/// # Errors
/// Propagates DataFusion read/plan/execute errors.
pub async fn execute_vertex<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> datafusion::error::Result<Vec<RecordBatch>> {
    let mir = lower_to_mir_pg(db, mapping);
    let ops = mir.ops(db);

    let uri = ops
        .iter()
        .find_map(|o| match o {
            Op::Source { uri, .. } => Some(uri.to_string()),
            _ => None,
        })
        .expect("lower_to_mir_pg always emits a Source");
    let (id, props) = ops
        .iter()
        .find_map(|o| match o {
            Op::EmitVertex { id, props, .. } => Some((id.clone(), props.clone())),
            _ => None,
        })
        .expect("lower_to_mir_pg always emits an EmitVertex");

    let ctx = SessionContext::new();
    let df = ctx.read_csv(uri.as_str(), CsvReadOptions::new()).await?;

    let mut exprs = vec![render(&id).alias("subject")];
    for p in &props {
        exprs.push(render(&p.value).alias(p.name.as_str()));
    }
    df.select(exprs)?.collect().await
}

/// Render a MIR [`Expr`] to a DataFusion logical [`DfExpr`]. Vertex-only covers
/// `ColRef` / `LitString` / `Concat` / `Assert`; `Call` / `BinOp` / `LitBool` are
/// deferred to the full paso-3 render (no vertex-only mapping uses them in
/// id/prop positions).
fn render(e: &Expr<'_>) -> DfExpr {
    match e {
        Expr::LitString(s) => lit(s.to_string()),
        Expr::ColRef { column, .. } => col(column.as_str()),
        Expr::Concat(a, b) => binary_expr(render(a), Operator::StringConcat, render(b)),
        Expr::Assert { inner, .. } => render(inner),
        other => unimplemented!("render MIR Expr → DataFusion (paso 3 full): {other:?}"),
    }
}
