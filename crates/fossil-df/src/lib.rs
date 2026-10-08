//! `DataFusion` backend for the property-graph MIR, and the one writer of a
//! `fossil/1` corpus.
//!
//! Consumes a [`fossil_mir::lower_to_mir_pg`] graph and materialises each
//! vertex type on `DataFusion`: read the source, project `id AS subject` + each
//! prop, dedup single-valued shapes, sort by `subject`, `collect()`, and prepend
//! `dense_id` — global across the graph: the types in schema order, each one
//! contiguous range, subject order inside it. The result is registered in the
//! [`SessionContext`] so the edge phase can resolve endpoint IRIs against it in
//! memory (the hard barrier: vertices before edges), and an edge's endpoints
//! come out global too.
//!
//! [`write()`] writes the corpus — one Parquet per vertex type and per
//! relation, `fossil.json` last — and [`Executor`] is the
//! whole run a host drives: compile, register documents, read sources through
//! a [`fossil_storage::Storage`], execute, write. `fossil-df-wasm` is a
//! `wasm-bindgen` shell over it, and it is the only write host.
//!
//! The crate compiles to `wasm32-unknown-unknown` (the whole executor runs in
//! the browser). `zstd-sys` is an unavoidable C dep (datafusion 54 hardcodes
//! `arrow-ipc/zstd`), so the wasm build needs a wasm-capable clang — see the
//! `datafusion` entry in `Cargo.toml`.

// The executor is single-threaded by construction — the browser has one thread,
// and native tests drive the same future on a current-thread runtime — so a
// future that is not `Send` costs nothing here. Nor is the bound available:
// `DataFusion`'s `DataFrame`/`ExecutionPlan` futures are not `Send`, so
// satisfying the nursery lint would mean a `Send` wrapper over every await in
// the crate.
#![allow(clippy::future_not_send)]
// `result_large_err`: a run refuses with `fossil_graph_schema::Failure`, which
// is a `Problem` plus its help, its related diagnostics and its cause — 144
// bytes, over clippy's 128. It is returned once per run or per `inputs()`
// call, on the path that ends the run, so the copy costs nothing measurable;
// boxing it would put `Box<Failure>` in every signature a host reads.
#![allow(clippy::result_large_err)]

/// The output descriptor a program names, decoded from the document the
/// checker read — the one resolution every host runs with.
pub mod descriptor;
/// The run a host drives: one compiled program, its documents, its sources,
/// and the corpus it writes.
pub mod executor;
/// What a run's operators may reserve, and the compaction that keeps
/// `DataFusion`'s count of it true.
pub mod memory;
/// The relational operators executed: the walk from an emit op back to the
/// sources it reads. Its own module doc lists which operators run here.
pub mod plan;
pub mod rdf;
/// What a run tells its caller: where it wrote, and the edges the join
/// discarded.
pub mod report;
/// A `DataFusion` session whose plans never spawn — the browser has no runtime
/// to spawn on.
pub mod session;
/// The catalog rendered for this engine: which `DataFusion` function each
/// stdlib entry becomes, and which rows this engine cannot render.
pub mod stdlib;
/// The corpus, written: one Parquet per table, `fossil.json` last.
pub mod write;

pub use descriptor::output_descriptor;
/// The executor a host drives. Named at the crate root because it is the
/// crate's answer, not a detail of the module it is written in.
pub use executor::Executor;
/// Re-exported so callers name the program-resident output descriptor that
/// [`execute_graph`] / [`provider_bindings`] take: it is passed as an argument,
/// never read through `Db::system()`.
pub use fossil_descriptors_output::OutputDescriptorKind;
/// The relation an op index produces — the seam a host (or a test that builds a
/// [`fossil_mir::MirGraph`] by hand) uses to materialise an intermediate.
pub use plan::plan_relation;
/// What a run hands its caller.
pub use report::{EdgeDrops, RunReport};
/// The corpus writer, and what it answers.
pub use write::{WriteError, Written, write};

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, UInt32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::Column;
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr as DfExpr, JoinType, Operator, binary_expr};
use datafusion::prelude::{
    CsvReadOptions, DataFrame, JsonReadOptions, ParquetReadOptions, SessionContext, col, lit,
};
use fossil_base::SourceFile;
use fossil_graph_schema::{
    Cardinality, EdgeType as GraphEdge, GraphSchema, NodeType, Primitive, Property as NodeProp,
};
use fossil_hir::shapes::inner_primitive;
use fossil_hir::{MappingLoc, def_map::def_map};
use fossil_location::SourceAnchor;
use fossil_mir::{Expr, Op, SourceFormat, VProp, apply_output_shape, lower_to_mir_pg};
use fossil_sinks::generated::{ENDPOINT_SRC, PAYLOAD_ADDRESS, PAYLOAD_IDENTITY};

/// The materialised graph for a program: the canonical [`GraphSchema`] (the
/// single source of all type/predicate/cardinality metadata) plus the relation
/// data — vertex tables and edge tables as in-memory `RecordBatch`es.
///
/// This is the universal substrate made concrete: **relations + a
/// graph-schema**. The corpus is written *from* this by [`write()`]; the data
/// carriers hold no metadata of their own — it all lives in
/// [`schema`](Self::schema).
#[derive(Debug)]
pub struct Graph {
    pub schema: GraphSchema,
    pub vertices: Vec<VertexTable>,
    pub edges: Vec<EdgeTable>,
}

/// A materialised edge's rows, `ORDER BY src_dense, dst_dense`: two `u32`
/// columns, each its endpoint's global `dense_id`. The `(src_type, label,
/// dst_type)` triple identifies the edge in the schema; all other metadata is
/// in the [`GraphSchema`].
#[derive(Debug)]
pub struct EdgeTable {
    pub label: String,
    pub src_type: String,
    pub dst_type: String,
    pub batches: Vec<RecordBatch>,
    /// The sources its rows were derived from, as [`plan::derived_from`] spells
    /// them, over every mapping that writes it.
    pub derived_from: Vec<String>,
    /// Rows of this edge's input that resolved no endpoint pair, and so are not
    /// in [`Self::batches`]. See [`execute_edge`] for why they are discarded
    /// and [`report::EdgeDrops`] for where the number goes.
    pub dropped: u64,
}

/// Execute a whole program's mappings into the graph [`write()`] lays out and
/// writes.
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
/// Propagates `DataFusion` read/plan/execute errors.
// The hasher is not ours to choose: `connections` is the host's connection map —
// the browser's `parse_refs` builds one, a plain `HashMap`. Generalising over
// `BuildHasher` would add a parameter no caller can vary, which is the same call
// `fossil-df-wasm` made.
#[allow(clippy::implicit_hasher)]
#[tracing::instrument(skip_all, fields(program = %file.path(db)))]
pub async fn execute_graph<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> datafusion::error::Result<Graph> {
    // The run's anchor, derived from the PROGRAM and not from the caller: a
    // source URI is a path the program wrote, so the directory it resolves
    // against is the program's own. The executor used to hand DataFusion the
    // written path verbatim, which made the process's working directory the
    // anchor — so `io.csv("data/items.csv")` meant a different file depending
    // on where you invoked from, while `io.shex("shop.shex")` in the same
    // program was already resolved beside it.
    let program_dir = fossil_location::program_dir(file.path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let mappings: Vec<MappingLoc<'db>> = def_map(db, file).mappings(db).clone();

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
    let mut vertices = Vec::with_capacity(groups.len());
    let mut nodes = Vec::with_capacity(groups.len());
    let mut first = 0u64;
    for (_, group) in groups {
        let (table, node) = finalize_vertex(ctx, group, first).await?;
        first += table
            .batches
            .iter()
            .map(|b| b.num_rows() as u64)
            .sum::<u64>();
        vertices.push(table);
        nodes.push(node);
    }

    // Phase 2: edges join the in-memory vertex tables (no Parquet re-read).
    //
    // An edge table is keyed `(src_type, label, dst_type)`, and it is one table
    // however many mappings write it — the same rule as a vertex type above. Two
    // `Comment` mappings that both write `hasCreator` each contribute rows to
    // ONE `Comment_hasCreator_Person`; grouped here and merged in
    // `finalize_edge`, they used to be two entries under one name, and the
    // writer looks a table up by that name, so one mapping's rows were written
    // twice and the other's not at all.
    let mut edge_groups: Vec<(GraphEdge, Vec<PreparedEdge>)> = Vec::new();
    for &mapping in &mappings {
        let produced = execute_edges(ctx, db, mapping, descriptor, anchor).await?;
        for (prepared, edge_type) in produced {
            let key = |e: &GraphEdge| (e.source.clone(), e.label.clone(), e.destination.clone());
            match edge_groups
                .iter_mut()
                .find(|(t, _)| key(t) == key(&edge_type))
            {
                Some((_, group)) => group.push(prepared),
                None => edge_groups.push((edge_type, vec![prepared])),
            }
        }
    }
    let mut edges = Vec::with_capacity(edge_groups.len());
    let mut edge_types = Vec::with_capacity(edge_groups.len());
    for (edge_type, group) in edge_groups {
        edges.push(finalize_edge(&edge_type, group).await?);
        edge_types.push(edge_type);
    }

    let schema = GraphSchema {
        nodes,
        edges: edge_types,
    };
    Ok(Graph {
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
///
/// A [`Cardinality::Multi`] property is not a column of it: its values are in
/// [`Self::properties`], a table each.
#[derive(Debug)]
pub struct VertexTable {
    pub label: String,
    pub batches: Vec<RecordBatch>,
    /// The sources its rows were derived from, as [`plan::derived_from`] spells
    /// them, over every mapping that emits the type.
    pub derived_from: Vec<String>,
    pub properties: Vec<PropertyValues>,
}

/// A multi-valued property's values, in first normal form: `src`, the
/// `dense_id` of the vertex a value belongs to, and the value under the
/// property's name — a set, `ORDER BY src, value`.
#[derive(Debug)]
pub struct PropertyValues {
    pub name: String,
    pub batches: Vec<RecordBatch>,
    /// The sources of the mappings that write the property — a subset of its
    /// vertex's [`VertexTable::derived_from`].
    pub derived_from: Vec<String>,
}

/// A mapping's vertex projection before the dense-id barrier — the W0b columns
/// (`subject` + props + `x`/`y`/`cluster_id`) as an un-collected [`DataFrame`],
/// plus the [`NodeType`] schema it contributes. Several of these with the same
/// node `label` are `UNIONed` before dense ids are assigned.
struct PreparedVertex {
    node: NodeType,
    dedup: bool,
    projected: DataFrame,
    derived_from: Vec<String>,
}

/// Materialise a single mapping's VERTEX on `DataFusion` and register it — the
/// one-mapping convenience over [`prepare_vertex`] + [`finalize_vertex`].
/// Returns the table data plus the [`NodeType`] it contributes to the schema.
///
/// # Errors
/// Propagates `DataFusion` read/plan/execute errors.
#[allow(clippy::implicit_hasher)] // as `execute_graph` above.
pub async fn execute_vertex<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    let program_dir = fossil_location::program_dir(mapping.file(db).path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let prepared = prepare_vertex(ctx, db, mapping, descriptor, anchor).await?;
    finalize_vertex(ctx, vec![prepared], 0).await
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
/// The list carries no [`Op::EmitVertex`], or any `DataFusion` read/plan/execute
/// error.
pub async fn execute_vertex_ops<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    ops: &[Op<'db>],
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    let prepared = prepare_vertex_ops(ctx, db, ops, anchor).await?;
    finalize_vertex(ctx, vec![prepared], 0).await
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
        // A `Failure`, not the engine's vocabulary: the executor finds it in
        // whatever DataFusion wraps it in and answers `run/does-not-compile`
        // with the program's diagnostics.
        return Err(datafusion::error::DataFusionError::External(Box::new(
            fossil_graph_schema::Failure::new(fossil_graph_schema::Problem::DoesNotCompile {}),
        )));
    }
    Ok(())
}

/// Project a mapping's source rows to the W0b vertex columns (no dedup/sort/
/// dense-id yet — those wait for [`finalize_vertex`], after the per-type union).
#[tracing::instrument(skip_all)]
async fn prepare_vertex<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    descriptor: &OutputDescriptorKind,
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<PreparedVertex> {
    let mir = lower_to_mir_pg(db, mapping);
    refuse_if_poisoned(mir, db)?;
    let ops = apply_output_shape(mir.ops(db), &descriptor.to_graph_schema());
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
    let projected = df.select(vertex_projection(render(&id)?, &props)?)?;
    let derived_from = plan::derived_from(ops, input)?;
    let node = NodeType {
        label: type_name,
        iri: rdf_type,
        properties: props.iter().map(|p| node_property(db, p)).collect(),
    };

    Ok(PreparedVertex {
        node,
        dedup,
        projected,
        derived_from,
    })
}

/// Finalise one vertex type from the mappings that emit it: UNION their
/// projections, merge the rows of each `subject` when the shape is
/// single-valued, sort by `subject` for a deterministic dense id, `collect()`,
/// prepend `dense_id`, and register the table once under its type name.
///
/// # The mappings of one type need not write the same properties
///
/// A shape's OPTIONAL properties may be written by one mapping and left out by
/// another — `People` writes a person's `email`, and the mapping that joins
/// people to `person_knows_person` for `knows` has no reason to. So the table's
/// properties are the union of what the group writes, in the order they first
/// appear; a mapping that does not write one contributes a typed null under its
/// name, and the projections are aligned by name before the `UNION ALL`. The
/// [`NodeType`] carries the same union, where it used to carry the first
/// mapping's list and the `UNION` refused two lists of different lengths.
///
/// And a subject two mappings both mint is ONE vertex whose value for each
/// property is the one somebody wrote: `first_value(… IGNORE NULLS)` per
/// column, grouped by `subject`. Picking one whole ROW per subject, as
/// `DISTINCT ON` did, would keep the `knows` mapping's row and its null `email`
/// as often as the `People` row that has it.
///
/// # A multi-valued property is a table of its own
///
/// Merging a subject's rows is right for a single-valued column and wrong for a
/// multi-valued one: `first_value` kept the first of a person's three
/// nicknames from a CSV and dropped the other two without a word. So a
/// [`Cardinality::Multi`] property leaves the vertex row and keeps every
/// value, a row each ([`PropertyValues`]) — the shape a relational source
/// already spells it in, and the RDF pivot's list unnested into it.
#[tracing::instrument(skip_all)]
async fn finalize_vertex(
    ctx: &SessionContext,
    group: Vec<PreparedVertex>,
    first_id: u64,
) -> datafusion::error::Result<(VertexTable, NodeType)> {
    use datafusion::common::ScalarValue;
    use datafusion::functions_aggregate::expr_fn::first_value;
    use datafusion::logical_expr::{ExprFunctionExt as _, expr::NullTreatment};

    let first = group.first().expect("a type group is never empty");
    let dedup = first.dedup;
    let mut node = first.node.clone();
    for other in &group[1..] {
        for p in &other.node.properties {
            if !node.properties.iter().any(|q| q.name == p.name) {
                node.properties.push(p.clone());
            }
        }
    }

    // `subject`, then every property of the union — the column order
    // `vertex_projection` gives a single mapping.
    let mut columns: Vec<(String, DataType)> = Vec::new();
    let names = std::iter::once(PAYLOAD_IDENTITY.to_string())
        .chain(node.properties.iter().map(|p| p.name.clone()));
    for name in names {
        let ty = group
            .iter()
            .find_map(|g| {
                g.projected
                    .schema()
                    .field_with_unqualified_name(&name)
                    .ok()
                    .map(|f| f.data_type().clone())
            })
            .unwrap_or(DataType::Null);
        columns.push((name, ty));
    }

    // A property's sources are those of the mappings that write it; a mapping
    // that leaves it out contributes a null column, filtered out below.
    let sources = |name: &str| {
        union(group.iter().filter_map(|g| {
            g.projected
                .schema()
                .field_with_unqualified_name(name)
                .is_ok()
                .then_some(g.derived_from.as_slice())
        }))
    };
    let derived_from = union(group.iter().map(|g| g.derived_from.as_slice()));
    let property_sources: Vec<(String, Vec<String>)> = node
        .properties
        .iter()
        .filter(|p| p.cardinality == Cardinality::Multi)
        .map(|p| (p.name.clone(), sources(&p.name)))
        .collect();

    let mut df: Option<DataFrame> = None;
    for part in group {
        let schema = part.projected.schema().clone();
        let aligned = part.projected.select(
            columns
                .iter()
                .map(|(name, ty)| {
                    if schema.field_with_unqualified_name(name).is_ok() {
                        Ok(DfExpr::Column(Column::new_unqualified(name.as_str())))
                    } else {
                        Ok(lit(ScalarValue::try_from(ty)?).alias(name.as_str()))
                    }
                })
                .collect::<datafusion::error::Result<Vec<_>>>()?,
        )?;
        // UNION ALL — the merge (if any) happens below.
        df = Some(match df {
            None => aligned,
            Some(df) => df.union(aligned)?,
        });
    }
    let df = df.expect("a type group is never empty");
    let multi = |name: &str| {
        node.properties
            .iter()
            .any(|p| p.name == name && p.cardinality == Cardinality::Multi)
    };
    let column = |name: &str| DfExpr::Column(Column::new_unqualified(name));
    let (values, columns): (Vec<_>, Vec<_>) = columns.into_iter().partition(|(n, _)| multi(n));
    let row = df
        .clone()
        .select(columns.iter().map(|(name, _)| column(name)))?;

    let by_subject = vec![col(PAYLOAD_IDENTITY).sort(true, false)];
    let sorted = if dedup {
        // Each merged column under a positional name, renamed after. Aliased
        // straight to its input's own name, the plan was refused with
        // «duplicate unqualified field name `customer`» over the `group-by`
        // program, whose rows already come out of an aggregate keyed on that
        // column.
        let merged: Vec<DfExpr> = columns[1..]
            .iter()
            .enumerate()
            .map(|(i, (name, _))| {
                first_value(
                    DfExpr::Column(Column::new_unqualified(name.as_str())),
                    vec![],
                )
                .null_treatment(NullTreatment::IgnoreNulls)
                .build()
                .map(|e| e.alias(format!("__merged_{i}")))
            })
            .collect::<datafusion::error::Result<_>>()?;
        let renamed: Vec<DfExpr> = std::iter::once(col(PAYLOAD_IDENTITY))
            .chain(columns[1..].iter().enumerate().map(|(i, (name, _))| {
                DfExpr::Column(Column::new_unqualified(format!("__merged_{i}")))
                    .alias(name.as_str())
            }))
            .collect();
        row.aggregate(vec![col(PAYLOAD_IDENTITY)], merged)?
            .select(renamed)?
            .sort(by_subject)?
    } else {
        row.sort(by_subject)?
    };

    let batches = prepend_dense_id(sorted.collect().await?, first_id)?;
    register_batches(ctx, &node.label, &batches)?;

    let mut properties = Vec::with_capacity(values.len());
    for (name, _) in values {
        let vertex = ctx.table(node.label.as_str()).await?.select(vec![
            col(PAYLOAD_IDENTITY).alias("__subject"),
            col(PAYLOAD_ADDRESS).alias(ENDPOINT_SRC),
        ])?;
        let batches = row_per_value(
            df.clone()
                .select(vec![col(PAYLOAD_IDENTITY), column(&name)])?,
            &name,
        )?
        .filter(column(&name).is_not_null())?
        .join(
            vertex,
            JoinType::Inner,
            &[PAYLOAD_IDENTITY],
            &["__subject"],
            None,
        )?
        .select(vec![col(ENDPOINT_SRC), column(&name)])?
        .distinct()?
        .sort(vec![
            col(ENDPOINT_SRC).sort(true, false),
            column(&name).sort(true, false),
        ])?
        .collect()
        .await?;
        let derived_from = property_sources
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        properties.push(PropertyValues {
            name,
            batches,
            derived_from,
        });
    }

    Ok((
        VertexTable {
            label: node.label.clone(),
            batches,
            derived_from,
            properties,
        },
        node,
    ))
}

/// One row per value of `column`. The RDF pivot spells a multi-valued
/// predicate as a `List` and is unnested; a relational source spells it as
/// repeated rows, which is already the shape `UNNEST` produces and must not be
/// unnested a second time.
///
/// **The condition is the column's TYPE, not the declared cardinality**, and
/// the difference is a whole class of source: a shape says how many values a
/// predicate MAY carry, not how the source spells them. Reading the cardinality
/// instead cost every `*` and `+` edge over a CSV — `unnest_columns` on a
/// `Utf8` is a `DataFusion` internal error at run time with no diagnostic in
/// front of it — and that was 8 of LDBC-SNB's 21 relationships.
fn row_per_value(df: DataFrame, column: &str) -> datafusion::error::Result<DataFrame> {
    let list = matches!(
        df.schema().field_with_unqualified_name(column)?.data_type(),
        DataType::List(_) | DataType::LargeList(_) | DataType::FixedSizeList(_, _)
    );
    if list {
        df.unnest_columns(&[column])
    } else {
        Ok(df)
    }
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
        term: prop.term.clone(),
        cardinality: if prop.single_valued {
            Cardinality::Single
        } else {
            Cardinality::Multi
        },
    }
}

/// The vertex projection exprs: `id AS subject`, then each prop.
fn vertex_projection(
    subject: DfExpr,
    props: &[VProp<'_>],
) -> datafusion::error::Result<Vec<DfExpr>> {
    std::iter::once(Ok(subject.alias(PAYLOAD_IDENTITY)))
        .chain(
            props
                .iter()
                .map(|p| Ok(render(&p.value)?.alias(p.name.as_str()))),
        )
        .collect()
}

/// Prepend `dense_id` to each collected batch: `first`, `first + 1`, … in
/// batch order. The batches arrive sorted by `subject` (the plan sorts before
/// collect) and `first` is the rows of every type before this one, so the id is
/// global and a type is one contiguous range of it — the numbering the edge
/// phase joins against, and the one the corpus is written in.
///
/// # Errors
/// `run/too-large` when the graph outgrows a `u32` `dense_id`, which is the
/// width `corpus.bnf` declares.
fn prepend_dense_id(
    batches: Vec<RecordBatch>,
    first: u64,
) -> datafusion::error::Result<Vec<RecordBatch>> {
    let mut out = Vec::with_capacity(batches.len());
    let mut next = first;
    for batch in batches {
        let end = next + batch.num_rows() as u64;
        let (Ok(lo), Ok(hi)) = (u32::try_from(next), u32::try_from(end)) else {
            let failure =
                fossil_graph_schema::Failure::new(fossil_graph_schema::Problem::TooLarge {
                    vertices: end,
                });
            return Err(DataFusionError::External(Box::new(failure)));
        };
        let ids: ArrayRef = Arc::new(UInt32Array::from_iter_values(lo..hi));

        let mut fields: Vec<Arc<Field>> = vec![Arc::new(Field::new(
            PAYLOAD_ADDRESS,
            DataType::UInt32,
            false,
        ))];
        fields.extend(batch.schema().fields().iter().cloned());
        let mut columns: Vec<ArrayRef> = vec![ids];
        columns.extend(batch.columns().iter().cloned());

        out.push(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?);
        next = end;
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

/// One mapping's contribution to an edge table before the tables are merged:
/// its resolved `(src_dense, dst_dense)` pairs, not yet collected, and how many
/// of its rows named an endpoint no vertex carries. Several of these with the
/// same `(src_type, label, dst_type)` are one table — see [`finalize_edge`].
struct PreparedEdge {
    resolved: DataFrame,
    dropped: u64,
    derived_from: Vec<String>,
}

/// Resolve every [`Op::EmitEdge`] of one mapping into its resolved pairs
/// ([`PreparedEdge`]) plus the [`EdgeType`](GraphEdge) it contributes to the
/// schema. Reads the mapping's source once and joins it against the registered
/// vertex tables.
#[tracing::instrument(skip_all)]
async fn execute_edges<'db>(
    ctx: &SessionContext,
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    descriptor: &OutputDescriptorKind,
    anchor: SourceAnchor<'_>,
) -> datafusion::error::Result<Vec<(PreparedEdge, GraphEdge)>> {
    let mir = lower_to_mir_pg(db, mapping);
    refuse_if_poisoned(mir, db)?;
    let ops = apply_output_shape(mir.ops(db), &descriptor.to_graph_schema());
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
            let derived_from = plan::derived_from(ops, *input)?;
            let prepared =
                execute_edge(ctx, rows, derived_from, src_type, dst_type, src_id, dst_id).await?;
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
            out.push((prepared, edge_type));
        }
    }
    Ok(out)
}

/// Resolve one mapping's edge. Projects the edge op's input relation (`rows`) to
/// `src_iri`/`dst_iri` and joins both against the registered vertex tables to
/// resolve endpoint IRIs to type-local dense ids. The pairs stay lazy:
/// [`finalize_edge`] merges every mapping's pairs for the table, dedups and
/// sorts them. The write renumbers both into the global `dense_id` and sorts
/// again, so that order is the executor's, not the corpus's.
///
/// # The join is inner, and the discard is counted
///
/// A row whose `src_iri` or `dst_iri` names a subject no vertex carries
/// resolves nothing and does not become an edge. **That is intended and it
/// stays** — a corpus cannot hold an edge to a vertex that is not there, and
/// the `no-dangling-endpoint` guard reads every endpoint back and fails on a
/// `dense_id` no vertex has.
///
/// What it stopped being is silent. It reported nothing at any log level, and
/// the only trace was an [`EdgeInfo::edge_count`] smaller than the input's row
/// count — a comparison nobody is obliged to make. [`EdgeTable::dropped`] is
/// `candidates − resolved`: the extra `count()` is one more pass over the edge's
/// input relation, which the run already scans twice (once per orientation).
///
/// This used to say it mirrored `fossil-sinks`'s `writer.rs`. There is no
/// second writer to mirror any more — `fossil-sinks/src/` is the manifest model
/// and nothing else, so THIS is where an edge becomes CSR/CSC. Both ends are
/// already the global `dense_id`, and `crate::write()` writes the pair as it is.
async fn execute_edge(
    ctx: &SessionContext,
    rows: DataFrame,
    derived_from: Vec<String>,
    src_type: &str,
    dst_type: &str,
    src_id: &Expr<'_>,
    dst_id: &Expr<'_>,
) -> datafusion::error::Result<PreparedEdge> {
    let edge_src = rows.select(vec![
        render(src_id)?.alias("src_iri"),
        render(dst_id)?.alias("dst_iri"),
    ])?;
    // A multi-valued edge is an edge per object, however the source spells it.
    let edge_src = row_per_value(edge_src, "dst_iri")?;
    // Pre-project each vertex table to (subject, dense) with disjoint names so
    // the two joins never collide on `subject`/`dense_id`.
    let src_v = ctx.table(src_type).await?.select(vec![
        col(PAYLOAD_IDENTITY).alias("v_src_subject"),
        col(PAYLOAD_ADDRESS).alias("src_dense"),
    ])?;
    let dst_v = ctx.table(dst_type).await?.select(vec![
        col(PAYLOAD_IDENTITY).alias("v_dst_subject"),
        col(PAYLOAD_ADDRESS).alias("dst_dense"),
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

    // The pairs this mapping offered, before the dedup in [`finalize_edge`].
    let matched = resolved.clone().count().await? as u64;

    // `saturating_sub` because the subtraction is only exact while a subject
    // identifies at most one vertex: a type materialised without dedup can hold
    // two rows with the same `subject`, and then one candidate resolves to two
    // edges. That corpus already violates `identity-is-the-subject`
    // (`packages/corpus/guards/guards.mjs`), which is the guard that catches it.
    //
    // **Against `matched` and not against what was written**, because the two
    // stopped being the same number when the dedup in [`finalize_edge`] landed.
    // `dropped` says *an endpoint named a subject no vertex carries*, which is a
    // defect in the program or the data; a row removed as a duplicate is
    // neither, and folding the two together would report a clean six-type
    // program as dropping 96% of its edges.
    let dropped = candidates.saturating_sub(matched);

    Ok(PreparedEdge {
        resolved,
        dropped,
        derived_from,
    })
}

/// Merge every mapping's pairs for one edge table into the table: `UNION ALL`,
/// then the set, then the order.
///
/// **An edge set is a set**, and until the dedup it was whatever the input
/// relation's row count happened to be. A mapping whose source is a JOIN
/// repeats its subject once per matching row, so every OTHER edge the mapping
/// writes is repeated with it: a six-type program producing `Person` from
/// `People.join(Interests, …)` wrote `Person_isLocatedIn_Place` **1,256 times
/// for 50 distinct pairs** — 1,206 duplicates in a `{1,1}` edge. Two mappings
/// of one type writing the same edge are the same case one level up: their
/// pairs overlap wherever their rows mint the same subject.
///
/// Nothing caught it and nothing could: every corpus check then asked whether
/// the stored relation was self-consistent, and a duplicate is *in* the
/// relation. `EmitVertex` has carried `dedup: true` for exactly this reason
/// since the first join landed, and [`Cardinality::Single`]'s own doc — *"a
/// materializer dedups by key"* — is the sentence this makes true.
///
/// **It is unconditional, and the reason is the schema rather than the
/// cardinality**: an edge table carries `src` and `dst` and nothing else, so two
/// identical rows are not two edges a reader could tell apart — they are one
/// edge stored twice. There is nowhere for a multiplicity to live. The day an
/// edge carries a property, this becomes a decision with two answers and the
/// `Multi` arm is the one that changes.
#[tracing::instrument(skip_all, fields(edge = %edge.label))]
async fn finalize_edge(
    edge: &GraphEdge,
    group: Vec<PreparedEdge>,
) -> datafusion::error::Result<EdgeTable> {
    let derived_from = union(group.iter().map(|g| g.derived_from.as_slice()));
    let mut dropped = 0;
    let mut merged: Option<DataFrame> = None;
    for part in group {
        dropped += part.dropped;
        merged = Some(match merged {
            None => part.resolved,
            Some(df) => df.union(part.resolved)?,
        });
    }
    let merged = merged.ok_or_else(|| {
        DataFusionError::Internal(format!("edge `{}` has no contributing mapping", edge.label))
    })?;
    let batches = merged
        .distinct()?
        .sort(vec![
            col("src_dense").sort(true, false),
            col("dst_dense").sort(true, false),
        ])?
        .collect()
        .await?;

    Ok(EdgeTable {
        label: edge.label.clone(),
        src_type: edge.source.clone(),
        dst_type: edge.destination.clone(),
        batches,
        derived_from,
        dropped,
    })
}

/// The union of several [`plan::derived_from`] sets, as one: sorted, no repeats.
fn union<'a>(sets: impl Iterator<Item = &'a [String]>) -> Vec<String> {
    let mut all: Vec<String> = sets.flatten().cloned().collect();
    all.sort();
    all.dedup();
    all
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
        SourceFormat::Csv { delimiter } => {
            let options = csv_options(delimiter.as_deref());
            let schema = csv_schema(ctx, uri, &options).await?;
            ctx.read_csv(uri, options.schema(&schema)).await
        }
        SourceFormat::Json => read_json_source(ctx, uri).await,
        SourceFormat::Parquet => {
            let options = ParquetReadOptions::default();
            source_files(ctx, uri, options.file_extension).await?;
            ctx.read_parquet(uri, options).await
        }
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

/// The columns of a CSV source, under the names the checker typed it with.
///
/// A header may name one column twice — LDBC's `person_knows_person` is
/// `Person.id|Person.id|creationDate` — and `DataFusion`'s own inference
/// cannot carry it: it merges the per-file schemas with `Schema::try_merge`,
/// which folds two fields of one name into one, and the read then fails on the
/// first row with `incorrect number of fields`. So the schema is inferred here,
/// per file and before any merge, through the same `CsvFormat` the read uses;
/// every file's names are made unique by [`unique_column_names`]; and the read
/// below takes the result and does not infer a second time.
async fn csv_schema(
    ctx: &SessionContext,
    uri: &str,
    options: &CsvReadOptions<'_>,
) -> datafusion::error::Result<Schema> {
    use datafusion::datasource::file_format::csv::CsvFormat;
    use datafusion::datasource::file_format::options::ReadOptions as _;
    use futures::{StreamExt as _, TryStreamExt as _};
    use object_store::ObjectStoreExt as _;

    let state = ctx.state();
    let listing = options.to_listing_options(&ctx.copied_config(), ctx.copied_table_options());
    let format = (listing.format.as_ref() as &dyn std::any::Any)
        .downcast_ref::<CsvFormat>()
        .ok_or_else(|| DataFusionError::Internal("CSV options without a CSV format".into()))?;
    let (store, objects) = source_files(ctx, uri, &listing.file_extension).await?;
    let mut remaining = options.schema_infer_max_records;
    let mut schemas = Vec::with_capacity(objects.len());
    for object in objects {
        let bytes = store
            .get(&object.location)
            .await
            .map_err(|e| DataFusionError::ObjectStore(Box::new(e)))?
            .into_stream()
            .map_err(|e| DataFusionError::ObjectStore(Box::new(e)))
            .boxed();
        let chunks = format.read_to_delimited_chunks_from_stream(bytes).await;
        let (schema, read) = format
            .infer_schema_from_stream(&state, remaining, chunks)
            .await?;
        let names = unique_column_names(schema.fields().iter().map(|f| f.name().as_str()));
        schemas.push(Schema::new(
            schema
                .fields()
                .iter()
                .zip(names)
                .map(|(f, name)| f.as_ref().clone().with_name(name))
                .collect::<Vec<_>>(),
        ));
        remaining = remaining.saturating_sub(read);
        if remaining == 0 {
            break;
        }
    }
    Ok(Schema::try_merge(schemas)?)
}

/// The files `uri` names in its store, and `source/not-found` when it names
/// none.
///
/// `DataFusion` reads a location that matches nothing as a relation with no
/// columns — a `HEAD` that answers not-found is retried as a prefix, and an
/// empty listing infers an empty schema — so a missing file failed later, at
/// the first column the program read, as `No field named <row>.<column>`: a
/// sentence about the program for a mistake in the store. The listing is the
/// one `read_csv` and `read_parquet` make themselves.
async fn source_files(
    ctx: &SessionContext,
    uri: &str,
    extension: &str,
) -> datafusion::error::Result<(
    Arc<dyn object_store::ObjectStore>,
    Vec<object_store::ObjectMeta>,
)> {
    use datafusion::datasource::listing::ListingTableUrl;
    use futures::TryStreamExt as _;

    let table = ListingTableUrl::parse(uri)?;
    let store = ctx.runtime_env().object_store(&table)?;
    let objects: Vec<object_store::ObjectMeta> = table
        .list_all_files(&ctx.state(), store.as_ref(), extension)
        .await?
        .try_collect()
        .await?;
    if objects.is_empty() {
        return Err(source_not_found(uri));
    }
    Ok((store, objects))
}

/// `source/not-found` for `uri`, as a `Failure` the executor finds in whatever
/// `DataFusion` wraps it in — the way [`refuse_if_poisoned`] carries its own.
fn source_not_found(uri: &str) -> DataFusionError {
    DataFusionError::External(Box::new(fossil_graph_schema::Failure::new(
        fossil_graph_schema::Problem::SourceNotFound {
            location: uri.to_string(),
        },
    )))
}

/// A header's names made unique by `DuckDB`'s rule, because `DuckDB` is the
/// engine that DESCRIBES a source (`fossil-introspect`, `@fossil-lang/introspect`)
/// and the checker types a program against what it describes: a name already
/// taken — compared without case — becomes `name_1`, then `name_2`, and a
/// generated name that is itself taken grows another suffix. `a|a|a_1|b|a` is
/// `a, a_1, a_1_1, b, a_2`, which is what `read_csv_auto` answers (`DuckDB`
/// 1.5).
fn unique_column_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut taken: HashMap<String, usize> = HashMap::new();
    names
        .map(|name| {
            let mut name = name.to_string();
            while let Some(count) = taken.get_mut(&name.to_lowercase()) {
                *count += 1;
                name = format!("{name}_{count}");
            }
            taken.insert(name.to_lowercase(), 0);
            name
        })
        .collect()
}

/// Read `io.json`, whichever of the two JSON shapes the file is.
///
/// **`DataFusion`'s `read_json` is newline-delimited only**, and `sightings`
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
    // `table_url` and not `url`, which is one letter from the `uri` parameter and
    // names a different thing: `uri` is what the program wrote, this is what the
    // object store resolves it to.
    let table_url = datafusion::datasource::listing::ListingTableUrl::parse(uri)?;
    let store = ctx.runtime_env().object_store(&table_url)?;
    let data = store
        .get_opts(
            table_url.prefix(),
            datafusion::object_store::GetOptions::default(),
        )
        .await
        .map_err(|e| match e {
            object_store::Error::NotFound { .. } => source_not_found(uri),
            e => DataFusionError::Execution(format!("read `{uri}`: {e}")),
        })?
        .bytes()
        .await
        .map_err(|e| DataFusionError::Execution(format!("read `{uri}`: {e}")))?;
    Ok(data.to_vec())
}

/// CSV read options matching the writer's whole-file schema inference (`DuckDB`
/// `sample_size = -1`). `DataFusion` samples only the first ~1000 rows by default,
/// which mis-types a column whose early values look numeric but later turn
/// stringy (or vice-versa) — read every record so the inferred Arrow types (and
/// thus the manifest's `data_type`s) match the writer. Trade-off: inference
/// reads the file once before execution reads it again; acceptable for parity,
/// revisit if it bites large remote sources.
///
/// # The delimiter
///
/// `delimiter` is what the program wrote (`io.csv("u.csv", delimiter = "|")`)
/// and `None` is ABSENT — so this leaves `CsvReadOptions`' own answer alone
/// rather than substituting a comma of its own. That distinction is not
/// cosmetic: `read_csv_auto`, which `fossil-introspect` DESCRIBES the same file
/// through, SNIFFS the delimiter, so «absent» is two different answers on the
/// two readers and only a named delimiter makes them one. `catalogue.bnf`
/// states no default for that reason and neither does this.
///
/// The option is one BYTE here and one SQL keyword there (`delim=`), which is
/// why the catalogue names neither: it names the position the program writes.
/// `fossil_hir::lower::check_reader_option` has already refused anything that
/// is not a single BYTE — which is why the rule there is stated in bytes and
/// not in `char`s, a two-byte character being one `char` this reader cannot
/// take. So the match below is total for every delimiter a checked program can
/// produce, and anything that somehow reached here unchecked falls back to the
/// reader's own default rather than to a silent truncation.
fn csv_options(delimiter: Option<&str>) -> CsvReadOptions<'_> {
    let opts = CsvReadOptions::new().schema_infer_max_records(usize::MAX);
    match delimiter.and_then(|d| {
        let mut bytes = d.bytes();
        match (bytes.next(), bytes.next()) {
            (Some(b), None) => Some(b),
            _ => None,
        }
    }) {
        Some(b) => opts.delimiter(b),
        None => opts,
    }
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
/// predicate) — no `ShEx` re-parse here, and unused shape predicates are not
/// materialised (the relation stays minimal). The `ShEx` descriptor's role is
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
#[allow(clippy::implicit_hasher)] // as `execute_graph` above.
pub fn provider_bindings(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    connections: &HashMap<String, String>,
) -> Vec<ProviderBinding> {
    let program_dir = fossil_location::program_dir(file.path(db));
    let anchor = SourceAnchor::new(&program_dir, connections);
    let mappings = def_map(db, file).mappings(db).clone();
    let schema = descriptor.to_graph_schema();
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
            } => Some((anchor.location(uri), binding.to_string())),
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
/// table `KB` would collide with the vertex table `KB` (`DataFusion` folds
/// identifiers to lowercase, so even case wouldn't save it).
fn provider_table_name(binding: &str) -> String {
    format!("__rdf_src_{binding}")
}

/// Render a MIR [`Expr`] to a `DataFusion` logical [`DfExpr`]. Total over the
/// MIR expression space since F2 §2 — there is no `unimplemented!()` left to
/// reach, which is what makes a property that type-checks a property that runs.
pub(crate) fn render(e: &Expr<'_>) -> datafusion::error::Result<DfExpr> {
    use fossil_hir::{BinOp, UnOp};
    Ok(match e {
        Expr::LitString(s) => lit(s.to_string()),
        // The source is the binding the author wrote (`User.email`), and
        // `plan_relation` qualifies each source relation under exactly that
        // name — which is what makes `Node.label` and `Other.label` two columns
        // after a self-join.
        Expr::ColRef { source, column } => plan::column(source, column),
        Expr::Concat(a, b) => binary_expr(render(a)?, Operator::StringConcat, render(b)?),
        Expr::Assert { inner, .. } => render(inner)?,
        Expr::Call { func, args, .. } => render_call(func.as_str(), args)?,
        Expr::LitInt(v) => lit(*v),
        Expr::LitFloat(v) => lit(v.get()),
        Expr::LitBool(b) => lit(*b),
        // The one comparison whose SQL is not its spelling — `x != NULL` is
        // NULL and not true, so this is where the surface's `x != null` gets
        // the operator it means.
        Expr::IsNull { operand, negated } => {
            let inner = render(operand)?;
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
            datafusion::logical_expr::cast(render(lhs)?, DataType::Float64),
            Operator::Divide,
            render(rhs)?,
        ),
        Expr::BinOp { op, lhs, rhs, .. } => {
            binary_expr(render(lhs)?, df_operator(*op), render(rhs)?)
        }
        // `-x` and `not x`, as themselves. Rendering `-x` as `0 - x` would make
        // `-0.0` come out `+0.0` — measured, and the reason
        // `fossil_hir::HirExpr::UnaryOp` is a node at all.
        Expr::UnaryOp { op, operand, .. } => match op {
            UnOp::Neg => DfExpr::Negative(Box::new(render(operand)?)),
            UnOp::Not => DfExpr::Not(Box::new(render(operand)?)),
        },
        // A two-armed CASE. `otherwise` is always present — fossil has no
        // one-armed conditional, so no row can fall through to NULL.
        Expr::Ternary {
            cond,
            then,
            otherwise,
            ..
        } => {
            datafusion::prelude::when(render(cond)?, render(then)?).otherwise(render(otherwise)?)?
        }
    })
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
fn render_call(func: &str, args: &[Expr<'_>]) -> datafusion::error::Result<DfExpr> {
    use fossil_hir::stdlib::LoweringKind;

    let rendered = args
        .iter()
        .map(render)
        .collect::<datafusion::error::Result<Vec<_>>>()?;
    let Some(entry) = fossil_hir::stdlib::stdlib().lookup(func) else {
        return Err(unsupported_call(func));
    };
    match &entry.lowering {
        LoweringKind::Expr(template) => {
            render_expr_template(template.as_str(), &rendered).map_err(|_| unsupported_call(func))
        }
        LoweringKind::Op(_) => Err(unsupported_call(func)),
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
/// programmatically. `sqlparser` would be a new dependency for one call site.
///
/// What is read here is not SQL. It is the closed expression language the
/// CATALOGUE writes, which is nine shapes wide and enumerated on
/// [`TemplateReader`]. A template outside it is an `Err` that names itself, so
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
/// `Err` fails the plan as `unsupported/expression`, never a dropped column.
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

/// A reader over one catalogue template. Its grammar is the whole of it, and
/// the reason a SQL parser is not needed:
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
struct TemplateReader<'a> {
    chars: Vec<char>,
    pos: usize,
    args: &'a [DfExpr],
}

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

/// A call the engine cannot make: the plan fails with fossil's own
/// `unsupported/expression`, which [`crate::executor`] finds wherever
/// `DataFusion` wrapped it.
fn unsupported_call(func: &str) -> datafusion::error::DataFusionError {
    datafusion::error::DataFusionError::External(Box::new(fossil_graph_schema::Failure::new(
        fossil_graph_schema::Problem::UnsupportedExpression {
            expression: func.to_owned(),
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::unique_column_names;

    /// A plan operator in a value position fails the plan with fossil's
    /// `unsupported/expression` — before, it rendered as a NULL column under an
    /// alias nothing read, and the run wrote the property empty.
    #[test]
    fn a_call_the_engine_cannot_make_fails_the_plan() {
        use fossil_hir::ty::{Ty, TyKind};
        let db = fossil_base::test_support::new_db();
        let call = fossil_mir::Expr::Call {
            func: "seq.where".into(),
            args: vec![],
            ty: Ty::new(
                &db,
                TyKind::Primitive(fossil_graph_schema::Primitive::String),
            ),
        };
        let err = super::render(&call).expect_err("a plan operator is not a value");
        let datafusion::error::DataFusionError::External(e) = err else {
            panic!("expected fossil's failure, got {err}");
        };
        let failure = e
            .downcast_ref::<fossil_graph_schema::Failure>()
            .expect("a Failure");
        assert!(matches!(
            &failure.problem,
            fossil_graph_schema::Problem::UnsupportedExpression { expression } if expression == "seq.where"
        ));
    }

    #[test]
    fn a_repeated_header_is_named_as_duckdb_names_it() {
        // `read_csv_auto` (DuckDB 1.5) over `a|a|a_1|b|a` and `Id|id|ID_1|x`:
        // the comparison ignores case, and a generated name that is itself
        // taken grows another suffix.
        let names = |h: &[&str]| unique_column_names(h.iter().copied());
        assert_eq!(
            names(&["a", "a", "a_1", "b", "a"]),
            ["a", "a_1", "a_1_1", "b", "a_2"]
        );
        assert_eq!(
            names(&["Id", "id", "ID_1", "x"]),
            ["Id", "id_1", "ID_1_1", "x"]
        );
        assert_eq!(
            names(&["Person.id", "Person.id", "creationDate"]),
            ["Person.id", "Person.id_1", "creationDate"]
        );
    }
}
