//! DataFusion backend for the property-graph MIR.
//!
//! Consumes a [`fossil_mir::lower_to_mir_pg`] graph and materialises the
//! GraphAr VERTEX layout on DataFusion: read the source, project
//! `id AS subject` + each prop + the `x`/`y`/`cluster_id` layout placeholders,
//! dedup single-valued shapes, sort by `subject` for a deterministic dense id,
//! `collect()`, and prepend `dense_id` (`0..N-1`, sort order). The result is
//! registered in the [`SessionContext`] so the edge phase can resolve endpoint
//! IRIs against it in memory (the hard barrier: vertices before edges).
//!
//! Column shape (writer-W0b contract):
//! `dense_id(u32), subject(varchar IRI), <props…>, x(f32=0), y(f32=0), cluster_id(u32=0)`.
//!
//! The crate compiles to `wasm32-unknown-unknown` (the whole executor runs in
//! the browser). `zstd-sys` is an unavoidable C dep (datafusion 54 hardcodes
//! `arrow-ipc/zstd`), so the wasm build needs a wasm-capable clang — see the
//! `datafusion` entry in `Cargo.toml`.

pub mod files;
/// Deriving the generalisation a declared bound needs, over the same batches
/// [`privacy`] then measures — and separately from it, so the verifier still
/// repairs nothing.
pub mod generalize;
/// The relational operators executed: the walk from an emit op back to the
/// sources it reads. Its own module doc lists which operators run here.
pub mod plan;
/// Write-time verification of a declared privacy bound: the check that runs
/// after the corpus is a value and before any of it is a file.
pub mod privacy;
pub mod rdf;
/// What a run tells its caller: the manifest it wrote, the destination, and the
/// edges the join discarded. Built by [`report::RunReport::of`] and by nothing
/// else.
pub mod report;
/// The catalog rendered for this engine: which `DataFusion` function each
/// stdlib entry becomes, and which rows this engine cannot render.
pub mod stdlib;

#[cfg(not(target_arch = "wasm32"))]
pub mod sink;

/// Re-exported so callers name the program-resident output descriptor that
/// [`execute_graph`] / [`provider_bindings`] take: it is passed as an argument,
/// never read through `Db::system()`.
pub use fossil_descriptors_output::OutputDescriptorKind;
/// Re-exported so a host can classify a [`SourceRef`]'s format without depending
/// on `fossil-mir` directly (the browser host maps it to a fetch strategy).
pub use fossil_mir::SourceFormat;
/// The relation an op index produces — the seam a host (or a test that builds a
/// [`fossil_mir::MirGraph`] by hand) uses to materialise an intermediate.
pub use plan::plan_relation;
/// What `fossil run --output-json` prints, and what the browser executor hands
/// JS. Named at the crate root because it is the crate's answer, not a detail
/// of the module it is written in.
pub use report::RunReport;

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, UInt32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::Column;
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
#[cfg(not(target_arch = "wasm32"))]
use datafusion::execution::memory_pool::{FairSpillPool, TrackConsumersPool};
#[cfg(not(target_arch = "wasm32"))]
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::logical_expr::{Expr as DfExpr, JoinType, Operator, binary_expr};
use datafusion::prelude::{
    CsvReadOptions, DataFrame, JsonReadOptions, ParquetReadOptions, SessionContext, col, lit,
};
use fossil_base::SourceFile;
use fossil_graph_schema::{
    Cardinality, EdgeType as GraphEdge, GraphSchema, NodeType, Primitive, Property as NodeProp,
};
use fossil_hir::shapes::{inner_primitive, primitive_to_graphar};
use fossil_hir::{MappingLoc, def_map::def_map};
use fossil_locator::SourceAnchor;
use fossil_mem_probe::Probe;
use fossil_mir::{Expr, Op, VProp, apply_output_shape, lower_to_mir_pg};
use fossil_sinks::manifest::{
    AdjList, Container, DEFAULT_CHUNK_SIZE, EdgeInfo, GRAPHAR_VERSION, GraphInfo, Privacy,
    Property, PropertyGroup, TILE_CODES_FILE, VertexCodes, VertexIndex, VertexInfo, VertexLevels,
    data_type_name,
};

/// The materialised graph for a program: the canonical [`GraphSchema`] (the
/// single source of all type/predicate/cardinality metadata) plus the relation
/// data — vertex tables and edge tables (CSR + CSC) as in-memory `RecordBatch`es.
///
/// This is the universal substrate made concrete: **relations + a
/// graph-schema**. The GraphAr view (the manifest + Parquet) is materialized
/// *from* this; the data carriers hold no metadata of their own — it all lives
/// in [`schema`](Self::schema).
#[derive(Debug)]
pub struct GraphArData {
    pub schema: GraphSchema,
    pub vertices: Vec<VertexTable>,
    pub edges: Vec<EdgeTable>,
    /// **What these bytes guarantee**, and the one field here that is not data.
    ///
    /// [`Privacy::Undeclared`] until [`privacy::verify`] has measured the
    /// release against a policy and replaced it. It is carried on the value
    /// rather than passed to [`Self::manifest`] so that the bound and the rows
    /// it was measured over cannot be separated: a manifest built from this
    /// value describes this corpus, and there is no call shape in which a
    /// caller supplies a bound for rows it did not check.
    ///
    /// A run with no policy leaves it [`Privacy::Undeclared`], which is written
    /// to `graph.graph.yml` as such. That is the whole mitigation for the
    /// binding being a host argument today: forgetting it does not produce a
    /// corpus that looks bounded, it produces one that says out loud that
    /// nothing was checked, and `apps/corpus`'s `declared-privacy` says so
    /// again to whoever receives it.
    pub privacy: Privacy,
}

/// A materialised edge's adjacency data in both orientations — `by_source` (CSR,
/// `ORDER BY src_dense, dst_dense`) and `by_target` (CSC). Both carry the same
/// two `u32` columns (`src_dense`, `dst_dense`); only the row order differs. The
/// `(src_type, label, dst_type)` triple identifies the edge in the schema and
/// names its `<src>_<label>_<dst>` directory; all other metadata is in the
/// [`GraphSchema`].
#[derive(Debug)]
pub struct EdgeTable {
    pub label: String,
    pub src_type: String,
    pub dst_type: String,
    pub by_source: Vec<RecordBatch>,
    pub by_target: Vec<RecordBatch>,
    /// Rows of this edge's input that resolved no endpoint pair, and so are not
    /// in either orientation. See [`execute_edge`] for why they are discarded
    /// and [`report::EdgeDrops`] for where the number goes.
    pub dropped: u64,
}

/// One emitted GraphAr manifest YAML + its dataset-relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    pub rel_path: String,
    pub yaml: String,
}

/// Execute a whole program's mappings into the GraphAr graph.
///
/// Two phases with a hard barrier between them: **(1)** materialise *every*
/// vertex (assigning dense ids, registering each as a [`MemTable`]); **(2)**
/// resolve *every* edge by joining its endpoint IRIs against the now-registered
/// vertex tables in memory — an edge may point at a vertex owned by another
/// mapping, so all vertices must exist before any edge.
///
/// The caller owns the [`SessionContext`] so it can configure the source layer
/// before execution — the browser host registers an `ObjectStore` per signed
/// source URL and tunes schema inference — and so the registered vertex tables
/// outlive the call for inspection.
///
/// # Errors
/// Propagates DataFusion read/plan/execute errors.
pub async fn execute_graph<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> datafusion::error::Result<GraphArData> {
    // The run's anchor, derived from the PROGRAM and not from the caller: a
    // source URI is a path the program wrote, so the directory it resolves
    // against is the program's own. The executor used to hand DataFusion the
    // written path verbatim, which made the process's working directory the
    // anchor — so `io.csv("data/items.csv")` meant a different file depending
    // on where you invoked from, while `io.shex("shop.shex")` in the same
    // program was already resolved beside it.
    let program_dir = fossil_locator::program_dir(file.path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let mappings: Vec<MappingLoc<'db>> = def_map(db, file).mappings(db).clone();
    let mut probe = Probe::new(&format!("execute_graph — {} mapping(s)", mappings.len()));

    // Phase 1 (barrier): prepare every mapping's vertex projection, then merge
    // the mappings that emit the SAME type (UNION) before assigning dense ids —
    // a vertex type may be fed by several sources. Each type is registered
    // exactly once, so two mappings of one type can't clobber each other's
    // `MemTable`.
    let mut groups: Vec<(String, Vec<PreparedVertex>)> = Vec::new();
    for &mapping in &mappings {
        let prepared = prepare_vertex(ctx, db, mapping, descriptor, anchor).await?;
        match groups.iter_mut().find(|(t, _)| *t == prepared.node.label) {
            Some((_, group)) => group.push(prepared),
            None => groups.push((prepared.node.label.clone(), vec![prepared])),
        }
    }
    probe.mark("prepare vertices (lazy)");
    let mut vertices = Vec::with_capacity(groups.len());
    let mut nodes = Vec::with_capacity(groups.len());
    for (label, group) in groups {
        let (table, node) = finalize_vertex(ctx, group).await?;
        probe.mark(&format!("collect vertex {label}"));
        vertices.push(table);
        nodes.push(node);
    }

    // Phase 2: edges join the in-memory vertex tables (no Parquet re-read).
    let mut edges = Vec::new();
    let mut edge_types = Vec::new();
    for &mapping in &mappings {
        // Marked per `execute_edges` call and not per table: the whole call's
        // memory is already spent by the time it returns, so a mark inside the
        // loop over its results would bill all of it to the first table.
        let produced = execute_edges(ctx, db, mapping, descriptor, anchor).await?;
        probe.mark(&format!("collect {} edge table(s)", produced.len()));
        for (table, edge_type) in produced {
            edges.push(table);
            edge_types.push(edge_type);
        }
    }
    probe.finish();

    let schema = GraphSchema {
        nodes,
        edges: edge_types,
    };
    Ok(GraphArData {
        privacy: Privacy::Undeclared,
        schema,
        vertices,
        edges,
    })
}

/// A materialised vertex relation: the `label`-named table and its
/// `RecordBatch`es in the writer-W0b column shape (`dense_id`-prefixed). The
/// batches are also registered in the executor's [`SessionContext`] under
/// `label` so the edge phase joins them without re-reading Parquet. The node's
/// type/property metadata lives in the [`GraphSchema`], keyed by this `label`.
#[derive(Debug)]
pub struct VertexTable {
    pub label: String,
    pub batches: Vec<RecordBatch>,
}

/// A mapping's vertex projection before the dense-id barrier — the W0b columns
/// (`subject` + props + `x`/`y`/`cluster_id`) as an un-collected [`DataFrame`],
/// plus the [`NodeType`] schema it contributes. Several of these with the same
/// node `label` are UNIONed before dense ids are assigned.
struct PreparedVertex {
    node: NodeType,
    dedup: bool,
    projected: DataFrame,
}

/// Materialise a single mapping's VERTEX on DataFusion and register it — the
/// one-mapping convenience over [`prepare_vertex`] + [`finalize_vertex`].
/// Returns the table data plus the [`NodeType`] it contributes to the schema.
///
/// # Errors
/// Propagates DataFusion read/plan/execute errors.
pub async fn execute_vertex<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    let program_dir = fossil_locator::program_dir(mapping.file(db).path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let prepared = prepare_vertex(ctx, db, mapping, descriptor, anchor).await?;
    finalize_vertex(ctx, vec![prepared]).await
}

/// Materialise the VERTEX of an op list that is already lowered and already
/// descriptor-refined — [`execute_vertex`] without the `MappingLoc`.
///
/// This is the seam for an op list built by hand rather than by
/// [`lower_to_mir_pg`]. The operator algebra is defined whole and lowered in
/// part — every operator exists, and only some of them are emitted from source
/// — so the rest are reached this way, and that is how they are tested. `db` is
/// still needed — a [`VProp`]'s type is an interned handle.
///
/// # Errors
/// The list carries no [`Op::EmitVertex`], or any DataFusion read/plan/execute
/// error.
pub async fn execute_vertex_ops<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    ops: &[Op<'db>],
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    let prepared = prepare_vertex_ops(ctx, db, ops, anchor).await?;
    finalize_vertex(ctx, vec![prepared]).await
}

/// Refuse to execute a mapping whose lowering failed.
///
/// A poisoned [`MirGraph`] means lowering could not resolve something the graph
/// needs — an unresolvable source binding, a mapping with no usable `iri`. It
/// carries no ops, so executing it would either panic on the `expect`s below or
/// silently produce an empty graph. Neither is acceptable: the run must fail
/// with the reason, which the accumulated `Diagnostic` already carries
/// (`fossil_base` guarantees at least one).
///
/// Checking here is also what makes the `expect`s below sound: a graph that is
/// not poisoned always carries its `Source` and `EmitVertex`.
fn refuse_if_poisoned(
    mir: fossil_mir::MirGraph<'_>,
    db: &dyn fossil_base::Db,
) -> datafusion::error::Result<()> {
    if mir.error(db).is_some() {
        return Err(datafusion::error::DataFusionError::Plan(
            "the mapping did not compile; see the reported diagnostics".to_string(),
        ));
    }
    Ok(())
}

/// Project a mapping's source rows to the W0b vertex columns (no dedup/sort/
/// dense-id yet — those wait for [`finalize_vertex`], after the per-type union).
async fn prepare_vertex<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    descriptor: &OutputDescriptorKind,
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<PreparedVertex> {
    let mir = lower_to_mir_pg(db, mapping);
    refuse_if_poisoned(mir, db)?;
    // The program's `@rename`s: they govern the emitted column label, and the
    // checker resolved the body's property keys against the same table.
    let renames = def_map(db, mapping.file(db)).renames(db);
    let ops = apply_output_shape(mir.ops(db), &descriptor.to_graph_schema(&renames));
    prepare_vertex_ops(ctx, db, &ops, anchor).await
}

/// [`prepare_vertex`] over an op list that is already lowered and refined.
///
/// The projection reads the relation the `EmitVertex`'s `input` names — NOT
/// "the mapping's source". Before F5 those coincided (`input` was always 0);
/// with a pipeline in front of the emit they do not, and it is the index that
/// is the contract.
async fn prepare_vertex_ops<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    ops: &[Op<'db>],
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<PreparedVertex> {
    let (input, type_name, rdf_type, id, dedup, props) = ops
        .iter()
        .find_map(|o| match o {
            Op::EmitVertex {
                input,
                type_name,
                rdf_type,
                id,
                dedup,
                props,
            } => Some((
                *input,
                type_name.to_string(),
                rdf_type.as_ref().map(ToString::to_string),
                id.clone(),
                *dedup,
                props.clone(),
            )),
            _ => None,
        })
        .ok_or_else(|| {
            DataFusionError::Plan(
                "the op list emits no vertex: there is nothing to materialise".to_string(),
            )
        })?;

    let df = plan_relation(ctx, ops, input, anchor).await?;
    let projected = df.select(vertex_projection(render(&id), &props))?;
    let node = NodeType {
        label: type_name,
        iri: rdf_type,
        properties: props.iter().map(|p| node_property(db, p)).collect(),
    };

    Ok(PreparedVertex {
        node,
        dedup,
        projected,
    })
}

/// Finalise one vertex type from the mappings that emit it: UNION their
/// projections, dedup by `subject` when the shape is single-valued, sort by
/// `subject` for a deterministic dense id, `collect()`, prepend `dense_id`, and
/// register the table once under its type name.
///
/// Returns the materialised [`VertexTable`] plus the [`NodeType`] it contributes
/// to the graph-schema — the group's first member carries the canonical schema
/// (all mappings of one type share the same shape).
async fn finalize_vertex(
    ctx: &SessionContext,
    group: Vec<PreparedVertex>,
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    let mut group = group.into_iter();
    let PreparedVertex {
        node,
        dedup,
        projected,
    } = group.next().expect("a type group is never empty");

    let mut df = projected;
    for next in group {
        df = df.union(next.projected)?; // UNION ALL — dedup (if any) happens below
    }

    let by_subject = vec![col("subject").sort(true, false)];
    let sorted = if dedup {
        // One vertex per subject IRI across all source mappings. `subject` now
        // exists (post-projection), so dedup on the column, not the raw IRI expr.
        let keep: Vec<DfExpr> = df
            .schema()
            .fields()
            .iter()
            .map(|f| DfExpr::Column(Column::new_unqualified(f.name().as_str())))
            .collect();
        df.distinct_on(vec![col("subject")], keep, Some(by_subject))?
    } else {
        df.sort(by_subject)?
    };

    let batches = prepend_dense_id(sorted.collect().await?)?;
    register_batches(ctx, &node.label, &batches)?;
    Ok((
        VertexTable {
            label: node.label.clone(),
            batches,
        },
        node,
    ))
}

/// Build a schema [`Property`](NodeProp) for a vertex prop: peel its canonical
/// type to a [`Primitive`] → the format-neutral [`Primitive`] (the manifest
/// derives its `data_type` spelling from it); the predicate IRI + shape
/// cardinality ride along. Falls back to `string` when the type carries no
/// primitive (the legacy default).
fn node_property(db: &dyn fossil_base::Db, prop: &VProp<'_>) -> NodeProp {
    NodeProp {
        name: prop.name.to_string(),
        datatype: inner_primitive(db, prop.ty).unwrap_or(Primitive::String),
        iri: prop.rdf_uri.as_ref().map(ToString::to_string),
        cardinality: if prop.single_valued {
            Cardinality::Single
        } else {
            Cardinality::Multi
        },
    }
}

/// The vertex projection exprs: `id AS subject`, each prop, and the
/// `x`/`y`/`cluster_id` layout placeholders the discovery viewer expects
/// (`RESERVED_VERTEX_COLUMNS`). Layout/cluster are filled by the layout pass;
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

        out.push(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?);
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

/// Resolve every [`Op::EmitEdge`] of one mapping into its adjacency data
/// ([`EdgeTable`]) plus the [`EdgeType`](GraphEdge) it contributes to the
/// schema. Reads the mapping's source once and joins it against the registered
/// vertex tables.
async fn execute_edges<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    descriptor: &OutputDescriptorKind,
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<Vec<(EdgeTable, GraphEdge)>> {
    let mir = lower_to_mir_pg(db, mapping);
    refuse_if_poisoned(mir, db)?;
    let renames = def_map(db, mapping.file(db)).renames(db);
    let ops = apply_output_shape(mir.ops(db), &descriptor.to_graph_schema(&renames));
    let ops = ops.as_slice();

    let mut out = Vec::new();
    for op in ops {
        if let Op::EmitEdge {
            input,
            edge_type,
            rdf_uri,
            src_type,
            dst_type,
            src_id,
            dst_id,
            single_valued,
        } = op
        {
            let rows = plan_relation(ctx, ops, *input, anchor).await?;
            let table = execute_edge(
                ctx,
                rows,
                edge_type,
                src_type,
                dst_type,
                src_id,
                dst_id,
                *single_valued,
            )
            .await?;
            let edge_type = GraphEdge {
                label: edge_type.to_string(),
                iri: rdf_uri.as_ref().map(ToString::to_string),
                source: src_type.to_string(),
                destination: dst_type.to_string(),
                cardinality: if *single_valued {
                    Cardinality::Single
                } else {
                    Cardinality::Multi
                },
            };
            out.push((table, edge_type));
        }
    }
    Ok(out)
}

/// Materialise one edge type. Projects the edge op's input relation (`rows`) to
/// `src_iri`/`dst_iri`, joins both against the registered vertex tables to
/// resolve endpoint IRIs to dense ids, then sorts the `(src_dense, dst_dense)`
/// pairs into CSR (`by_source`) and CSC (`by_target`).
///
/// # The join is inner, and the discard is counted
///
/// A row whose `src_iri` or `dst_iri` names a subject no vertex carries
/// resolves nothing and does not become an edge. **That is intended and it
/// stays** — a corpus cannot hold an edge to a vertex that is not there, and
/// `fossil-cli`'s conformance assertion 4 reads every endpoint back and
/// fails on a `dense_id` no vertex has.
///
/// What it stopped being is silent. It reported nothing at any log level, and
/// the only trace was an [`EdgeInfo::edge_count`] smaller than the input's row
/// count — a comparison nobody is obliged to make. [`EdgeTable::dropped`] is
/// `candidates − resolved`: the extra `count()` is one more pass over the edge's
/// input relation, which the run already scans twice (once per orientation).
///
/// This used to say it mirrored `fossil-sinks`'s `writer.rs`. There is no
/// second writer to mirror any more — `fossil-sinks/src/` is the manifest model
/// and nothing else, so THIS is where an edge becomes CSR/CSC. What still reads
/// the pair afterwards is the layout pass, which re-sorts the tiles in place
/// (`fossil-layout/src/layout.rs`).
#[allow(clippy::too_many_arguments)] // the edge spec is a flat tuple, not worth a struct here
async fn execute_edge(
    ctx: &SessionContext,
    rows: DataFrame,
    label: &str,
    src_type: &str,
    dst_type: &str,
    src_id: &Expr<'_>,
    dst_id: &Expr<'_>,
    single_valued: bool,
) -> datafusion::error::Result<EdgeTable> {
    let edge_src = rows.select(vec![
        render(src_id).alias("src_iri"),
        render(dst_id).alias("dst_iri"),
    ])?;
    // A multi-valued edge's `dst_iri` is a `List` (the RDF pivot kept every
    // object); UNNEST expands it to one (src, dst) row per element — the
    // DataFusion-native counterpart of the writer's `UNNEST(list(...))`. A
    // single-valued edge's `dst_iri` is already scalar.
    let edge_src = if single_valued {
        edge_src
    } else {
        edge_src.unnest_columns(&["dst_iri"])?
    };
    // Pre-project each vertex table to (subject, dense) with disjoint names so
    // the two joins never collide on `subject`/`dense_id`.
    let src_v = ctx.table(src_type).await?.select(vec![
        col("subject").alias("v_src_subject"),
        col("dense_id").alias("src_dense"),
    ])?;
    let dst_v = ctx.table(dst_type).await?.select(vec![
        col("subject").alias("v_dst_subject"),
        col("dense_id").alias("dst_dense"),
    ])?;

    // Before the join, because the join is what consumes it: how many rows were
    // offered as edges. Everything the two `select`s above did is preserved —
    // this is the same relation the inner join reads, counted.
    let candidates = edge_src.clone().count().await? as u64;

    let resolved = edge_src
        .join(
            src_v,
            JoinType::Inner,
            &["src_iri"],
            &["v_src_subject"],
            None,
        )?
        .join(
            dst_v,
            JoinType::Inner,
            &["dst_iri"],
            &["v_dst_subject"],
            None,
        )?
        .select(vec![col("src_dense"), col("dst_dense")])?;

    let by_source = resolved
        .clone()
        .sort(vec![
            col("src_dense").sort(true, false),
            col("dst_dense").sort(true, false),
        ])?
        .collect()
        .await?;
    let by_target = resolved
        .sort(vec![
            col("dst_dense").sort(true, false),
            col("src_dense").sort(true, false),
        ])?
        .collect()
        .await?;

    // `saturating_sub` because the subtraction is only exact while a subject
    // identifies at most one vertex: a type materialised without dedup can hold
    // two rows with the same `subject`, and then one candidate resolves to two
    // edges. That corpus already violates `identity-is-the-subject`
    // (`apps/corpus/guards/guards.mjs`), which is the guard that catches it.
    let dropped = candidates.saturating_sub(count_rows(&by_source));

    Ok(EdgeTable {
        label: label.to_string(),
        src_type: src_type.to_string(),
        dst_type: dst_type.to_string(),
        by_source,
        by_target,
        dropped,
    })
}

// `resolve_source_uri` lived here and expanded `@conn` aliases and nothing else
// — it was called THE source-resolution rule, and it was half of one. A
// reference also has to be anchored somewhere, and every caller of this was
// left to decide that for itself: the executor decided "nowhere", which is the
// process's working directory. Both halves are one function now,
// `fossil_locator::SourceAnchor::locator`, and it cannot be called without an
// anchor.

/// Every resolved source URI + format + binding name of a lowered mapping, in
/// op order. A mapping had exactly one `Source` until a `join` gave it two,
/// so this is a list and not a lookup — the second source of a
/// joined mapping is as much a source as the first, and a host that fetches
/// only the first would run the program against half its inputs.
///
/// The raw `@conn` alias is resolved through `connections` before use, by
/// `SourceAnchor::locator`. The `binding` is the table name a `Provider` source
/// is registered under (the host pre-registers it; [`read_source`] scans it);
/// object-store formats ignore it.
fn sources_of<'db>(
    ops: &[Op<'db>],
    anchor: SourceAnchor<'_>,
) -> Vec<(String, SourceFormat, String)> {
    ops.iter()
        .filter_map(|o| match o {
            Op::Source {
                uri,
                format,
                binding,
                ..
            } => Some((anchor.locator(uri), format.clone(), binding.to_string())),
            _ => None,
        })
        .collect()
}

/// Read a source into a [`DataFrame`], dispatching on its [`SourceFormat`] — the
/// two halves of the host input seam:
///
/// - **Object-store formats** (`io.csv`/`io.json`/`io.parquet`) stream through
///   the [`SessionContext`]'s registered `ObjectStore` (the local filesystem by
///   default; an HTTP/signed-URL/S3 store the host registers for remote `uri`s).
///   `uri` may be local or remote — the host owns that registration, NOT this
///   crate. Streaming reads preserve larger-than-RAM behaviour, so these are
///   never pre-materialised.
/// - **`Provider` formats** (RDF) are NOT DataFusion-native: the host reads the
///   source bytes (fs natively, `fetch` in the browser) and registers the
///   decoded relation as a `MemTable` under `binding` *before* this call (see
///   [`register_rdf`] / [`provider_bindings`]). Here we simply scan that
///   pre-registered table — RDF stays at the I/O border, the executor never
///   parses it.
pub(crate) async fn read_source(
    ctx: &SessionContext,
    uri: &str,
    format: &SourceFormat,
    binding: &str,
) -> datafusion::error::Result<DataFrame> {
    match format {
        SourceFormat::Csv => ctx.read_csv(uri, csv_options()).await,
        SourceFormat::Json => read_json_source(ctx, uri).await,
        SourceFormat::Parquet => ctx.read_parquet(uri, ParquetReadOptions::default()).await,
        SourceFormat::Provider { name } => {
            let table = provider_table_name(binding);
            ctx.table(&table).await.map_err(|e| {
                DataFusionError::Execution(format!(
                    "io.{name} source `{binding}` is not registered — the host must decode it \
                     (read the bytes of `{uri}` + register via `register_rdf`) before execute_graph: {e}"
                ))
            })
        }
    }
}

/// Read `io.json`, whichever of the two JSON shapes the file is.
///
/// **DataFusion's `read_json` is newline-delimited only**, and `sightings`
/// failed on it with `Json error: Not valid JSON: EOF while parsing a list` —
/// the fixture is an array, which is what a `.json` file ordinarily holds.
/// Letting the engine's reader decide what `io.json` MEANS would be the
/// accident redefining the design, so the constructor reads both and the first
/// non-whitespace byte says which.
///
/// **An array is not a streaming format, and that is the format's property and
/// not this reader's.** You cannot know where record N ends without parsing
/// from the start, so any array reader is bounded by the file. The docblock on
/// [`read_source`] says object-store formats are never pre-materialised; this
/// is the exception and it is forced. NDJSON is the streaming spelling of the
/// same data and takes the streaming path below, unchanged.
async fn read_json_source(ctx: &SessionContext, uri: &str) -> datafusion::error::Result<DataFrame> {
    use datafusion::arrow::json::ReaderBuilder;
    use datafusion::arrow::json::reader::infer_json_schema_from_iterator;

    let bytes = fetch_bytes(ctx, uri).await?;
    // The sniff, and it is one byte. A JSON array starts with `[`; NDJSON's
    // first record is an object, a number or a string.
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'[') {
        return ctx
            .read_json(
                uri,
                JsonReadOptions::default().schema_infer_max_records(usize::MAX),
            )
            .await;
    }

    let values: Vec<serde_json::Value> = serde_json::from_slice(&bytes).map_err(|e| {
        DataFusionError::Execution(format!("io.json source `{uri}` is not valid JSON: {e}"))
    })?;
    if values.is_empty() {
        return Err(DataFusionError::Execution(format!(
            "io.json source `{uri}` is an empty array, so there is no schema to infer"
        )));
    }
    // Every record, like the CSV path's `schema_infer_max_records(usize::MAX)`:
    // a column whose early values look numeric and later turn stringy is
    // mis-typed by a sample, and the file is already in memory.
    let schema = Arc::new(
        infer_json_schema_from_iterator(values.iter().map(|v| Ok(v.clone())))
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?,
    );
    let mut decoder = ReaderBuilder::new(Arc::clone(&schema))
        .build_decoder()
        .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
    decoder
        .serialize(&values)
        .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
    let batch = decoder
        .flush()
        .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?
        .ok_or_else(|| {
            DataFusionError::Execution(format!("io.json source `{uri}` decoded to no rows"))
        })?;
    ctx.read_batch(batch)
}

/// The bytes of `uri`, through the `ObjectStore` the host registered.
///
/// No new seam: this is the same store [`read_source`]'s streaming paths go
/// through, so a remote `uri` a host signed is reachable here for the same
/// reason it is there.
async fn fetch_bytes(ctx: &SessionContext, uri: &str) -> datafusion::error::Result<Vec<u8>> {
    let url = datafusion::datasource::listing::ListingTableUrl::parse(uri)?;
    let store = ctx.runtime_env().object_store(&url)?;
    let data = store
        .get_opts(
            url.prefix(),
            datafusion::object_store::GetOptions::default(),
        )
        .await
        .map_err(|e| DataFusionError::Execution(format!("read `{uri}`: {e}")))?
        .bytes()
        .await
        .map_err(|e| DataFusionError::Execution(format!("read `{uri}`: {e}")))?;
    Ok(data.to_vec())
}

/// CSV read options matching the writer's whole-file schema inference (DuckDB
/// `sample_size = -1`). DataFusion samples only the first ~1000 rows by default,
/// which mis-types a column whose early values look numeric but later turn
/// stringy (or vice-versa) — read every record so the inferred Arrow types (and
/// thus the manifest's `data_type`s) match the writer. Trade-off: inference
/// reads the file once before execution reads it again; acceptable for parity,
/// revisit if it bites large remote sources.
fn csv_options<'a>() -> CsvReadOptions<'a> {
    CsvReadOptions::new().schema_infer_max_records(usize::MAX)
}

// ── Host input seam: provider (RDF) sources ─────────────────────────────────
//
// Object-store formats need no host help beyond the registered `ObjectStore`.
// `Provider` formats (RDF) do: the decode is fossil's (pure, WASM-clean —
// `rdf::rdf_to_batch`), but the *bytes* are the host's (fs natively, `fetch` in
// the browser). So fossil enumerates what to read + how to pivot it
// ([`provider_bindings`], derived from the MIR — the executor's single source of
// truth), the host reads the bytes, and [`register_rdf`] puts the decoded
// relation in the ctx for [`execute_graph`] to scan.

/// A provider-backed source (`io.rdf`, …) the host must materialise into the
/// [`SessionContext`] before [`execute_graph`]: register a table named `binding`
/// holding the bytes at `uri`, decoded by selecting the subjects of `type_iri`
/// and pivoting `columns` (predicate → relation column).
///
/// Derived from the **MIR**, so it carries exactly the columns the mapping reads
/// (each prop's value `ColRef` → column name, its predicate IRI → the pivot
/// predicate) — no ShEx re-parse here, and unused shape predicates are not
/// materialised (the relation stays minimal). The ShEx descriptor's role is
/// type inference at compile time, not the executor's pivot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBinding {
    pub binding: String,
    pub uri: String,
    pub type_iri: String,
    pub columns: Vec<rdf::RdfColumn>,
}

/// Enumerate every provider-backed source in `file` — one per mapping whose
/// source lowers to [`SourceFormat::Provider`]. The host reads each `uri`'s
/// bytes and calls [`register_rdf`] to put the pivoted relation in the ctx
/// before running [`execute_graph`].
#[must_use]
pub fn provider_bindings(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> Vec<ProviderBinding> {
    let program_dir = fossil_locator::program_dir(file.path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let mappings = def_map(db, file).mappings(db).clone();
    let schema = descriptor.to_graph_schema(&def_map(db, file).renames(db));
    let mut out = Vec::new();
    for mapping in mappings {
        let mir = lower_to_mir_pg(db, mapping);
        let ops = apply_output_shape(mir.ops(db), &schema);

        let Some((uri, binding)) = ops.iter().find_map(|o| match o {
            Op::Source {
                uri,
                format: SourceFormat::Provider { .. },
                binding,
                ..
            } => Some((anchor.locator(uri), binding.to_string())),
            _ => None,
        }) else {
            continue; // object-store source — no host bytes seam
        };

        let Some(type_iri) = ops.iter().find_map(|o| match o {
            Op::EmitVertex { rdf_type, .. } => Some(
                rdf_type
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            ),
            _ => None,
        }) else {
            continue;
        };

        out.push(ProviderBinding {
            binding,
            uri,
            type_iri,
            columns: rdf_columns(&ops),
        });
    }
    out
}

/// A source the host must fetch before running the executor: its program URI
/// (`io.csv("…")`) and format. The browser host enumerates these (via the wasm
/// `sources()` wrapper) to know which signed URLs to request and how to stage the
/// bytes — object-store formats into the `SessionContext`, `Provider` (RDF) via
/// [`register_rdf`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub uri: String,
    pub format: SourceFormat,
}

/// Enumerate every distinct source the program reads (one [`Op::Source`] per
/// mapping, deduplicated by URI). Pure — no IO — so a host can call it to plan
/// its fetches before [`execute_graph`]. Mirrors what [`execute_graph`] resolves
/// internally, so the list is exactly the sources the run will read.
#[must_use]
pub fn program_sources(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> Vec<SourceRef> {
    let program_dir = fossil_locator::program_dir(file.path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let mappings = def_map(db, file).mappings(db).clone();
    let schema = descriptor.to_graph_schema(&def_map(db, file).renames(db));
    let mut out: Vec<SourceRef> = Vec::new();
    for mapping in mappings {
        let mir = lower_to_mir_pg(db, mapping);
        let ops = apply_output_shape(mir.ops(db), &schema);
        for (uri, format, _binding) in sources_of(&ops, anchor) {
            if !out.iter().any(|s| s.uri == uri) {
                out.push(SourceRef { uri, format });
            }
        }
    }
    out
}

/// The RDF pivot columns a mapping reads, derived from the descriptor-refined
/// ops: each vertex prop AND each edge endpoint whose source value is a `ColRef`
/// contributes `{ name: <that column>, predicate: <its IRI>, multi: <not single
/// valued> }`. The column name is the value `ColRef` (what the projection / edge
/// join reads), NOT the predicate's local name — so `name = people.fullName`
/// pivots `foaf:name` into a `fullName` column. A multi-valued constraint pivots
/// into a `List` (a multi-valued vertex prop, or — after UNNEST — an edge per
/// object); the vertex `subject` id needs no pivot column (`rdf_to_batch` always
/// emits it).
fn rdf_columns(ops: &[Op<'_>]) -> Vec<rdf::RdfColumn> {
    let mut cols = Vec::new();
    let mut push = |value: &Expr<'_>, rdf_uri: Option<&str>, single_valued: bool| {
        if let (Expr::ColRef { column, .. }, Some(predicate)) = (value, rdf_uri) {
            cols.push(rdf::RdfColumn {
                name: column.to_string(),
                predicate: predicate.to_string(),
                multi: !single_valued,
            });
        }
    };
    for op in ops {
        match op {
            Op::EmitVertex { props, .. } => {
                for p in props {
                    push(&p.value, p.rdf_uri.as_deref(), p.single_valued);
                }
            }
            Op::EmitEdge {
                dst_id,
                rdf_uri,
                single_valued,
                ..
            } => push(dst_id, rdf_uri.as_deref(), *single_valued),
            _ => {}
        }
    }
    cols
}

/// Decode `turtle` for `binding`'s shape and register the result as a `MemTable`
/// named `binding.binding` in `ctx`, so [`execute_graph`]'s `Provider` arm scans
/// it. Target-agnostic — the host supplies the bytes, so native and browser
/// share this exact path.
///
/// # Errors
/// Turtle parse / Arrow construction errors, or table registration failure.
pub fn register_rdf(
    ctx: &SessionContext,
    binding: &ProviderBinding,
    turtle: &str,
) -> datafusion::error::Result<()> {
    let batch = rdf::rdf_to_batch(turtle, &binding.type_iri, &binding.columns)?;
    register_batches(ctx, &provider_table_name(&binding.binding), &[batch])
}

/// The `SessionContext` table name a provider (RDF) source is registered under.
/// Namespaced away from the vertex tables [`finalize_vertex`] registers
/// (`node.label`): for an RDF shape the source binding *is* the shape local name
/// (`{ KB } := io.rdf …` + `KB : KB from KB`), so an un-namespaced source
/// table `KB` would collide with the vertex table `KB` (DataFusion folds
/// identifiers to lowercase, so even case wouldn't save it).
fn provider_table_name(binding: &str) -> String {
    format!("__rdf_src_{binding}")
}

/// Native host convenience: read every provider source's bytes from the local
/// filesystem and register the decoded relations in `ctx`. The browser host
/// reimplements this loop with `fetch` + [`register_rdf`] (same decode, async
/// byte source), which is why the byte read — and only the byte read — is gated
/// off wasm here.
///
/// # Errors
/// Filesystem read errors or decode/registration failures.
#[cfg(not(target_arch = "wasm32"))]
pub fn register_provider_sources(
    ctx: &SessionContext,
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> datafusion::error::Result<()> {
    for binding in provider_bindings(db, file, descriptor, connections) {
        let turtle = std::fs::read_to_string(&binding.uri).map_err(|e| {
            DataFusionError::Execution(format!("read RDF source `{}`: {e}", binding.uri))
        })?;
        register_rdf(ctx, &binding, &turtle)?;
    }
    Ok(())
}

/// A `SessionContext` under a declared memory budget, or the unbounded default.
///
/// Ten million vertices peak at 15.7 GiB in the executor while the graph it
/// produces is 1.64 GiB of Arrow, and nothing in between is retained — so the
/// difference is operator memory that DataFusion is never told to bound. A pool
/// bounds it and spills instead, and [`TrackConsumersPool`] names the operators
/// that asked for it when the budget is too small to hold.
///
/// The budget is an input of the run — `memory_bytes` comes from the command
/// that started it (`fossil run --memory-gib`), not from the environment the
/// process happens to be carrying. `None` is today's behaviour, unbounded.
#[cfg(not(target_arch = "wasm32"))]
fn bounded_context(memory_bytes: Option<u64>) -> datafusion::error::Result<SessionContext> {
    let Some(bytes) = memory_bytes else {
        return Ok(SessionContext::new());
    };
    let pool = TrackConsumersPool::new(
        FairSpillPool::new(usize::try_from(bytes).unwrap_or(usize::MAX)),
        std::num::NonZeroUsize::new(5).expect("5 is not zero"),
    );
    let runtime = RuntimeEnvBuilder::new()
        .with_memory_pool(Arc::new(pool))
        .build_arc()?;
    // A budget only means something if the operators can honour it. The edge
    // phase joins every edge's endpoints against the vertex table, and a hash
    // join's build side reports `can spill: false` — one reservation per
    // partition, none of which will give anything back. A sort-merge join
    // spills; that it is the slower plan on a small graph is not the trade being
    // made here.
    let config = datafusion::prelude::SessionConfig::new()
        .set_bool("datafusion.optimizer.prefer_hash_join", false);
    Ok(SessionContext::new_with_config_rt(config, runtime))
}

/// Native one-call orchestration the host (CLI/engine) drives: register every
/// provider (RDF) source from host-read bytes, execute the whole program on
/// DataFusion, and write the GraphAr tree under `dest_dir`. Returns the
/// [`GraphArData`] so the caller can build a [`RunReport`] and run any post-pass
/// (e.g. the layout enrichment).
///
/// `connections` is the name→base-URL ref-map: `@conn/path` source aliases
/// resolve through it for BOTH object-store reads (csv/json/parquet → the
/// resolved URL feeds `read_csv`) and provider (RDF)
/// bindings. `read_uri` is the host's byte seam for RDF only: given a (resolved)
/// source URI, return its text — the host owns credentials + transport (fs /
/// cloud). Object-store formats are NOT read through it; they stream via the
/// ctx's `ObjectStore` (the local filesystem by default).
///
/// `memory_bytes` is the run's declared memory budget ([`bounded_context`]):
/// under one, the executor spills instead of growing, and a corpus larger than
/// the machine is a slower run rather than an OOM. `None` leaves the pool
/// unbounded.
///
/// Blocks the async executor on a private current-thread runtime — the host
/// stays synchronous. The browser path drives [`execute_graph`] directly from
/// JS, so this native convenience never reaches the wasm build.
///
/// # Errors
/// Host read errors (surfaced from `read_uri`), decode/registration failures,
/// DataFusion execution errors, or Parquet/manifest write failures.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_to_dir(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    dest_dir: &std::path::Path,
    connections: &HashMap<String, String>,
    read_uri: impl Fn(&str) -> Result<String, String>,
    memory_bytes: Option<u64>,
    policy: Option<&fossil_policy::PrivacyPolicy>,
) -> datafusion::error::Result<GraphArData> {
    let mut probe = Probe::new("run_to_dir");
    let ctx = bounded_context(memory_bytes)?;
    for binding in provider_bindings(db, file, descriptor, connections) {
        let bytes = read_uri(&binding.uri).map_err(DataFusionError::Execution)?;
        register_rdf(&ctx, &binding, &bytes)?;
    }
    probe.mark("register providers");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| DataFusionError::Execution(format!("build tokio runtime: {e}")))?;
    let mut graph = runtime.block_on(execute_graph(&ctx, db, file, descriptor, connections))?;
    // What the boundary type itself holds, against what the process holds. The
    // gap between them is the executor's transient — sort buffers, and pages the
    // allocator has not returned — and the two want opposite fixes, so the mark
    // reports both rather than leaving the difference to be assumed.
    probe.mark(&format!(
        "execute_graph — {:.2}G in Arrow",
        graph.arrow_gib()
    ));

    // The executor's context still holds a `MemTable` per vertex type, and the
    // edge phase joined against them. Nothing below reads them, and the encode
    // that follows is the other half of the corpus resident at once — so this
    // is the last moment they can be released rather than added to.
    drop(ctx);
    probe.mark("drop session context");

    // The bound, measured HERE — after the corpus is a value and before any of
    // it is a file. There is no read path to put a control on, so the only
    // control there is is not writing the bytes; a refusal on this line leaves
    // nothing on disk to leak.
    //
    // No policy leaves `Privacy::Undeclared`, which the manifest writes down as
    // such rather than omitting.
    if let Some(policy) = policy {
        // Derive FIRST, over these same batches, and hand the verifier nothing:
        // `derived` carries only what the manifest prints. The bound below is
        // measured by a DataFusion aggregate that shares no code with the
        // anonymiser and is told nothing about it, so the two agree by
        // measurement or not at all. See `generalize`'s module docs.
        let derived = generalize::apply(policy, &mut graph)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        probe.mark("derive generalisation");
        graph.privacy = runtime
            .block_on(privacy::verify(policy, &graph))
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        generalize::record(&derived, &mut graph.privacy);
        probe.mark("verify privacy bound");
    }

    graph
        .write_to_dir(dest_dir)
        .map_err(|e| DataFusionError::Execution(format!("write GraphAr: {e}")))?;
    probe.mark("write_to_dir");
    probe.finish();
    Ok(graph)
}

/// Render a MIR [`Expr`] to a DataFusion logical [`DfExpr`]. Total over the
/// MIR expression space since F2 §2 — there is no `unimplemented!()` left to
/// reach, which is what makes a property that type-checks a property that runs.
pub(crate) fn render(e: &Expr<'_>) -> DfExpr {
    use fossil_hir::{BinOp, UnOp};
    match e {
        Expr::LitString(s) => lit(s.to_string()),
        // `new_unqualified` / `TableReference::bare` (NOT `col()`): a bare
        // `col("hasProject")` folds the identifier to lowercase, but the source
        // columns (CSV headers, the RDF pivot's predicate-named columns)
        // preserve case — reference them verbatim, and the relation too.
        //
        // The source is the binding the author wrote (`User.email`), and
        // `plan_relation` qualifies each source relation under exactly that
        // name — which is what makes `Node.label` and `Other.label` two columns
        // after a self-join. An empty source is the retired bare `.column`; it
        // resolves against whichever relation carries the name.
        Expr::ColRef { source, column } => DfExpr::Column(if source.is_empty() {
            Column::new_unqualified(column.as_str())
        } else {
            Column::new(
                Some(datafusion::common::TableReference::bare(source.as_str())),
                column.as_str(),
            )
        }),
        Expr::Concat(a, b) => binary_expr(render(a), Operator::StringConcat, render(b)),
        Expr::Assert { inner, .. } => render(inner),
        Expr::Call { func, args, .. } => render_call(func.as_str(), args),
        Expr::LitInt(v) => lit(*v),
        Expr::LitFloat(v) => lit(v.get()),
        Expr::LitBool(b) => lit(*b),
        // The one comparison whose SQL is not its spelling — `x != NULL` is
        // NULL and not true, so this is where the surface's `x != null` gets
        // the operator it means.
        Expr::IsNull { operand, negated } => {
            let inner = render(operand);
            if *negated {
                DfExpr::IsNotNull(Box::new(inner))
            } else {
                DfExpr::IsNull(Box::new(inner))
            }
        }
        // `/` is the one operator whose SQL is not its spelling. fossil types
        // `a / b` as Float (see `synth_binop`), and DataFusion's `Divide` on two
        // `Int64`s is INTEGER division — `7 / 2` would be `3` while DuckDB, the
        // other engine this language runs on, gives `3.5` for the same program.
        // Casting the left operand makes the engine compute what the type says
        // rather than making the type describe whatever the engine did.
        Expr::BinOp {
            op: BinOp::Div,
            lhs,
            rhs,
            ..
        } => binary_expr(
            datafusion::logical_expr::cast(render(lhs), DataType::Float64),
            Operator::Divide,
            render(rhs),
        ),
        Expr::BinOp { op, lhs, rhs, .. } => binary_expr(render(lhs), df_operator(*op), render(rhs)),
        // `-x` and `not x`, as themselves. Rendering `-x` as `0 - x` would make
        // `-0.0` come out `+0.0` — measured, and the reason
        // `fossil_hir::HirExpr::UnaryOp` is a node at all.
        Expr::UnaryOp { op, operand, .. } => match op {
            UnOp::Neg => DfExpr::Negative(Box::new(render(operand))),
            UnOp::Not => DfExpr::Not(Box::new(render(operand))),
        },
        // A two-armed CASE. `otherwise` is always present — fossil has no
        // one-armed conditional, so no row can fall through to NULL.
        Expr::Ternary {
            cond,
            then,
            otherwise,
            ..
        } => datafusion::prelude::when(render(cond), render(then))
            .otherwise(render(otherwise))
            .unwrap_or_else(|e| {
                unsupported_call("? :", &format!("could not be built as a CASE: {e}"))
            }),
    }
}

/// The `DataFusion` operator for a fossil one. The two sets coincide exactly —
/// this is a spelling, not a translation, and it is total, so a new operator in
/// the language stops compiling here rather than reaching a plan wrong.
const fn df_operator(op: fossil_hir::BinOp) -> Operator {
    use fossil_hir::BinOp;
    match op {
        BinOp::Eq => Operator::Eq,
        BinOp::Ne => Operator::NotEq,
        BinOp::Lt => Operator::Lt,
        BinOp::Le => Operator::LtEq,
        BinOp::Gt => Operator::Gt,
        BinOp::Ge => Operator::GtEq,
        BinOp::And => Operator::And,
        BinOp::Or => Operator::Or,
        BinOp::Add => Operator::Plus,
        BinOp::Sub => Operator::Minus,
        BinOp::Mul => Operator::Multiply,
        // Reached only through the general `BinOp` arm above, which `Div` never
        // takes — it has its own arm because it needs a cast. Kept total so a
        // new operator stops compiling here rather than reaching a plan wrong.
        BinOp::Div => Operator::Divide,
        BinOp::Rem => Operator::Modulo,
    }
}

/// Render a catalogued call. The name is fossil's (`str.slug`); what it
/// becomes on this engine comes from the catalog entry, via [`crate::stdlib`].
///
/// A call that reaches here has type-checked, so the name IS catalogued. What
/// it can still hit is a function this engine has no implementation for — an
/// aggregate in a scalar position — and that is an error carried in the plan,
/// not a panic and not a dropped column.
fn render_call(func: &str, args: &[Expr<'_>]) -> DfExpr {
    use fossil_hir::stdlib::LoweringKind;

    let rendered: Vec<DfExpr> = args.iter().map(render).collect();
    let Some(entry) = fossil_hir::stdlib::stdlib().lookup(func) else {
        return unsupported_call(func, "is not in the stdlib catalog");
    };

    match &entry.lowering {
        LoweringKind::Expr(template) => match render_expr_template(template.as_str(), &rendered) {
            Ok(e) => e,
            Err(why) => unsupported_call(func, &why),
        },
        LoweringKind::Op(_) => unsupported_call(func, "is a plan operator, not a value"),
    }
}

/// Turn a catalogue SQL template into a `DataFusion` expression.
///
/// # Why the template is INTERPRETED rather than pattern-matched
///
/// The SQL shape is a field of the catalogue row, not a `match` arm here, so
/// adding `str.slugify` over `regexp_replace` changes no Rust.
///
/// # Why a hand-written reader and not a SQL parser
///
/// Two SQL parsers were available and both were refused, for reasons that are
/// this crate's and not preferences. `DataFusion`'s own
/// `SessionContext::parse_sql_expr` is behind its `sql` feature, which
/// `Cargo.toml` turns OFF on purpose — this crate compiles to `wasm32` and the
/// SQL frontend is bundle weight for a backend that builds plans
/// programmatically. `sqlparser` is declared in the workspace but is in nobody's
/// dependency tree, so it would be a new dependency for one call site.
///
/// What is read here is not SQL. It is the closed expression language the
/// CATALOGUE writes, which is nine shapes wide and enumerated in
/// [`TEMPLATE_GRAMMAR`]. A template outside it is an `Err` that names itself, so
/// the day the catalogue wants a tenth shape, the failure says so.
///
/// # The dialect seam, which is real and is NOT hidden
///
/// The templates are `DuckDB`'s, because `DuckDB` is what they were measured
/// against. [`crate::stdlib::datafusion_name`] has always been the one place the
/// two vocabularies are reconciled, and it is applied here per function name
/// rather than to a single builtin.
///
/// What cannot be reconciled stays a NAMED gap — see [`crate::stdlib`]. An
/// `Err` becomes an `unsupported_call`, never a dropped column.
///
/// # Errors
///
/// Returns the reason the template could not be rendered, phrased to be read
/// after "`<func>` ".
pub(crate) fn render_expr_template(template: &str, args: &[DfExpr]) -> Result<DfExpr, String> {
    let mut r = TemplateReader {
        chars: template.chars().collect(),
        pos: 0,
        args,
    };
    let e = r.expr()?;
    r.skip_ws();
    if r.pos < r.chars.len() {
        return Err(format!(
            "has a template with trailing text at offset {} (`{}`)",
            r.pos,
            r.chars[r.pos..].iter().collect::<String>()
        ));
    }
    Ok(e)
}

/// A reader over one catalogue template. See [`TEMPLATE_GRAMMAR`] for the grammar.
struct TemplateReader<'a> {
    chars: Vec<char>,
    pos: usize,
    args: &'a [DfExpr],
}

/// The grammar of a catalogue template — the whole of it, and the reason a SQL
/// parser is not needed:
///
/// ```text
/// Expr    := Or
/// Or      := Concat ('OR' Concat)*
/// Concat  := Primary ('||' Primary)*         -- also `IS NULL` / `IS NOT NULL`
/// Primary := '%' DIGITS                      -- an argument hole
///          | "'" ... "'"                     -- a string literal ('' escapes)
///          | DIGITS                          -- an integer literal
///          | 'CAST' '(' Expr 'AS' TYPE ')'
///          | 'CASE' 'WHEN' Expr 'THEN' Expr 'ELSE' Expr 'END'
///          | IDENT '(' Expr (',' Expr)* ')'  -- a function call
///          | '(' Expr ')'
/// ```
///
/// Nine shapes. Anything else is an error that names the offset.
#[allow(dead_code)] // documentation anchor: the grammar above is the datum.
const TEMPLATE_GRAMMAR: () = ();

impl TemplateReader<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    /// Does the (case-insensitive) word `kw` stand here, as a whole word?
    fn peek_kw(&mut self, kw: &str) -> bool {
        self.skip_ws();
        let end = self.pos + kw.len();
        if end > self.chars.len() {
            return false;
        }
        let got: String = self.chars[self.pos..end].iter().collect();
        if !got.eq_ignore_ascii_case(kw) {
            return false;
        }
        // A keyword must not be the prefix of a longer identifier.
        !self
            .chars
            .get(end)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.peek_kw(kw) {
            self.pos += kw.len();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), String> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(format!(
                "has a template missing `{kw}` at offset {}",
                self.pos
            ))
        }
    }

    fn eat_char(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.chars.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<DfExpr, String> {
        let mut lhs = self.concat()?;
        while self.eat_kw("OR") {
            let rhs = self.concat()?;
            lhs = binary_expr(lhs, Operator::Or, rhs);
        }
        Ok(lhs)
    }

    fn concat(&mut self) -> Result<DfExpr, String> {
        let mut lhs = self.primary()?;
        loop {
            self.skip_ws();
            if self.chars.get(self.pos) == Some(&'|') && self.chars.get(self.pos + 1) == Some(&'|')
            {
                self.pos += 2;
                let rhs = self.primary()?;
                lhs = binary_expr(lhs, Operator::StringConcat, rhs);
                continue;
            }
            if self.eat_kw("IS") {
                if self.eat_kw("NOT") {
                    self.expect_kw("NULL")?;
                    lhs = lhs.is_not_null();
                } else {
                    self.expect_kw("NULL")?;
                    lhs = lhs.is_null();
                }
                continue;
            }
            break;
        }
        Ok(lhs)
    }

    fn primary(&mut self) -> Result<DfExpr, String> {
        self.skip_ws();
        let Some(&c) = self.chars.get(self.pos) else {
            return Err("has a template that ends where a value was due".to_string());
        };

        // `%N` — an argument hole. Out of range is a catalogue bug and is named
        // as one rather than silently dropping the term.
        if c == '%' {
            self.pos += 1;
            let start = self.pos;
            while self.chars.get(self.pos).is_some_and(char::is_ascii_digit) {
                self.pos += 1;
            }
            let n: usize = self.chars[start..self.pos]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| format!("has a template with a malformed hole at offset {start}"))?;
            return self.args.get(n).cloned().ok_or_else(|| {
                format!(
                    "has a template naming `%{n}`, and it was given {} argument(s)",
                    self.args.len()
                )
            });
        }

        // `'...'`, with `''` for an embedded quote.
        if c == '\'' {
            self.pos += 1;
            let mut s = String::new();
            loop {
                match self.chars.get(self.pos) {
                    None => return Err("has a template with an unterminated string".to_string()),
                    Some('\'') if self.chars.get(self.pos + 1) == Some(&'\'') => {
                        s.push('\'');
                        self.pos += 2;
                    }
                    Some('\'') => {
                        self.pos += 1;
                        break;
                    }
                    Some(&ch) => {
                        s.push(ch);
                        self.pos += 1;
                    }
                }
            }
            return Ok(lit(s));
        }

        if c.is_ascii_digit() {
            let start = self.pos;
            while self.chars.get(self.pos).is_some_and(char::is_ascii_digit) {
                self.pos += 1;
            }
            let n: i64 = self.chars[start..self.pos]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| format!("has a template with a malformed number at offset {start}"))?;
            return Ok(lit(n));
        }

        if c == '(' {
            self.pos += 1;
            let e = self.expr()?;
            if !self.eat_char(')') {
                return Err(format!("has a template missing `)` at offset {}", self.pos));
            }
            return Ok(e);
        }

        if self.eat_kw("CAST") {
            if !self.eat_char('(') {
                return Err("has a `CAST` with no `(`".to_string());
            }
            let inner = self.expr()?;
            self.expect_kw("AS")?;
            self.skip_ws();
            let start = self.pos;
            let mut depth = 0usize;
            while let Some(&ch) = self.chars.get(self.pos) {
                match ch {
                    '(' => depth += 1,
                    ')' if depth == 0 => break,
                    ')' => depth -= 1,
                    _ => {}
                }
                self.pos += 1;
            }
            let sql_type: String = self.chars[start..self.pos].iter().collect();
            if !self.eat_char(')') {
                return Err("has a `CAST` with no closing `)`".to_string());
            }
            let dt = cast_target(sql_type.trim()).ok_or_else(|| {
                format!(
                    "casts to `{}`, which this engine does not map",
                    sql_type.trim()
                )
            })?;
            return Ok(DfExpr::Cast(datafusion::logical_expr::Cast::new(
                Box::new(inner),
                dt,
            )));
        }

        if self.eat_kw("CASE") {
            self.expect_kw("WHEN")?;
            let when = self.expr()?;
            self.expect_kw("THEN")?;
            let then = self.expr()?;
            self.expect_kw("ELSE")?;
            let otherwise = self.expr()?;
            self.expect_kw("END")?;
            return datafusion::logical_expr::when(when, then)
                .otherwise(otherwise)
                .map_err(|e| format!("has a `CASE` this engine rejected ({e})"));
        }

        // A function call. The name is the only thing the dialect seam touches.
        if c.is_alphabetic() || c == '_' {
            let start = self.pos;
            while self
                .chars
                .get(self.pos)
                .is_some_and(|ch| ch.is_alphanumeric() || *ch == '_')
            {
                self.pos += 1;
            }
            let name: String = self.chars[start..self.pos].iter().collect();
            if !self.eat_char('(') {
                return Err(format!("has a bare name `{name}` where a value was due"));
            }
            let mut args = Vec::new();
            if !self.eat_char(')') {
                loop {
                    args.push(self.expr()?);
                    if self.eat_char(',') {
                        continue;
                    }
                    if self.eat_char(')') {
                        break;
                    }
                    return Err(format!(
                        "has a call to `{name}` with a malformed argument list"
                    ));
                }
            }
            let df_name = crate::stdlib::datafusion_name(&name)
                .ok_or_else(|| format!("calls `{name}`, which has no DataFusion equivalent"))?;
            // BOTH function packages, and the second one is why `str.split`
            // renders. `all_default_functions()` is the SCALAR package alone;
            // `string_to_array` — what `string_split` reconciles to — lives in
            // the nested package, which the `nested_expressions` feature brings
            // in. Enabling the feature and not looking here left the row exactly
            // as unrenderable as before, under a Cargo.toml that said otherwise.
            let udf = datafusion::functions::all_default_functions()
                .into_iter()
                .chain(datafusion::functions_nested::all_default_nested_functions())
                .find(|u| u.name() == df_name || u.aliases().iter().any(|a| a == df_name))
                .ok_or_else(|| {
                    format!("calls `{name}` → `{df_name}`, which is not a DataFusion function")
                })?;
            return Ok(DfExpr::ScalarFunction(
                datafusion::logical_expr::expr::ScalarFunction::new_udf(udf, args),
            ));
        }

        Err(format!(
            "has a template with an unexpected `{c}` at offset {}",
            self.pos
        ))
    }
}

/// The Arrow type a catalogued `CAST(x AS <sql_type>)` targets.
fn cast_target(sql_type: &str) -> Option<datafusion::arrow::datatypes::DataType> {
    Some(match sql_type {
        "BIGINT" => datafusion::arrow::datatypes::DataType::Int64,
        "DOUBLE" => datafusion::arrow::datatypes::DataType::Float64,
        "BOOLEAN" => datafusion::arrow::datatypes::DataType::Boolean,
        "DATE" => datafusion::arrow::datatypes::DataType::Date32,
        s if s.starts_with("DECIMAL") => datafusion::arrow::datatypes::DataType::Float64,
        _ => return None,
    })
}

/// A call the engine cannot make, rendered as an expression that fails the plan
/// with fossil's own words. Not a panic: one unrunnable property must not take
/// the whole run down before the other diagnostics are reported.
fn unsupported_call(func: &str, why: &str) -> DfExpr {
    DfExpr::Literal(datafusion::scalar::ScalarValue::Utf8(None), None)
        .alias(format!("__fossil_unsupported__{func}__{why}"))
}

// ── The manifest ───────────────────────────────────────────────────────────
//
// One description of the dataset, in the shape the fossil-graph reader
// round-trips (`prefix = vertex/<Type>/`). Type-name casing is preserved
// throughout. There used to be a second one — a `RunStatus` the CLI printed;
// see `report.rs`.

/// The `<src>_<label>_<dst>` adjacency directory name (writer convention).
fn edge_dir(src: &str, label: &str, dst: &str) -> String {
    format!("{src}_{label}_{dst}")
}

/// Total rows across a set of batches — the count the manifest declares.
fn count_rows(batches: &[RecordBatch]) -> u64 {
    batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64
}

impl GraphArData {
    /// What this value costs in Arrow buffers, in `GiB` — every vertex batch plus
    /// both orientations of every edge table. Cheap (a walk of the batch list, no
    /// data touched) and the number the peak-memory work is about: the whole corpus
    /// is resident in one value, and it is resident because the TYPE of the boundary
    /// between executing and writing is a whole graph — a streaming boundary would
    /// not hold it, and no amount of tuning inside either half can give it back.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn arrow_gib(&self) -> f64 {
        let batches = self.vertices.iter().flat_map(|v| &v.batches).chain(
            self.edges
                .iter()
                .flat_map(|e| e.by_source.iter().chain(&e.by_target)),
        );
        batches
            .map(RecordBatch::get_array_memory_size)
            .sum::<usize>() as f64
            / (1024.0 * 1024.0 * 1024.0)
    }

    /// The corpus's own description — the GraphAr
    /// *materializer*: the `graph.graph.yml` index, one [`VertexInfo`] per node
    /// type and one [`EdgeInfo`] per edge type, in the order the index names
    /// them. Reuses the WASM-clean `fossil_sinks::manifest` structs.
    ///
    /// **This is the single description of the dataset.** It is serialised to
    /// YAML by [`Self::manifests`] and printed as JSON by `fossil run
    /// --output-json` through [`RunReport`] — the same values, so stdout and the
    /// disk cannot disagree without this function disagreeing with itself.
    ///
    /// `GraphInfo::vertices` / `GraphInfo::edges` are the rel-paths of the two
    /// returned lists, positionally.
    #[must_use]
    pub fn manifest(&self) -> (GraphInfo, Vec<VertexInfo>, Vec<EdgeInfo>) {
        let vertex_paths: Vec<String> = self
            .schema
            .nodes
            .iter()
            .map(|n| format!("vertex/{}.vertex.yml", n.label))
            .collect();
        let edge_paths: Vec<String> = self
            .schema
            .edges
            .iter()
            .map(|e| {
                let dir = edge_dir(&e.source, &e.label, &e.destination);
                format!("edge/{dir}/{dir}.edge.yml")
            })
            .collect();
        // The container, declared. A reader cannot work it out — working it out
        // means listing a directory, and there is no listing over HTTP — so it
        // is written down here, once, for every payload set of the corpus. What
        // fossil emits is the row-group one: `fossil-layout` cuts each set into
        // one Parquet whose row groups ARE its tiles.
        // The bound travels with the manifest because it is a property of the
        // release the manifest describes. `with_privacy` is the only way to set
        // it and only the verifier calls it, so a corpus cannot declare a bound
        // that was not measured over these rows.
        let graph = GraphInfo::new("graph", "", Container::RowGroups, vertex_paths, edge_paths)
            .with_privacy(self.privacy.clone());

        // The counts come off the materialised batches and not off the schema,
        // because the schema knows what types there are and only the data knows
        // how many rows each one got. A declared type that materialised nothing
        // is `0`, which is the honest answer and the one that says «no tiles»
        // rather than «one empty tile».
        let vertices = self
            .schema
            .nodes
            .iter()
            .map(|node| {
                let rows = self
                    .vertices
                    .iter()
                    .find(|v| v.label == node.label)
                    .map_or(0, |v| count_rows(&v.batches));
                vertex_info(node, rows)
            })
            .collect();
        let edges = self
            .schema
            .edges
            .iter()
            .map(|edge| {
                // One orientation, because the two are one relation stored
                // twice — and `by_source` because that is the one a reader sums
                // the CSR tiles against.
                let rows = self
                    .edge_table(edge)
                    .map_or(0, |e| count_rows(&e.by_source));
                edge_info(edge, rows)
            })
            .collect();
        (graph, vertices, edges)
    }

    /// The materialised [`EdgeTable`] for a schema edge, matched on the
    /// `(src, label, dst)` triple that identifies it. `None` for a declared edge
    /// type nothing wrote.
    pub(crate) fn edge_table(&self, edge: &GraphEdge) -> Option<&EdgeTable> {
        self.edges.iter().find(|e| {
            e.src_type == edge.source && e.label == edge.label && e.dst_type == edge.destination
        })
    }

    /// [`Self::manifest`] as YAML documents and the paths they go to:
    /// the top-level `graph.graph.yml` index, one `vertex/<Type>.vertex.yml` per
    /// node type, and one `edge/<dir>/<dir>.edge.yml` per edge type.
    ///
    /// # Errors
    /// Propagates `serde_yaml_ng` serialization errors (cannot fail for these
    /// plain structs, but the signature is honest).
    pub fn manifests(&self) -> Result<Vec<ManifestFile>, serde_yaml_ng::Error> {
        let (graph, vertices, edges) = self.manifest();
        let mut out = vec![ManifestFile {
            rel_path: "graph.graph.yml".to_string(),
            yaml: graph.to_yaml()?,
        }];
        for (rel_path, info) in graph.vertices.iter().zip(&vertices) {
            out.push(ManifestFile {
                rel_path: rel_path.clone(),
                yaml: info.to_yaml()?,
            });
        }
        for (rel_path, info) in graph.edges.iter().zip(&edges) {
            out.push(ManifestFile {
                rel_path: rel_path.clone(),
                yaml: info.to_yaml()?,
            });
        }
        Ok(out)
    }
}

/// The `VertexInfo` manifest for one node type: `dense_id`, `subject`, the
/// schema's props, then `x`/`y`/`cluster_id`, each with the **real** per-prop
/// `data_type` the schema carries.
///
/// `rows` is the one argument that is not a function of the schema, and it is
/// the whole of what the manifest could not say before: how far the corpus goes.
fn vertex_info(node: &NodeType, rows: u64) -> VertexInfo {
    let mut properties = Vec::with_capacity(node.properties.len() + 5);
    // `is_primary` marks the IDENTITY, and the identity is `subject`.
    //
    // This writer said `dense_id`, and it was the only thing in the tree that
    // did: `apps/corpus/guards/guards.mjs`'s `identity-is-the-subject` says
    // "`dense_id` is an address and cannot also be an identity", the
    // conformance corpus's `vertex/Person.vertex.yml` marks `subject`, and
    // `packages/corpus`'s reader keys on the column name precisely because it
    // could not trust the flag with two writers spelling it two ways. One
    // field, two answers, and the way that resolves in practice is that
    // nothing reads it — which is `RunStatus` again, in one boolean.
    //
    // The address cannot be the identity for the reason `fossil-layout`
    // exists: the pass ranks every vertex by the Morton code of its new
    // position and lets that rank be its `dense_id`, so a `dense_id` held
    // anywhere outside the corpus names a different vertex after the next
    // write. `subject` is what survives that, and since `55f573e` it is
    // load-bearing rather than decorative — `index:` below declares a second
    // copy of the type ordered by it.
    properties.push(Property {
        name: "dense_id".to_string(),
        data_type: "uint32".to_string(),
        is_primary: false,
        is_nullable: Some(false),
    });
    properties.push(Property {
        name: "subject".to_string(),
        data_type: data_type_name(&DataType::Utf8),
        is_primary: true,
        is_nullable: Some(false),
    });
    for p in &node.properties {
        properties.push(Property {
            name: p.name.clone(),
            data_type: graphar_spelling(p.datatype),
            is_primary: false,
            is_nullable: None,
        });
    }
    for layout_col in ["x", "y"] {
        properties.push(Property {
            name: layout_col.to_string(),
            data_type: data_type_name(&DataType::Float32),
            is_primary: false,
            is_nullable: Some(false),
        });
    }
    properties.push(Property {
        name: "cluster_id".to_string(),
        data_type: "uint32".to_string(),
        is_primary: false,
        is_nullable: Some(false),
    });

    let mut info = VertexInfo::new(
        node.label.clone(),
        rows,
        DEFAULT_CHUNK_SIZE,
        format!("vertex/{}/", node.label),
        vec![PropertyGroup {
            file_type: "parquet".to_string(),
            properties,
        }],
    );
    info.iri = node.iri.clone().unwrap_or_default();
    // Every vertex this writer emits carries `subject`, so every one of them gets
    // an identity index and the manifest says so. Declared here, beside the
    // properties that make it possible, rather than after the layout pass that
    // fills it — which is the same order `prefix` is already declared in: the
    // manifest is the plan, and `apps/corpus`'s `index-agrees-with-the-payload`
    // is what goes red if the pass does not deliver it.
    //
    // The same `chunk_size` as the payload because there is no reason yet for
    // them to differ, and a separate field because tile `k` here is the `k`th
    // slice of the SORTED order and has nothing to do with the `dense_id` range
    // tile `k` of the payload holds. One number in two fields would read as an
    // alignment that does not exist.
    info = info.with_index(VertexIndex {
        prefix: "index/".to_string(),
        ordered_by: "subject".to_string(),
        chunk_size: DEFAULT_CHUNK_SIZE,
    });
    // And where the tile-code anchor will be, for the same reason and in the
    // same place: the manifest is the plan, and this pass writes placeholder
    // `x`/`y` it has no codes for. Only the layout pass knows the numbers, and
    // it writes them under this path — `apps/corpus`'s guards are what go red if
    // it does not.
    info = info.with_codes(VertexCodes {
        path: TILE_CODES_FILE.to_string(),
    });
    // And the pyramid, when the type is big enough to have earned one — same
    // place and same reasoning as the two above: the manifest is the plan, and
    // the layout pass is what fills it.
    //
    // **`VertexLevels::planned` is the one place the levels are chosen**, and
    // the pass calls it over the same two numbers this does — `rows` is the
    // count this manifest declares and the count the pass writes — so the
    // manifest cannot name a level nobody wrote. A second copy of the rule on
    // either side is a 404 in a camera the day one of them moves.
    if let Some(levels) = VertexLevels::planned(rows, DEFAULT_CHUNK_SIZE) {
        info = info.with_levels(levels);
    }
    info
}

/// The `EdgeInfo` manifest for one edge type. W0b edges carry no properties
/// (only `src_dense`/`dst_dense`); both CSR + CSC adjacencies are ordered.
///
/// `rows` is one orientation's row count, which is the relation's: the two
/// orientations are the same edges twice.
fn edge_info(edge: &GraphEdge, rows: u64) -> EdgeInfo {
    EdgeInfo {
        src_type: edge.source.clone(),
        edge_type: edge.label.clone(),
        iri: edge.iri.clone().unwrap_or_default(),
        dst_type: edge.destination.clone(),
        edge_count: rows,
        chunk_size: DEFAULT_CHUNK_SIZE,
        src_chunk_size: DEFAULT_CHUNK_SIZE,
        dst_chunk_size: DEFAULT_CHUNK_SIZE,
        directed: true,
        prefix: format!(
            "edge/{}/",
            edge_dir(&edge.source, &edge.label, &edge.destination)
        ),
        // Both orientations, and each says where its tiles are. `aligned_by`
        // gives a reader the arithmetic — which endpoint column addresses this
        // half — and `prefix` gives it the URL, so a hop out of a `dense_id` is
        // derived from the manifest and never agreed between two repositories.
        adj_lists: vec![
            AdjList {
                ordered: true,
                aligned_by: "src".to_string(),
                prefix: "by_source/".to_string(),
                file_type: "parquet".to_string(),
            },
            AdjList {
                ordered: true,
                aligned_by: "dst".to_string(),
                prefix: "by_target/".to_string(),
                file_type: "parquet".to_string(),
            },
        ],
        property_groups: vec![],
        version: GRAPHAR_VERSION.to_string(),
    }
}

/// The GraphAr `data_type` spelling (`string`/`int64`/…) of a schema datatype.
fn graphar_spelling(p: Primitive) -> String {
    primitive_to_graphar(p).to_string()
}
