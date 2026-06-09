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
//! The crate compiles to `wasm32-unknown-unknown` (the whole executor runs in
//! the browser, design §D). `zstd-sys` is an unavoidable C dep (datafusion 54
//! hardcodes `arrow-ipc/zstd`), so the wasm build needs a wasm-capable clang —
//! see the `datafusion` entry in `Cargo.toml`.
//!
//! Still TODO (next increments): the `Call`/`BinOp` UDFs (`ScalarUDF`),
//! multi-valued (`single_valued = false`) cardinality, and the wasm-bindgen
//! wrapper + parquet-wasm write glue (the JS-facing packaging, design §E).

#[cfg(not(target_arch = "wasm32"))]
pub mod sink;

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, UInt32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::logical_expr::{binary_expr, Expr as DfExpr, JoinType, Operator};
use datafusion::prelude::{col, lit, CsvReadOptions, DataFrame, SessionContext};
use fossil_base::SourceFile;
use fossil_graph_schema::{
    Cardinality, DataType as ScalarType, EdgeType as GraphEdge, GraphSchema, NodeType,
    Property as NodeProp,
};
use fossil_hir::shapes::{inner_primitive, primitive_to_graphar, primitive_to_xsd};
use fossil_hir::{def_map::def_map, MappingLoc, Primitive};
use fossil_mir::{lower_to_mir_pg, Expr, Op, VProp};
use fossil_sinks::manifest::{
    data_type_name, AdjList, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
    DEFAULT_CHUNK_SIZE, GRAPHAR_VERSION,
};
use fossil_run_status::{ColumnStatus, EdgeStatus, RunStatus, VertexStatus, WIRE_VERSION};

/// The materialised graph for a program: the canonical [`GraphSchema`] (the
/// single source of all type/predicate/cardinality metadata) plus the relation
/// data — vertex tables and edge tables (CSR + CSC) as in-memory `RecordBatch`es.
///
/// This is the universal substrate made concrete: **relations + a graph-schema**
/// (`fossil-universal-substrate-architecture.md`). The GraphAr view (manifests +
/// `RunStatus` + Parquet) is materialized *from* this; the data carriers hold no
/// metadata of their own — it all lives in [`schema`](Self::schema).
#[derive(Debug)]
pub struct GraphArData {
    pub schema: GraphSchema,
    pub vertices: Vec<VertexTable>,
    pub edges: Vec<EdgeTable>,
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
/// The caller owns the [`SessionContext`] so it can configure the source layer
/// before execution — the browser host registers an `ObjectStore` per signed
/// source URL and tunes schema inference (design §C2/§E2) — and so the
/// registered vertex tables outlive the call for inspection.
///
/// # Errors
/// Propagates DataFusion read/plan/execute errors.
pub async fn execute_graph<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
) -> datafusion::error::Result<GraphArData> {
    let mappings: Vec<MappingLoc<'db>> = def_map(db, file).mappings(db).clone();

    // Phase 1 (barrier): prepare every mapping's vertex projection, then merge
    // the mappings that emit the SAME type (UNION) before assigning dense ids —
    // a vertex type may be fed by several sources (design §B4). Each type is
    // registered exactly once, so two mappings of one type can't clobber each
    // other's `MemTable`.
    let mut groups: Vec<(String, Vec<PreparedVertex>)> = Vec::new();
    for &mapping in &mappings {
        let prepared = prepare_vertex(ctx, db, mapping).await?;
        match groups.iter_mut().find(|(t, _)| *t == prepared.node.label) {
            Some((_, group)) => group.push(prepared),
            None => groups.push((prepared.node.label.clone(), vec![prepared])),
        }
    }
    let mut vertices = Vec::with_capacity(groups.len());
    let mut nodes = Vec::with_capacity(groups.len());
    for (_, group) in groups {
        let (table, node) = finalize_vertex(ctx, group).await?;
        vertices.push(table);
        nodes.push(node);
    }

    // Phase 2: edges join the in-memory vertex tables (no Parquet re-read).
    let mut edges = Vec::new();
    let mut edge_types = Vec::new();
    for &mapping in &mappings {
        for (table, edge_type) in execute_edges(ctx, db, mapping).await? {
            edges.push(table);
            edge_types.push(edge_type);
        }
    }

    let schema = GraphSchema {
        nodes,
        edges: edge_types,
    };
    Ok(GraphArData {
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
/// node `label` are UNIONed before dense ids are assigned (design §B4).
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
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    let prepared = prepare_vertex(ctx, db, mapping).await?;
    finalize_vertex(ctx, vec![prepared]).await
}

/// Project a mapping's source rows to the W0b vertex columns (no dedup/sort/
/// dense-id yet — those wait for [`finalize_vertex`], after the per-type union).
async fn prepare_vertex<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> datafusion::error::Result<PreparedVertex> {
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

    let df = ctx.read_csv(uri.as_str(), csv_options()).await?;
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
/// `subject` for a deterministic dense id (design §A1), `collect()`, prepend
/// `dense_id`, and register the table once under its type name.
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
            .map(|f| col(f.name().as_str()))
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
/// type to a [`Primitive`] → the format-neutral [`ScalarType`] (the manifest/
/// `RunStatus` derive graphar/xsd spellings from it); the predicate IRI + shape
/// cardinality ride along. Falls back to `string` when the type carries no
/// primitive (the legacy default).
fn node_property(db: &dyn fossil_base::Db, prop: &VProp<'_>) -> NodeProp {
    NodeProp {
        name: prop.name.to_string(),
        datatype: inner_primitive(db, prop.ty).map_or(ScalarType::String, primitive_to_scalar),
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

/// Resolve every [`Op::EmitEdge`] of one mapping into its adjacency data
/// ([`EdgeTable`]) plus the [`EdgeType`](GraphEdge) it contributes to the
/// schema. Reads the mapping's source once and joins it against the registered
/// vertex tables.
async fn execute_edges<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> datafusion::error::Result<Vec<(EdgeTable, GraphEdge)>> {
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
            single_valued,
            ..
        } = op
        {
            let table =
                execute_edge(ctx, &uri, edge_type, src_type, dst_type, src_id, dst_id).await?;
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

/// Materialise one edge type. Projects the source rows to `src_iri`/`dst_iri`,
/// joins both against the registered vertex tables to resolve endpoint IRIs to
/// dense ids (inner join — dangling endpoints drop, like the writer), then sorts
/// the `(src_dense, dst_dense)` pairs into CSR (`by_source`) and CSC
/// (`by_target`). Mirrors the writer's edge SQL (writer.rs:469-492).
#[allow(clippy::too_many_arguments)] // the edge spec is a flat tuple, not worth a struct here
async fn execute_edge(
    ctx: &SessionContext,
    uri: &str,
    label: &str,
    src_type: &str,
    dst_type: &str,
    src_id: &Expr<'_>,
    dst_id: &Expr<'_>,
) -> datafusion::error::Result<EdgeTable> {
    let edge_src = ctx.read_csv(uri, csv_options()).await?.select(vec![
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
        label: label.to_string(),
        src_type: src_type.to_string(),
        dst_type: dst_type.to_string(),
        by_source,
        by_target,
    })
}

/// CSV read options matching the writer's whole-file schema inference (DuckDB
/// `sample_size = -1`). DataFusion samples only the first ~1000 rows by default,
/// which mis-types a column whose early values look numeric but later turn
/// stringy (or vice-versa) — read every record so the inferred Arrow types (and
/// thus the manifest/`RunStatus` `data_type`s) match the writer (design unknown
/// #4). Trade-off: inference reads the file once before execution reads it
/// again; acceptable for parity, revisit if it bites large remote sources.
fn csv_options<'a>() -> CsvReadOptions<'a> {
    CsvReadOptions::new().schema_infer_max_records(usize::MAX)
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
fn edge_dir(src: &str, label: &str, dst: &str) -> String {
    format!("{src}_{label}_{dst}")
}

/// Total rows across a set of batches — the `count` for the wire status.
fn count_rows(batches: &[RecordBatch]) -> i64 {
    batches.iter().map(RecordBatch::num_rows).sum::<usize>() as i64
}

impl GraphArData {
    /// Build the three GraphAr manifest YAMLs (design §C4 phase 3) — the GraphAr
    /// *materializer*, a pure function of the [`GraphSchema`]: the top-level
    /// `graph.graph.yml` index, one `vertex/<Type>.vertex.yml` per node type, and
    /// one `edge/<dir>/<dir>.edge.yml` per edge type. Reuses the WASM-clean
    /// `fossil_sinks::manifest` structs.
    ///
    /// # Errors
    /// Propagates `serde_yaml_ng` serialization errors (cannot fail for these
    /// plain structs, but the signature is honest).
    pub fn manifests(&self) -> Result<Vec<ManifestFile>, serde_yaml_ng::Error> {
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

        let graph = GraphInfo::new("graph", "", vertex_paths.clone(), edge_paths.clone());
        let mut out = vec![ManifestFile {
            rel_path: "graph.graph.yml".to_string(),
            yaml: graph.to_yaml()?,
        }];

        for (node, rel_path) in self.schema.nodes.iter().zip(vertex_paths) {
            out.push(ManifestFile {
                rel_path,
                yaml: vertex_info(node).to_yaml()?,
            });
        }
        for (edge, rel_path) in self.schema.edges.iter().zip(edge_paths) {
            out.push(ManifestFile {
                rel_path,
                yaml: edge_info(edge).to_yaml()?,
            });
        }
        Ok(out)
    }

    /// Build the [`RunStatus`] wire contract keasy consumes for DCAT (design
    /// §A3/§E2) — the type/predicate metadata comes from the [`GraphSchema`], the
    /// `count`s from the materialised batches (`num_rows()`).
    #[must_use]
    pub fn run_status(&self, dest: &str) -> RunStatus {
        let vertices = self
            .vertices
            .iter()
            .map(|v| {
                let node = self.schema.node(&v.label);
                VertexStatus {
                    vertex_type: v.label.clone(),
                    rdf_type: node.and_then(|n| n.iri.clone()),
                    file: format!("vertex/{}.parquet", v.label),
                    count: Some(count_rows(&v.batches)),
                    columns: node
                        .map(|n| n.properties.iter().map(column_status).collect())
                        .unwrap_or_default(),
                }
            })
            .collect();

        let edges = self
            .edges
            .iter()
            .map(|e| {
                let dir = edge_dir(&e.src_type, &e.label, &e.dst_type);
                EdgeStatus {
                    edge_type: e.label.clone(),
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

/// A wire `ColumnStatus` for a schema property: the GraphAr `data_type` spelling
/// with the RDF predicate/xsd the governance layer (DCAT) reads, derived from
/// the format-neutral [`ScalarType`].
fn column_status(p: &NodeProp) -> ColumnStatus {
    ColumnStatus {
        name: p.name.clone(),
        data_type: graphar_spelling(p.datatype),
        rdf_uri: p.iri.clone(),
        xsd_datatype: Some(xsd_spelling(p.datatype)),
    }
}

/// The `VertexInfo` manifest for one node type. Mirrors the writer's
/// `build_vertex_manifest` (dense_id/subject/<props>/x/y/cluster_id), with the
/// **real** per-prop `data_type` the schema carries.
fn vertex_info(node: &NodeType) -> VertexInfo {
    let mut properties = Vec::with_capacity(node.properties.len() + 5);
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
        DEFAULT_CHUNK_SIZE,
        format!("vertex/{}/", node.label),
        vec![PropertyGroup {
            file_type: "parquet".to_string(),
            properties,
        }],
    );
    info.iri = node.iri.clone().unwrap_or_default();
    info
}

/// The `EdgeInfo` manifest for one edge type. W0b edges carry no properties
/// (only `src_dense`/`dst_dense`); both CSR + CSC adjacencies are ordered.
fn edge_info(edge: &GraphEdge) -> EdgeInfo {
    EdgeInfo {
        src_type: edge.source.clone(),
        edge_type: edge.label.clone(),
        iri: edge.iri.clone().unwrap_or_default(),
        dst_type: edge.destination.clone(),
        chunk_size: DEFAULT_CHUNK_SIZE,
        src_chunk_size: DEFAULT_CHUNK_SIZE,
        dst_chunk_size: DEFAULT_CHUNK_SIZE,
        directed: true,
        prefix: format!("edge/{}/", edge_dir(&edge.source, &edge.label, &edge.destination)),
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

/// Adapt fossil's internal [`Primitive`] to the contract's [`ScalarType`] (1:1).
fn primitive_to_scalar(p: Primitive) -> ScalarType {
    match p {
        Primitive::String => ScalarType::String,
        Primitive::Integer => ScalarType::Integer,
        Primitive::Float => ScalarType::Float,
        Primitive::Bool => ScalarType::Bool,
        Primitive::Date => ScalarType::Date,
        Primitive::DateTime => ScalarType::DateTime,
        Primitive::Time => ScalarType::Time,
        Primitive::GYear => ScalarType::GYear,
        Primitive::AnyURI => ScalarType::AnyUri,
    }
}

/// Inverse of [`primitive_to_scalar`] — lets the GraphAr materializer reuse the
/// `Primitive → {graphar, xsd}` spelling authority (`fossil_hir::shapes`) for a
/// schema datatype, with no duplicate spelling tables.
const fn scalar_to_primitive(s: ScalarType) -> Primitive {
    match s {
        ScalarType::String => Primitive::String,
        ScalarType::Integer => Primitive::Integer,
        ScalarType::Float => Primitive::Float,
        ScalarType::Bool => Primitive::Bool,
        ScalarType::Date => Primitive::Date,
        ScalarType::DateTime => Primitive::DateTime,
        ScalarType::Time => Primitive::Time,
        ScalarType::GYear => Primitive::GYear,
        ScalarType::AnyUri => Primitive::AnyURI,
    }
}

/// The GraphAr `data_type` spelling (`string`/`int64`/…) of a schema datatype.
fn graphar_spelling(s: ScalarType) -> String {
    primitive_to_graphar(scalar_to_primitive(s)).to_string()
}

/// The xsd datatype IRI of a schema datatype.
fn xsd_spelling(s: ScalarType) -> String {
    primitive_to_xsd(scalar_to_primitive(s))
}
