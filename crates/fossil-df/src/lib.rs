//! DataFusion backend for the property-graph MIR (paso 3 — vertex phase).
//!
//! Consumes a [`fossil_mir::lower_to_mir_pg`] graph and materialises the
//! GraphAr VERTEX layout on DataFusion: read the source, project
//! `id AS subject` + each prop + the `x`/`y`/`cluster_id` layout placeholders,
//! dedup single-valued shapes, sort by `subject` for a deterministic dense id,
//! `collect()`, and prepend `dense_id` (`0..N-1`, sort order). The result is
//! registered in the [`SessionContext`] so the edge phase can resolve endpoint
//! IRIs against it in memory (the C4 hard barrier: vertices before edges).
//!
//! Column shape (writer-W0b contract, see design §A1):
//! `dense_id(u32), subject(varchar IRI), <props…>, x(f32=0), y(f32=0), cluster_id(u32=0)`.
//!
//! Still TODO (next increments): the edge phase (`EmitEdge` → join vertex
//! tables → CSR/CSC), the GraphAr manifests + `RunStatus`, the `Call` UDFs, and
//! the wasm build (cut `arrow ipc_compression`/zstd).

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, UInt32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::logical_expr::{binary_expr, Expr as DfExpr, Operator};
use datafusion::prelude::{col, lit, CsvReadOptions, SessionContext};
use fossil_hir::MappingLoc;
use fossil_mir::{lower_to_mir_pg, Expr, Op, VProp};

/// A materialised GraphAr vertex table: the `type_name`-named relation and its
/// `RecordBatch`es in the writer-W0b column shape (`dense_id`-prefixed). The
/// batches are also registered in the executor's [`SessionContext`] under
/// `type_name` so the edge phase joins them without re-reading Parquet.
#[derive(Debug)]
pub struct VertexTable {
    pub type_name: String,
    pub rdf_type: Option<String>,
    pub batches: Vec<RecordBatch>,
}

/// Materialise a mapping's VERTEX on DataFusion and register it in `ctx`.
///
/// Reads the CSV source, projects `id AS subject` + props + `x`/`y`/`cluster_id`
/// placeholders, dedups when the shape is single-valued, sorts by `subject`
/// (deterministic dense id — closes the writer's no-`ORDER BY` gap, design §A1),
/// collects, and prepends `dense_id`. Registers the batches as a [`MemTable`]
/// named after the vertex type for the edge phase.
///
/// # Errors
/// Propagates DataFusion read/plan/execute errors.
pub async fn execute_vertex<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> datafusion::error::Result<VertexTable> {
    let mir = lower_to_mir_pg(db, mapping);
    let ops = mir.ops(db);

    let uri = ops
        .iter()
        .find_map(|o| match o {
            Op::Source { uri, .. } => Some(uri.to_string()),
            _ => None,
        })
        .expect("lower_to_mir_pg always emits a Source");
    let (type_name, rdf_type, id, dedup, props) = ops
        .iter()
        .find_map(|o| match o {
            Op::EmitVertex {
                type_name,
                rdf_type,
                id,
                dedup,
                props,
                ..
            } => Some((
                type_name.to_string(),
                rdf_type.as_ref().map(ToString::to_string),
                id.clone(),
                *dedup,
                props.clone(),
            )),
            _ => None,
        })
        .expect("lower_to_mir_pg always emits an EmitVertex");

    let df = ctx.read_csv(uri.as_str(), CsvReadOptions::new()).await?;

    let subject = render(&id);
    let exprs = vertex_projection(subject.clone(), &props);
    let projected = if dedup {
        // Single-valued shape: one vertex per subject IRI. `distinct_on` keeps the
        // first row per IRI in sort order — deterministic, like the writer's
        // `DISTINCT ON`. The `on`/`sort` exprs run on the SOURCE schema, so they
        // reference the raw IRI expression, not the `subject` projection alias.
        df.distinct_on(
            vec![subject.clone()],
            exprs,
            Some(vec![subject.sort(true, false)]),
        )?
    } else {
        df.select(exprs)?.sort(vec![col("subject").sort(true, false)])?
    };

    let batches = prepend_dense_id(projected.collect().await?)?;

    register_batches(ctx, &type_name, &batches)?;
    Ok(VertexTable {
        type_name,
        rdf_type,
        batches,
    })
}

/// The vertex projection exprs: `id AS subject`, each prop, and the
/// `x`/`y`/`cluster_id` layout placeholders the discovery viewer expects
/// (`RESERVED_VERTEX_COLUMNS`, design §A2). Layout/cluster are filled by W3;
/// here they are deterministic zeros.
fn vertex_projection(subject: DfExpr, props: &[VProp<'_>]) -> Vec<DfExpr> {
    let mut exprs = vec![subject.alias("subject")];
    for p in props {
        exprs.push(render(&p.value).alias(p.name.as_str()));
    }
    exprs.push(lit(0.0_f32).alias("x"));
    exprs.push(lit(0.0_f32).alias("y"));
    exprs.push(lit(0_u32).alias("cluster_id"));
    exprs
}

/// Prepend `dense_id` (`u32`, `0..N-1` in batch order) to each collected batch.
/// The batches arrive globally sorted by `subject` (the plan sorts before
/// collect), so a running offset yields the deterministic dense index the
/// edge phase joins against.
fn prepend_dense_id(batches: Vec<RecordBatch>) -> datafusion::error::Result<Vec<RecordBatch>> {
    let mut out = Vec::with_capacity(batches.len());
    let mut offset: u32 = 0;
    for batch in batches {
        let n = batch.num_rows() as u32;
        let ids: ArrayRef = Arc::new(UInt32Array::from_iter_values(offset..offset + n));

        let mut fields: Vec<Arc<Field>> =
            vec![Arc::new(Field::new("dense_id", DataType::UInt32, false))];
        fields.extend(batch.schema().fields().iter().cloned());
        let mut columns: Vec<ArrayRef> = vec![ids];
        columns.extend(batch.columns().iter().cloned());

        out.push(RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)?);
        offset += n;
    }
    Ok(out)
}

/// Register in-memory `batches` as a [`MemTable`] named `name`, so a later plan
/// (the edge phase) can scan them without re-reading from object storage.
fn register_batches(
    ctx: &SessionContext,
    name: &str,
    batches: &[RecordBatch],
) -> datafusion::error::Result<()> {
    let schema = batches
        .first()
        .map_or_else(|| Arc::new(Schema::empty()), RecordBatch::schema);
    let table = MemTable::try_new(schema, vec![batches.to_vec()])?;
    ctx.register_table(name, Arc::new(table))?;
    Ok(())
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
