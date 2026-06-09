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
use datafusion::logical_expr::{binary_expr, Expr as DfExpr, JoinType, Operator};
use datafusion::prelude::{col, lit, CsvReadOptions, SessionContext};
use fossil_base::SourceFile;
use fossil_hir::shapes::{
    default_xsd_string, inner_primitive, primitive_to_graphar, primitive_to_xsd,
};
use fossil_hir::{def_map::def_map, MappingLoc};
use fossil_mir::{lower_to_mir_pg, Expr, Op, VProp};
use fossil_sinks::manifest::{
    data_type_name, AdjList, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
    DEFAULT_CHUNK_SIZE, GRAPHAR_VERSION,
};
use fossil_run_status::{ColumnStatus, EdgeStatus, RunStatus, VertexStatus, WIRE_VERSION};

/// The materialised GraphAr graph for a program: every vertex table and every
/// edge table (CSR + CSC), all in memory as `RecordBatch`es. The WASM/keasy
/// layer turns these into Parquet (`parquet-wasm`) + the manifests and uploads
/// them by signed PUT (design §C4/§E). The manifests + `RunStatus` are a later
/// increment.
#[derive(Debug)]
pub struct GraphArData {
    pub vertices: Vec<VertexTable>,
    pub edges: Vec<EdgeTable>,
}

/// A materialised GraphAr edge type: the `<src>_<edge>_<dst>` adjacency in both
/// orientations — `by_source` (CSR, `ORDER BY src_dense, dst_dense`) and
/// `by_target` (CSC, `ORDER BY dst_dense, src_dense`). Both carry the same two
/// `u32` columns (`src_dense`, `dst_dense`); only the row order differs (writer
/// contract, design §A1).
#[derive(Debug)]
pub struct EdgeTable {
    pub edge_type: String,
    pub src_type: String,
    pub dst_type: String,
    pub rdf_uri: Option<String>,
    pub by_source: Vec<RecordBatch>,
    pub by_target: Vec<RecordBatch>,
}

/// One emitted GraphAr manifest YAML + its dataset-relative path. Keasy serves
/// these verbatim from `GET /discover/manifest` (`manifest_files: Record<path,
/// yaml>`), fed opaquely into `createGraphClient` (design §A2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    pub rel_path: String,
    pub yaml: String,
}

/// Execute a whole program's mappings into the GraphAr graph (design §C4).
///
/// Two phases with a hard barrier between them: **(1)** materialise *every*
/// vertex (assigning dense ids, registering each as a [`MemTable`]); **(2)**
/// resolve *every* edge by joining its endpoint IRIs against the now-registered
/// vertex tables in memory — an edge may point at a vertex owned by another
/// mapping, so all vertices must exist before any edge.
///
/// # Errors
/// Propagates DataFusion read/plan/execute errors.
pub async fn execute_graph<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
) -> datafusion::error::Result<GraphArData> {
    let ctx = SessionContext::new();
    let mappings: Vec<MappingLoc<'db>> = def_map(db, file).mappings(db).clone();

    // Phase 1 (barrier): all vertices, dense ids assigned + tables registered.
    let mut vertices = Vec::with_capacity(mappings.len());
    for &mapping in &mappings {
        vertices.push(execute_vertex(&ctx, db, mapping).await?);
    }

    // Phase 2: edges join the in-memory vertex tables (no Parquet re-read).
    let mut edges = Vec::new();
    for &mapping in &mappings {
        edges.extend(execute_edges(&ctx, db, mapping).await?);
    }

    Ok(GraphArData { vertices, edges })
}

/// A materialised GraphAr vertex table: the `type_name`-named relation and its
/// `RecordBatch`es in the writer-W0b column shape (`dense_id`-prefixed). The
/// batches are also registered in the executor's [`SessionContext`] under
/// `type_name` so the edge phase joins them without re-reading Parquet.
#[derive(Debug)]
pub struct VertexTable {
    pub type_name: String,
    pub rdf_type: Option<String>,
    /// The user property columns (predicate-mapped), in projection order. The
    /// reserved columns (`dense_id`/`subject`/`x`/`y`/`cluster_id`) are NOT here
    /// — they carry no predicate. Feeds the manifest property groups + the
    /// `RunStatus` `ColumnStatus`es.
    pub columns: Vec<VertexColumn>,
    pub batches: Vec<RecordBatch>,
}

/// One user property column's output spec — the GraphAr `data_type` spelling +
/// the RDF predicate/xsd the governance layer (DCAT) reads. Derived from the
/// [`VProp`]'s canonical type via the shared `Primitive → {graphar, xsd}`
/// authority (`fossil_hir::shapes`), identical to what the SQL engine emits.
#[derive(Debug, Clone)]
pub struct VertexColumn {
    pub name: String,
    pub data_type: String,
    pub rdf_uri: Option<String>,
    pub xsd_datatype: Option<String>,
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

    let columns = props.iter().map(|p| vertex_column(db, p)).collect();

    register_batches(ctx, &type_name, &batches)?;
    Ok(VertexTable {
        type_name,
        rdf_type,
        columns,
        batches,
    })
}

/// The output spec of one vertex property — its GraphAr `data_type` + RDF
/// predicate/xsd, derived from the prop's canonical type exactly as the SQL
/// engine derives `VertexProperty` (peel to a [`Primitive`], map to the graphar
/// + xsd vocab; fall back to `string` when the type carries no primitive).
fn vertex_column(db: &dyn fossil_base::Db, prop: &VProp<'_>) -> VertexColumn {
    let prim = inner_primitive(db, prop.ty);
    VertexColumn {
        name: prop.name.to_string(),
        data_type: prim.map_or("string", primitive_to_graphar).to_string(),
        rdf_uri: prop.rdf_uri.as_ref().map(ToString::to_string),
        xsd_datatype: Some(prim.map_or_else(default_xsd_string, primitive_to_xsd)),
    }
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

/// Resolve every [`Op::EmitEdge`] of one mapping into an [`EdgeTable`]. Reads
/// the mapping's source once and joins it against the registered vertex tables.
async fn execute_edges<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> datafusion::error::Result<Vec<EdgeTable>> {
    let mir = lower_to_mir_pg(db, mapping);
    let ops = mir.ops(db);

    let uri = ops
        .iter()
        .find_map(|o| match o {
            Op::Source { uri, .. } => Some(uri.to_string()),
            _ => None,
        })
        .expect("lower_to_mir_pg always emits a Source");

    let mut out = Vec::new();
    for op in ops {
        if let Op::EmitEdge {
            edge_type,
            rdf_uri,
            src_type,
            dst_type,
            src_id,
            dst_id,
            ..
        } = op
        {
            out.push(
                execute_edge(
                    ctx,
                    &uri,
                    edge_type,
                    rdf_uri.as_ref().map(ToString::to_string),
                    src_type,
                    dst_type,
                    src_id,
                    dst_id,
                )
                .await?,
            );
        }
    }
    Ok(out)
}

/// Materialise one edge type. Projects the source rows to `src_iri`/`dst_iri`,
/// joins both against the registered vertex tables to resolve endpoint IRIs to
/// dense ids (inner join — dangling endpoints drop, like the writer), then sorts
/// the `(src_dense, dst_dense)` pairs into CSR (`by_source`) and CSC
/// (`by_target`). Mirrors the writer's edge SQL (writer.rs:469-492).
#[allow(clippy::too_many_arguments)] // the edge spec is a flat tuple, not worth a struct here
async fn execute_edge(
    ctx: &SessionContext,
    uri: &str,
    edge_type: &str,
    rdf_uri: Option<String>,
    src_type: &str,
    dst_type: &str,
    src_id: &Expr<'_>,
    dst_id: &Expr<'_>,
) -> datafusion::error::Result<EdgeTable> {
    let edge_src = ctx.read_csv(uri, CsvReadOptions::new()).await?.select(vec![
        render(src_id).alias("src_iri"),
        render(dst_id).alias("dst_iri"),
    ])?;
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

    let resolved = edge_src
        .join(src_v, JoinType::Inner, &["src_iri"], &["v_src_subject"], None)?
        .join(dst_v, JoinType::Inner, &["dst_iri"], &["v_dst_subject"], None)?
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

    Ok(EdgeTable {
        edge_type: edge_type.to_string(),
        src_type: src_type.to_string(),
        dst_type: dst_type.to_string(),
        rdf_uri,
        by_source,
        by_target,
    })
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

// ── Phase 3: manifests + RunStatus (design §C4 phase 3) ─────────────────────
//
// The paths follow the W0b single-file layout the discovery consumer expects
// (OpenAPI `VertexStatus.file = vertex/<Type>.parquet`) and the manifest shape
// the fossil-graph reader round-trips (`prefix = vertex/<Type>/`). Type-name
// casing is preserved throughout.

/// The `<src>_<label>_<dst>` adjacency directory name (writer convention).
fn edge_dir_name(e: &EdgeTable) -> String {
    format!("{}_{}_{}", e.src_type, e.edge_type, e.dst_type)
}

/// Total rows across a set of batches — the `count` for the wire status.
fn count_rows(batches: &[RecordBatch]) -> i64 {
    batches.iter().map(RecordBatch::num_rows).sum::<usize>() as i64
}

impl GraphArData {
    /// Build the three GraphAr manifest YAMLs (design §C4 phase 3): the
    /// top-level `graph.graph.yml` index, one `vertex/<Type>.vertex.yml` per
    /// vertex type, and one `edge/<dir>/<dir>.edge.yml` per edge type. Reuses
    /// the WASM-clean `fossil_sinks::manifest` structs.
    ///
    /// # Errors
    /// Propagates `serde_yaml_ng` serialization errors (cannot fail for these
    /// plain structs, but the signature is honest).
    pub fn manifests(&self) -> Result<Vec<ManifestFile>, serde_yaml_ng::Error> {
        let vertex_paths: Vec<String> = self
            .vertices
            .iter()
            .map(|v| format!("vertex/{}.vertex.yml", v.type_name))
            .collect();
        let edge_paths: Vec<String> = self
            .edges
            .iter()
            .map(|e| {
                let dir = edge_dir_name(e);
                format!("edge/{dir}/{dir}.edge.yml")
            })
            .collect();

        let graph = GraphInfo::new("graph", "", vertex_paths.clone(), edge_paths.clone());
        let mut out = vec![ManifestFile {
            rel_path: "graph.graph.yml".to_string(),
            yaml: graph.to_yaml()?,
        }];

        for (v, rel_path) in self.vertices.iter().zip(vertex_paths) {
            out.push(ManifestFile {
                rel_path,
                yaml: vertex_info(v).to_yaml()?,
            });
        }
        for (e, rel_path) in self.edges.iter().zip(edge_paths) {
            out.push(ManifestFile {
                rel_path,
                yaml: edge_info(e).to_yaml()?,
            });
        }
        Ok(out)
    }

    /// Build the [`RunStatus`] wire contract keasy consumes for DCAT (design
    /// §A3/§E2). Counts come from the materialised batches (`num_rows()`); the
    /// per-column `rdf_uri`/`xsd_datatype` ride from the vertex columns.
    #[must_use]
    pub fn run_status(&self, dest: &str) -> RunStatus {
        let vertices = self
            .vertices
            .iter()
            .map(|v| VertexStatus {
                vertex_type: v.type_name.clone(),
                rdf_type: v.rdf_type.clone(),
                file: format!("vertex/{}.parquet", v.type_name),
                count: Some(count_rows(&v.batches)),
                columns: v
                    .columns
                    .iter()
                    .map(|c| ColumnStatus {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                        rdf_uri: c.rdf_uri.clone(),
                        xsd_datatype: c.xsd_datatype.clone(),
                    })
                    .collect(),
            })
            .collect();

        let edges = self
            .edges
            .iter()
            .map(|e| {
                let dir = edge_dir_name(e);
                EdgeStatus {
                    edge_type: e.edge_type.clone(),
                    src_type: e.src_type.clone(),
                    dst_type: e.dst_type.clone(),
                    by_source: format!("edge/{dir}/by_source.parquet"),
                    by_target: format!("edge/{dir}/by_target.parquet"),
                    count: Some(count_rows(&e.by_source)),
                }
            })
            .collect();

        RunStatus {
            version: WIRE_VERSION,
            dest: dest.to_string(),
            vertices,
            edges,
        }
    }
}

/// The `VertexInfo` manifest for one materialised vertex table. Mirrors the
/// writer's `build_vertex_manifest` (dense_id/subject/<props>/x/y/cluster_id),
/// but with the **real** per-prop `data_type` the executor knows (the writer
/// defaults caller props to `string`).
fn vertex_info(v: &VertexTable) -> VertexInfo {
    let mut properties = Vec::with_capacity(v.columns.len() + 5);
    properties.push(Property {
        name: "dense_id".to_string(),
        data_type: "uint32".to_string(),
        is_primary: true,
        is_nullable: Some(false),
    });
    properties.push(Property {
        name: "subject".to_string(),
        data_type: data_type_name(&DataType::Utf8),
        is_primary: false,
        is_nullable: Some(false),
    });
    for c in &v.columns {
        properties.push(Property {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
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
        v.type_name.clone(),
        DEFAULT_CHUNK_SIZE,
        format!("vertex/{}/", v.type_name),
        vec![PropertyGroup {
            file_type: "parquet".to_string(),
            properties,
        }],
    );
    info.iri = v.rdf_type.clone().unwrap_or_default();
    info
}

/// The `EdgeInfo` manifest for one materialised edge table. W0b edges carry no
/// properties (only `src_dense`/`dst_dense`); both CSR + CSC adjacencies are
/// ordered. Mirrors the writer's `build_edge_manifest`.
fn edge_info(e: &EdgeTable) -> EdgeInfo {
    EdgeInfo {
        src_type: e.src_type.clone(),
        edge_type: e.edge_type.clone(),
        iri: e.rdf_uri.clone().unwrap_or_default(),
        dst_type: e.dst_type.clone(),
        chunk_size: DEFAULT_CHUNK_SIZE,
        src_chunk_size: DEFAULT_CHUNK_SIZE,
        dst_chunk_size: DEFAULT_CHUNK_SIZE,
        directed: true,
        prefix: format!("edge/{}/", edge_dir_name(e)),
        adj_lists: vec![
            AdjList {
                ordered: true,
                aligned_by: "src".to_string(),
                file_type: "parquet".to_string(),
            },
            AdjList {
                ordered: true,
                aligned_by: "dst".to_string(),
                file_type: "parquet".to_string(),
            },
        ],
        property_groups: vec![],
        version: GRAPHAR_VERSION.to_string(),
    }
}
