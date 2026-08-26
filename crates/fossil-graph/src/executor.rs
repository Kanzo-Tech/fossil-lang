//! Verb execution: the [`DuckExecutor`] seam + per-verb SQL generation.
//!
//! The verb→SQL logic lives here (pure string building, WASM-safe) so every
//! binding reuses it: the native impl (`fossil-layout`, `duckdb` crate) and
//! the WASM impl (`fossil-wasm`, DuckDB-WASM) differ only in how they satisfy
//! [`DuckExecutor`] — they never re-derive a verb's SQL
//!.
//!
//! [`dispatch`] matches the [`Operation`] enum and returns the verb's `Result`
//! serialised to JSON — the transport-agnostic wire form every binding ships.
//!
//! The verb futures are intentionally not `Send`: the two hosts are
//! single-threaded — DuckDB-WASM runs on one browser thread and the native
//! runtime drives the future with a single-thread `block_on`. Requiring `Send`
//! would force the bound onto every executor (and DuckDB-WASM, which is not
//! `Send`) for zero benefit, so `future_not_send` is allowed crate-wide here.
#![allow(clippy::future_not_send)]

use std::fmt::Write;

use serde_json::Value;

use crate::manifest::RESERVED_VERTEX_COLUMNS;
use crate::manifest::{Manifest, edge_table_name};
use crate::operations::aggregate::{AggregateParams, AggregateResult, AggregateRow, Aggregation};
use crate::operations::discovery::{
    ExpandMode, ExpandParams, ExpandResult, GraphEdge, GraphVertex, PathParams, PathResult,
    ReadParams, ReadResult,
};
use crate::operations::schema::{
    EdgeTypeSummary, FieldRole, FieldStat, SchemaParams, SchemaResult, VertexTypeSummary,
};
use crate::operations::sql::{ColumnDescriptor, ExecuteSqlParams, ExecuteSqlResult};
use crate::{GraphError, Operation, Result};

/// What [`DuckExecutor::query_columns`] gives back: the result rows, and the
/// `(name, type)` descriptor of each column.
///
/// It was `pub type ColumnedRows = (Vec<(String, String)>, Vec<Value>)`, and
/// the name and the shape were the same complaint. A two-`Vec` tuple is
/// destructured positionally at every call site — `let (columns, mut rows) =`
/// here, `.map(|(_, rows)| rows)` in the native executor — so which `Vec` is
/// which was carried by argument order and by a name («columned») that had to
/// be read twice. Two fields say it once.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// `(column_name, column_type)`, in the order the query returns them.
    ///
    /// The type string is whatever the binding can say: the native executor
    /// reads `DuckDB`'s own logical-type spelling off the executed statement,
    /// and the default below derives it from the JSON value kinds, which is
    /// best-effort and says so.
    pub columns: Vec<(String, String)>,
    /// One JSON object per row, column name → value.
    pub rows: Vec<Value>,
}

/// The thin `DuckDB` seam. A binding implements
/// exactly this — run a query, hand back rows as JSON objects — and inherits
/// every verb's SQL for free.
pub trait DuckExecutor {
    /// Run `sql`, returning each result row as a JSON object (column → value).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::Execution`] (carrying the binding's stringified
    /// DB error) when the query fails.
    fn query_json(&self, sql: &str) -> impl std::future::Future<Output = Result<Vec<Value>>>;

    /// Run `sql`, returning `(column_name, column_type)` descriptors alongside
    /// the rows. Only [`Operation::ExecuteSql`] needs column types; the default
    /// derives them best-effort from the JSON value kinds, and a binding with
    /// access to real `DuckDB` column types (the native runtime) overrides this.
    ///
    /// # Errors
    ///
    /// As [`DuckExecutor::query_json`].
    fn query_columns(&self, sql: &str) -> impl std::future::Future<Output = Result<QueryResult>> {
        async move {
            let rows = self.query_json(sql).await?;
            let columns = rows
                .first()
                .and_then(Value::as_object)
                .map(|obj| {
                    obj.iter()
                        .map(|(name, value)| (name.clone(), json_kind(value).to_string()))
                        .collect()
                })
                .unwrap_or_default();
            Ok(QueryResult { columns, rows })
        }
    }
}

/// Everything a verb needs to execute: the parsed [`Manifest`] + a `DuckDB`
/// seam. Internal — the public entry point is the free [`dispatch`] function so
/// there is a single way to run a verb.
struct Context<'a, E: DuckExecutor> {
    manifest: &'a Manifest,
    exec: &'a E,
}

/// Dispatch one [`Operation`] against the manifest + executor.
///
/// Returns the verb's `Result` as JSON. Every variant is handled: the four
/// that had no implementation were deleted rather than stubbed, so the match
/// is exhaustive and a new verb cannot be added without wiring it.
///
/// # Errors
///
/// Propagates verb execution errors.
pub async fn dispatch<E: DuckExecutor>(
    op: &Operation,
    manifest: &Manifest,
    exec: &E,
) -> Result<Value> {
    Context { manifest, exec }.dispatch(op).await
}

impl<E: DuckExecutor> Context<'_, E> {
    async fn dispatch(&self, op: &Operation) -> Result<Value> {
        match op {
            Operation::Schema(p) => to_json(&self.schema(p).await?),
            Operation::Aggregate(p) => to_json(&self.aggregate(p).await?),
            Operation::Read(p) => to_json(&self.read(p).await?),
            Operation::Expand(p) => to_json(&self.expand(p).await?),
            Operation::Path(p) => to_json(&self.path(p).await?),
            Operation::ExecuteSql(p) => to_json(&self.execute_sql(p).await?),
        }
    }

    // ── Schema verb ───────────────────────────────────────────────────────

    /// The manifest, and — only if asked — what the data says about a type's
    /// fields.
    ///
    /// The three calls this verb replaced differed in what they cost, not in
    /// what they were: two listings that read the manifest, and one that ran a
    /// query per field. **That difference is now a parameter.** A bare call
    /// touches no field; naming a type costs one batched query; naming a field
    /// costs one more, for its samples.
    async fn schema(&self, p: &SchemaParams) -> Result<SchemaResult> {
        // Fail on an unknown type before spending the listing queries.
        if let Some(vertex_type) = p.vertex_type.as_deref() {
            self.manifest.lookup_vertex(vertex_type)?;
        }

        let mut vertices = Vec::with_capacity(self.manifest.vertices().len());
        for info in self.manifest.vertices() {
            vertices.push(VertexTypeSummary {
                name: info.vertex_type.clone(),
                iri: info.iri.clone(),
                count: self.count_rows(&info.vertex_type).await?,
                fields: self.manifest.vertex_fields(&info.vertex_type)?,
            });
        }

        let mut edges = Vec::with_capacity(self.manifest.edges().len());
        for info in self.manifest.edges() {
            let table_name = edge_table_name(info);
            let count = self.count_rows(&table_name).await?;
            edges.push(EdgeTypeSummary {
                source_type: info.src_type.clone(),
                name: info.edge_type.clone(),
                target_type: info.dst_type.clone(),
                iri: info.iri.clone(),
                count,
                table_name,
            });
        }

        let fields = match p.vertex_type.as_deref() {
            None => Vec::new(),
            Some(vertex_type) => self.field_stats(vertex_type, p.field.as_deref()).await?,
        };

        Ok(SchemaResult {
            vertices,
            edges,
            fields,
        })
    }

    /// Per-field cardinality + role for one vertex type in ONE query
    /// (`COUNT(*)` + a `COUNT(DISTINCT)` per field) — the single source for what
    /// keasy used to compute client-side (`computeColumnStats` + `inferRole`).
    ///
    /// `field` narrows the answer to that one column and is what earns the
    /// extra samples query: it is the only shape where sampling costs one query
    /// rather than one per column.
    async fn field_stats(&self, vertex_type: &str, field: Option<&str>) -> Result<Vec<FieldStat>> {
        // User fields in manifest order, reserved (writer) columns filtered —
        // same source as the summary's field list.
        let mut fields: Vec<(String, String)> = self
            .manifest
            .lookup_vertex(vertex_type)?
            .property_groups
            .iter()
            .flat_map(|g| g.properties.iter())
            .filter(|prop| !RESERVED_VERTEX_COLUMNS.contains(&prop.name.as_str()))
            .map(|prop| (prop.name.clone(), prop.data_type.clone()))
            .collect();

        if let Some(name) = field {
            fields.retain(|(candidate, _)| candidate == name);
            if fields.is_empty() {
                return Err(GraphError::UnknownEntity {
                    kind: "field",
                    name: name.to_string(),
                });
            }
        }
        if fields.is_empty() {
            return Ok(Vec::new());
        }

        let mut selects = String::from("count(*) AS n");
        for (i, (name, _)) in fields.iter().enumerate() {
            let _ = write!(selects, ", count(DISTINCT {}) AS d{i}", quote_ident(name));
        }
        let rows = self
            .exec
            .query_json(&format!(
                "SELECT {selects} FROM {}",
                quote_ident(vertex_type)
            ))
            .await?;
        let row = rows.first().ok_or_else(|| {
            GraphError::Execution(format!("schema stats on `{vertex_type}` returned no row"))
        })?;
        let count = row.get("n").and_then(Value::as_u64).unwrap_or(0);

        let mut out: Vec<FieldStat> = fields
            .iter()
            .enumerate()
            .map(|(i, (name, datatype))| {
                let distinct = row
                    .get(format!("d{i}"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                FieldStat {
                    role: infer_role(name, datatype, Some(distinct), Some(count)),
                    name: name.clone(),
                    datatype: datatype.clone(),
                    distinct,
                    samples: Vec::new(),
                }
            })
            .collect();

        // One field named, so one extra query — the whole reason samples are not
        // part of a per-type answer.
        if let (Some(name), Some(stat)) = (field, out.first_mut()) {
            stat.samples = self.field_samples(vertex_type, name).await?;
        }
        Ok(out)
    }

    /// Up to 8 non-null values of one column, for a caller deciding what a
    /// field holds.
    async fn field_samples(&self, vertex_type: &str, field: &str) -> Result<Vec<String>> {
        let rows = self
            .exec
            .query_json(&format!(
                "SELECT {field} AS sample FROM {table} WHERE {field} IS NOT NULL LIMIT 8",
                field = quote_ident(field),
                table = quote_ident(vertex_type),
            ))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get("sample"))
            .map(value_to_string)
            .collect())
    }

    // ── Aggregation verbs ─────────────────────────────────────────────────

    /// One grouping, over values or over ranges.
    ///
    /// `bins` is the whole of what `histogram` used to be a separate verb for.
    /// Both arms end in one `GROUP BY` with a bounded cardinality; they differ
    /// only in where the group key comes from.
    async fn aggregate(&self, p: &AggregateParams) -> Result<AggregateResult> {
        self.manifest.lookup_vertex(&p.vertex_type)?;
        match p.bins {
            None => self.aggregate_by_value(p).await,
            // `limit` caps rows in both arms, so it caps bins here: one meaning.
            Some(bins) => {
                self.aggregate_by_range(p, bins.clamp(1, p.limit.max(1)))
                    .await
            }
        }
    }

    /// `GROUP BY` the column, ordered by the aggregate and cut at `limit` —
    /// constant memory by the cap.
    async fn aggregate_by_value(&self, p: &AggregateParams) -> Result<AggregateResult> {
        let table = quote_ident(&p.vertex_type);
        let group = quote_ident(&p.group_by);
        let agg_expr = agg_expr(p)?;
        let sql = format!(
            "SELECT {group} AS grp, {agg_expr}::DOUBLE AS val FROM {table} \
             GROUP BY {group} ORDER BY val DESC LIMIT {}",
            p.limit
        );
        let rows = self
            .exec
            .query_json(&sql)
            .await?
            .iter()
            .map(|r| AggregateRow {
                group: r.get("grp").cloned().unwrap_or(Value::Null),
                value: r.get("val").and_then(Value::as_f64).unwrap_or(0.0),
            })
            .collect();
        Ok(AggregateResult {
            rows,
            edges: Vec::new(),
        })
    }

    /// The same `GROUP BY` with the key computed from an equal-width range of
    /// the column instead of read from it.
    ///
    /// The answer is dense: one row per bin, `0.0` where nothing landed. A
    /// grouping over values omits its empty groups because it cannot know them;
    /// a grouping over ranges knows exactly how many there are, and a gap in a
    /// binned answer is information.
    ///
    /// Only a numeric or temporal column has ranges. A categorical one is
    /// rejected rather than silently grouped by value, because the caller that
    /// asked for bins would get an answer of a shape it did not ask for — and
    /// grouping it by value is one call away.
    async fn aggregate_by_range(&self, p: &AggregateParams, bins: u32) -> Result<AggregateResult> {
        let datatype = self.field_datatype(&p.vertex_type, &p.group_by)?;
        if !is_binnable_datatype(&datatype) {
            return Err(GraphError::InvalidParams {
                verb: "aggregate",
                detail: format!(
                    "`bins` needs a numeric or temporal column; `{}` is `{datatype}` — group over \
                     its values instead",
                    p.group_by
                ),
            });
        }

        let table = quote_ident(&p.vertex_type);
        let field = quote_ident(&p.group_by);
        let agg_expr = agg_expr(p)?;

        let bounds = self
            .exec
            .query_json(&format!(
                "SELECT min({field})::DOUBLE AS lo, max({field})::DOUBLE AS hi FROM {table}"
            ))
            .await?;
        let (Some(lo), Some(hi)) = (scalar_f64(&bounds, "lo"), scalar_f64(&bounds, "hi")) else {
            // Empty column → no range, so no bins.
            return Ok(AggregateResult {
                rows: Vec::new(),
                edges: Vec::new(),
            });
        };

        let width = (hi - lo) / f64::from(bins);
        let edges = (0..=bins)
            .map(|i| f64::from(i).mul_add(width, lo))
            .collect();
        let mut values = vec![0.0_f64; bins as usize];

        if width > 0.0 {
            let rows = self
                .exec
                .query_json(&format!(
                    "SELECT least({bins} - 1, floor(({field}::DOUBLE - {lo}) / {width}))::BIGINT \
                     AS grp, {agg_expr}::DOUBLE AS val FROM {table} WHERE {field} IS NOT NULL \
                     GROUP BY grp ORDER BY grp"
                ))
                .await?;
            for r in &rows {
                let (Some(bin), Some(val)) = (
                    r.get("grp").and_then(Value::as_u64),
                    r.get("val").and_then(Value::as_f64),
                ) else {
                    continue;
                };
                if let Some(slot) = usize::try_from(bin).ok().and_then(|i| values.get_mut(i)) {
                    *slot = val;
                }
            }
        } else {
            // Every value equal → one bin holds the whole column.
            let rows = self
                .exec
                .query_json(&format!(
                    "SELECT {agg_expr}::DOUBLE AS val FROM {table} WHERE {field} IS NOT NULL"
                ))
                .await?;
            if let (Some(val), Some(slot)) = (scalar_f64(&rows, "val"), values.first_mut()) {
                *slot = val;
            }
        }

        Ok(AggregateResult {
            rows: values
                .into_iter()
                .enumerate()
                .map(|(i, value)| AggregateRow {
                    group: Value::from(i),
                    value,
                })
                .collect(),
            edges,
        })
    }

    // ── Escape hatch ──────────────────────────────────────────────────────

    async fn execute_sql(&self, p: &ExecuteSqlParams) -> Result<ExecuteSqlResult> {
        let cap = u64::from(p.row_cap);
        // Wrap so the row cap is enforced regardless of the user's own LIMIT;
        // fetch one extra row to detect truncation. Rows are the only bound:
        // neither host can interrupt a running statement, so a query that is
        // slow rather than large runs to completion.
        let wrapped = format!("SELECT * FROM ({}) AS _q LIMIT {}", p.sql, cap + 1);
        let QueryResult { columns, mut rows } = self.exec.query_columns(&wrapped).await?;
        let truncated = u64::try_from(rows.len()).unwrap_or(u64::MAX) > cap;
        rows.truncate(usize::try_from(cap).unwrap_or(usize::MAX));
        Ok(ExecuteSqlResult {
            columns: columns
                .into_iter()
                .map(|(name, duckdb_type)| ColumnDescriptor { name, duckdb_type })
                .collect(),
            rows,
            truncated,
        })
    }

    // ── Read verbs ────────────────────────────────────────────────────────

    /// Rows of one vertex type: a predicate, an order, a limit.
    ///
    /// The general bounded read, and the two verbs it replaced are two of its
    /// shapes — `get_vertex` was `where: "subject = '…'"`, `top_k` was an
    /// `order_by` and a `limit`. Neither was a different question.
    ///
    /// The projection is the identity plus the user-facing columns: `subject`
    /// rides along because a filter that must change the picture answers with
    /// ids — the canvas masks its resident tiles with them — and the writer's
    /// `dense_id`/`x`/`y`/`cluster_id` do not,
    /// because those are what a tile carries and this is the algebra.
    async fn read(&self, p: &ReadParams) -> Result<ReadResult> {
        let props: Vec<&str> = self
            .manifest
            .lookup_vertex(&p.vertex_type)?
            .property_groups
            .iter()
            .flat_map(|g| g.properties.iter())
            .map(|prop| prop.name.as_str())
            .collect();

        let mut cols: Vec<String> = Vec::with_capacity(props.len());
        if props.contains(&"subject") {
            cols.push(quote_ident("subject"));
        }
        cols.extend(
            props
                .iter()
                .filter(|name| !RESERVED_VERTEX_COLUMNS.contains(name))
                .map(|name| quote_ident(name)),
        );
        if cols.is_empty() {
            // Nothing but writer columns: there is nothing here to read.
            return Ok(ReadResult { rows: Vec::new() });
        }

        let mut sql = format!(
            "SELECT {} FROM {}",
            cols.join(", "),
            quote_ident(&p.vertex_type)
        );
        if let Some(predicate) = p.r#where.as_deref() {
            let _ = write!(sql, " WHERE {predicate}");
        }
        if let Some(order_by) = p.order_by.as_deref() {
            let dir = if p.descending { "DESC" } else { "ASC" };
            let _ = write!(sql, " ORDER BY {} {dir}", quote_ident(order_by));
        }
        let _ = write!(sql, " LIMIT {}", p.limit);

        Ok(ReadResult {
            rows: self.exec.query_json(&sql).await?,
        })
    }

    /// The neighbourhood of a set of vertices, either outward or among itself.
    async fn expand(&self, p: &ExpandParams) -> Result<ExpandResult> {
        let seeds = self.seed_vertices(&p.from).await;
        let Some(edge_relation) = self.edge_relation_sql(&p.edge_types) else {
            // No edge type matches → the set reaches nothing.
            return Ok(ExpandResult {
                vertices: seeds,
                edges: Vec::new(),
            });
        };
        match p.mode {
            ExpandMode::Into => self.expand_into(p, &edge_relation, seeds).await,
            ExpandMode::All => self.expand_all(p, &edge_relation, seeds).await,
        }
    }

    /// `Expand(Into)`: the subgraph induced on `from` — every edge whose two
    /// ends are both in the set, and no vertex the call did not name.
    ///
    /// One pass, no recursion: an induced subgraph has no frontier to advance,
    /// so `depth` means nothing here. See [`ExpandMode::Into`] for why this is
    /// not the fast path it was argued to be.
    async fn expand_into(
        &self,
        p: &ExpandParams,
        edge_relation: &str,
        seeds: Vec<GraphVertex>,
    ) -> Result<ExpandResult> {
        let Some(set) = iri_list(&p.from) else {
            return Ok(ExpandResult {
                vertices: seeds,
                edges: Vec::new(),
            });
        };
        let sql = format!(
            "SELECT src, dst, predicate FROM ({edge_relation}) AS _e \
             WHERE src IN ({set}) AND dst IN ({set}) LIMIT {}",
            p.limit
        );
        Ok(ExpandResult {
            edges: self
                .exec
                .query_json(&sql)
                .await?
                .iter()
                .filter_map(row_to_edge)
                .collect(),
            vertices: seeds,
        })
    }

    /// `Expand(All)`: a breadth-first walk out of the set, bounded by `depth`
    /// and by an outer `LIMIT` so a hub cannot explode the result.
    async fn expand_all(
        &self,
        p: &ExpandParams,
        edge_relation: &str,
        seeds: Vec<GraphVertex>,
    ) -> Result<ExpandResult> {
        let Some(roots) = iri_list(&p.from) else {
            return Ok(ExpandResult {
                vertices: seeds,
                edges: Vec::new(),
            });
        };
        let depth = u32::from(p.depth.max(1));
        let sql = format!(
            "WITH RECURSIVE edge_rel AS ({edge_relation}), \
             walk(src, dst, dst_type, predicate, hop) AS ( \
                 SELECT src, dst, dst_type, predicate, 1 FROM edge_rel WHERE src IN ({roots}) \
                 UNION ALL \
                 SELECT e.src, e.dst, e.dst_type, e.predicate, w.hop + 1 \
                 FROM edge_rel e JOIN walk w ON e.src = w.dst WHERE w.hop < {depth} \
             ) \
             SELECT src, dst, dst_type, predicate, min(hop) AS hop \
             FROM walk GROUP BY src, dst, dst_type, predicate LIMIT {}",
            p.limit
        );

        let rows = self.exec.query_json(&sql).await?;
        let mut edges = Vec::with_capacity(rows.len());
        let mut vertices = seeds;
        let mut seen: std::collections::HashSet<String> =
            vertices.iter().map(|v| v.iri.clone()).collect();
        for r in &rows {
            let Some(edge) = row_to_edge(r) else { continue };
            let reached = edge.target.clone();
            edges.push(edge);
            if seen.insert(reached.clone()) {
                vertices.push(GraphVertex {
                    iri: reached.clone(),
                    label: reached,
                    vertex_type: r
                        .get("dst_type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    hop: r
                        .get("hop")
                        .and_then(Value::as_u64)
                        .and_then(|h| u8::try_from(h).ok())
                        .unwrap_or(u8::MAX),
                });
            }
        }
        Ok(ExpandResult { vertices, edges })
    }

    async fn path(&self, p: &PathParams) -> Result<PathResult> {
        let empty = PathResult {
            vertices: Vec::new(),
            edges: Vec::new(),
        };
        let Some(edge_relation) = self.edge_relation_sql(&[]) else {
            return Ok(empty);
        };

        let source = sql_str_lit(&p.source_iri);
        let target = sql_str_lit(&p.target_iri);
        let max_hops = u32::from(p.max_hops.max(1));
        let src_type = sql_str_lit(&self.vertex_type_of(&p.source_iri).await.unwrap_or_default());

        // BFS accumulating the node / predicate / type lists, pruning cycles via
        // `list_contains`; the shortest path to `target` is the min-depth row.
        let sql = format!(
            "WITH RECURSIVE edge_rel AS ({edge_relation}), \
             walk(node, depth, nodes, preds, types) AS ( \
                 SELECT {source}, 0, [{source}], []::VARCHAR[], [{src_type}] \
                 UNION ALL \
                 SELECT e.dst, w.depth + 1, list_append(w.nodes, e.dst), \
                        list_append(w.preds, e.predicate), list_append(w.types, e.dst_type) \
                 FROM edge_rel e JOIN walk w ON e.src = w.node \
                 WHERE w.depth < {max_hops} AND NOT list_contains(w.nodes, e.dst) \
             ) \
             SELECT nodes, preds, types FROM walk WHERE node = {target} ORDER BY depth LIMIT 1"
        );

        let rows = self.exec.query_json(&sql).await?;
        let Some(row) = rows.first() else {
            return Ok(empty);
        };
        let nodes = json_str_array(row, "nodes");
        let preds = json_str_array(row, "preds");
        let types = json_str_array(row, "types");

        let vertices = nodes
            .iter()
            .enumerate()
            .map(|(i, iri)| GraphVertex {
                iri: iri.clone(),
                label: iri.clone(),
                vertex_type: types.get(i).cloned().unwrap_or_default(),
                hop: u8::try_from(i).unwrap_or(u8::MAX),
            })
            .collect();
        let edges = nodes
            .windows(2)
            .enumerate()
            .map(|(i, pair)| GraphEdge {
                source: pair[0].clone(),
                target: pair[1].clone(),
                predicate: preds.get(i).cloned().unwrap_or_default(),
            })
            .collect();
        Ok(PathResult { vertices, edges })
    }

    /// The vertices the call named, at hop 0, with their types resolved. The
    /// seed of an expansion, and the whole answer of an `into` one.
    async fn seed_vertices(&self, iris: &[String]) -> Vec<GraphVertex> {
        let types = self.vertex_types_of(iris).await;
        iris.iter()
            .map(|iri| GraphVertex {
                iri: iri.clone(),
                label: iri.clone(),
                vertex_type: types.get(iri).cloned().unwrap_or_default(),
                hop: 0,
            })
            .collect()
    }

    /// Resolve which vertex type holds each IRI, in ONE query: each vertex
    /// table probed for the whole set at once, rather than a round trip per
    /// vertex per table. Subjects are unique across the graph, so the first
    /// row wins.
    async fn vertex_types_of(&self, iris: &[String]) -> std::collections::HashMap<String, String> {
        let Some(set) = iri_list(iris) else {
            return std::collections::HashMap::new();
        };
        let probe = self
            .manifest
            .vertices()
            .iter()
            .map(|v| {
                format!(
                    "SELECT subject AS iri, '{}' AS t FROM {} WHERE subject IN ({set})",
                    v.vertex_type,
                    quote_ident(&v.vertex_type)
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        if probe.is_empty() {
            return std::collections::HashMap::new();
        }
        let Ok(rows) = self.exec.query_json(&probe).await else {
            return std::collections::HashMap::new();
        };
        rows.iter()
            .filter_map(|r| {
                Some((
                    r.get("iri").and_then(Value::as_str)?.to_string(),
                    r.get("t").and_then(Value::as_str)?.to_string(),
                ))
            })
            .collect()
    }

    /// One IRI's vertex type — the single-vertex case of [`Self::vertex_types_of`].
    async fn vertex_type_of(&self, iri: &str) -> Option<String> {
        self.vertex_types_of(std::slice::from_ref(&iri.to_string()))
            .await
            .remove(iri)
    }

    /// The resolved-subject edge relation: every edge table joined to its src
    /// and dst vertex tables so `(src, dst)` are IRIs, not dense ids. `None`
    /// when no edge table survives the `edge_types` filter. Built once and used
    /// as the recursive-CTE base for an expansion.
    fn edge_relation_sql(&self, edge_types: &[String]) -> Option<String> {
        let parts: Vec<String> = self
            .manifest
            .edges()
            .iter()
            .filter(|e| edge_types.is_empty() || edge_types.iter().any(|t| t == &e.edge_type))
            .map(|e| {
                format!(
                    "SELECT s.subject AS src, d.subject AS dst, '{et}' AS predicate, \
                     '{dt}' AS dst_type \
                     FROM {tbl} e \
                     JOIN {src} s ON e.src_dense = s.dense_id \
                     JOIN {dst} d ON e.dst_dense = d.dense_id",
                    et = e.edge_type,
                    dt = e.dst_type,
                    tbl = quote_ident(&edge_table_name(e)),
                    src = quote_ident(&e.src_type),
                    dst = quote_ident(&e.dst_type),
                )
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" UNION ALL "))
        }
    }

    /// The `GraphAr` `data_type` of a vertex field, or `UnknownEntity` if the
    /// field is not declared on that vertex type.
    fn field_datatype(&self, vertex_type: &str, field: &str) -> Result<String> {
        self.manifest
            .lookup_vertex(vertex_type)?
            .property_groups
            .iter()
            .flat_map(|g| g.properties.iter())
            .find(|prop| prop.name == field)
            .map(|prop| prop.data_type.clone())
            .ok_or_else(|| GraphError::UnknownEntity {
                kind: "field",
                name: field.to_string(),
            })
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    /// Cheap row count — `DuckDB` answers from the Parquet footer metadata
    /// (O(1), no full scan) for `read_parquet`-backed views.
    async fn count_rows(&self, table: &str) -> Result<u64> {
        let sql = format!("SELECT count(*) AS n FROM {}", quote_ident(table));
        scalar_u64(&self.exec.query_json(&sql).await?, "n")
            .ok_or_else(|| GraphError::Execution(format!("count(*) on `{table}` returned no row")))
    }
}

/// Quote a `DuckDB` identifier, doubling embedded quotes.
///
/// `pub` because `fossil-mcp` builds `CREATE VIEW` over the same datasets and
/// had a byte-identical private copy of this. Two spellings of SQL quoting is
/// not a cosmetic duplication — the siblings below already disagree on purpose
/// (`sql_str_lit` wraps in quotes, `fossil-mcp`'s `escape_lit` does not), so
/// a reader with two `quote_ident`s in front of them has no way to tell which
/// difference is deliberate.
#[must_use]
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A `DuckDB` single-quoted string literal, doubling embedded quotes.
fn sql_str_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Best-effort `DuckDB` type name from a JSON value kind — the default
/// [`DuckExecutor::query_columns`] fallback when real column types aren't
/// available (the native runtime overrides with exact types).
const fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "NULL",
        Value::Bool(_) => "BOOLEAN",
        Value::Number(_) => "DOUBLE",
        Value::String(_) => "VARCHAR",
        Value::Array(_) => "LIST",
        Value::Object(_) => "STRUCT",
    }
}

/// A comma-separated `IN` list of quoted IRIs, or `None` for an empty set —
/// `IN ()` is not valid SQL and an empty set has no answer to look up.
fn iri_list(iris: &[String]) -> Option<String> {
    (!iris.is_empty()).then(|| {
        iris.iter()
            .map(|iri| sql_str_lit(iri))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// One `(src, dst, predicate)` row to a [`GraphEdge`], dropping rows missing a
/// column the caller could do nothing with.
fn row_to_edge(row: &Value) -> Option<GraphEdge> {
    Some(GraphEdge {
        source: row.get("src").and_then(Value::as_str)?.to_string(),
        target: row.get("dst").and_then(Value::as_str)?.to_string(),
        predicate: row
            .get("predicate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Extract a row column that is a JSON array of strings (a `DuckDB` `VARCHAR[]`).
fn json_str_array(row: &Value, key: &str) -> Vec<String> {
    row.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|e| GraphError::Execution(format!("result serialisation failed: {e}")))
}

fn scalar_u64(rows: &[Value], key: &str) -> Option<u64> {
    rows.first()
        .and_then(|r| r.get(key))
        .and_then(Value::as_u64)
}

fn scalar_f64(rows: &[Value], key: &str) -> Option<f64> {
    rows.first()
        .and_then(|r| r.get(key))
        .and_then(Value::as_f64)
}

/// The `DuckDB` aggregate expression, checking that a measured aggregation was
/// handed the column it measures.
fn agg_expr(p: &AggregateParams) -> Result<String> {
    Ok(match p.agg {
        Aggregation::Count => "count(*)".to_string(),
        measured => {
            let measure = p
                .measure
                .as_deref()
                .ok_or_else(|| GraphError::InvalidParams {
                    verb: "aggregate",
                    detail: format!("agg `{}` requires a `measure` column", agg_fn(measured)),
                })?;
            format!("{}({})", agg_fn(measured), quote_ident(measure))
        }
    })
}

/// The `DuckDB` aggregate function name for a measured [`Aggregation`].
/// `Count` is handled separately (it takes no measure column).
const fn agg_fn(a: Aggregation) -> &'static str {
    match a {
        Aggregation::Count => "count",
        Aggregation::Sum => "sum",
        Aggregation::Avg => "avg",
        Aggregation::Min => "min",
        Aggregation::Max => "max",
    }
}

/// Whether a `GraphAr` `data_type` spelling has ranges to bin over. A string
/// or a boolean does not: the only grouping it admits is by value.
fn is_binnable_datatype(datatype: &str) -> bool {
    is_numeric_datatype(datatype) || matches!(datatype, "date" | "timestamp" | "time")
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Infer a chart-axis role from a field's name, `GraphAr` `data_type`, and
/// cardinality. Authoritative port of keasy's former `lib/graph-schema.ts::
/// inferRole` (this surface is now the single source):
/// an id/uri/iri name wins; then numeric→measure; then bool/temporal→dimension;
/// then a high-cardinality column (distinct > 200 or > 80% unique) is an
/// identifier; else dimension. Operating on `GraphAr` spellings natively fixes
/// keasy's latent `int64`-misclassified-as-dimension bug by construction.
fn infer_role(name: &str, datatype: &str, distinct: Option<u64>, count: Option<u64>) -> FieldRole {
    if is_identifier_name(name) {
        return FieldRole::Identifier;
    }
    if is_numeric_datatype(datatype) {
        return FieldRole::Measure;
    }
    match datatype {
        "bool" | "boolean" | "date" | "timestamp" | "time" => return FieldRole::Dimension,
        _ => {}
    }
    if let (Some(d), Some(c)) = (distinct, count) {
        #[allow(clippy::cast_precision_loss)]
        if c > 0 && (d > 200 || (d as f64) / (c as f64) > 0.8) {
            return FieldRole::Identifier;
        }
    }
    FieldRole::Dimension
}

/// keasy `IDENTIFIER_PATTERN` = `/(?:^|[_.])(id|uri|iri)(?:$|[_.])/i`, ported as
/// a token-boundary scan (no regex dep — WASM-size-conscious). A token counts
/// only at a `_`/`.`/string boundary, case-insensitive.
fn is_identifier_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    for tok in ["id", "uri", "iri"] {
        let mut from = 0;
        while let Some(off) = lower[from..].find(tok) {
            let i = from + off;
            let end = i + tok.len();
            let before_ok = i == 0 || matches!(bytes[i - 1], b'_' | b'.');
            let after_ok = end == bytes.len() || matches!(bytes[end], b'_' | b'.');
            if before_ok && after_ok {
                return true;
            }
            from = i + 1;
        }
    }
    false
}

/// `GraphAr` numeric datatype spellings (the writer's int/float family). The
/// numeric arm of role inference and of what can be binned.
fn is_numeric_datatype(datatype: &str) -> bool {
    matches!(
        datatype,
        "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "float"
            | "double"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{GRAPH_INFO_PATH, ManifestSource};
    use fossil_sinks::manifest::{
        Container, DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
    };
    use std::collections::HashMap;

    struct MapSource(HashMap<String, Vec<u8>>);
    impl ManifestSource for MapSource {
        fn fetch(&self, rel_path: &str) -> Result<Vec<u8>> {
            self.0
                .get(rel_path)
                .cloned()
                .ok_or_else(|| GraphError::InvalidManifest(format!("missing {rel_path}")))
        }
    }

    fn person_info() -> VertexInfo {
        let mut info = VertexInfo::new(
            "Person",
            3,
            DEFAULT_CHUNK_SIZE,
            "vertex/Person/",
            vec![PropertyGroup {
                file_type: "parquet".into(),
                properties: vec![
                    Property {
                        name: "dense_id".into(),
                        data_type: "uint32".into(),
                        // The address is never the primary; this fixture carries
                        // no identity column, so it carries no primary.
                        is_primary: false,
                        is_nullable: Some(false),
                    },
                    Property {
                        name: "age".into(),
                        data_type: "int64".into(),
                        is_primary: false,
                        is_nullable: None,
                    },
                    Property {
                        name: "name".into(),
                        data_type: "string".into(),
                        is_primary: false,
                        is_nullable: None,
                    },
                ],
            }],
        );
        info.iri = "http://example.org/Person".into();
        info
    }

    fn fixture() -> Manifest {
        let person = person_info();
        let edge = EdgeInfo {
            src_type: "Person".into(),
            edge_type: "knows".into(),
            iri: "http://example.org/knows".into(),
            dst_type: "Person".into(),
            edge_count: 2,
            chunk_size: DEFAULT_CHUNK_SIZE,
            src_chunk_size: DEFAULT_CHUNK_SIZE,
            dst_chunk_size: DEFAULT_CHUNK_SIZE,
            directed: true,
            prefix: "edge/Person_knows_Person/".into(),
            adj_lists: vec![],
            property_groups: vec![],
            version: "gar/v1".into(),
        };
        let graph = GraphInfo::new(
            "graph",
            "",
            Container::RowGroups,
            vec!["vertex/Person.vertex.yml".into()],
            vec!["edge/Person_knows_Person/Person_knows_Person.edge.yml".into()],
        );
        let mut map = HashMap::new();
        map.insert(
            GRAPH_INFO_PATH.into(),
            graph.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "vertex/Person.vertex.yml".into(),
            person.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "edge/Person_knows_Person/Person_knows_Person.edge.yml".into(),
            edge.to_yaml().unwrap().into_bytes(),
        );
        Manifest::load(&MapSource(map)).expect("load manifest")
    }

    /// Canned executor: matches the verb SQL shapes by substring so the verb
    /// wiring is testable without a real `DuckDB` (SQL correctness is covered by
    /// the fossil-layout integration test).
    /// Drive a future to completion synchronously. Our test executors never
    /// suspend (they return ready values), so a noop-waker poll loop returns on
    /// the first poll — no runtime dependency needed.
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        use std::task::{Context as TaskContext, Poll};
        let mut fut = std::pin::pin!(fut);
        let mut cx = TaskContext::from_waker(std::task::Waker::noop());
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    /// Block on [`dispatch`] — the tests are sync.
    fn run<E: DuckExecutor>(op: &Operation, m: &Manifest, exec: &E) -> Result<Value> {
        block_on(dispatch(op, m, exec))
    }

    struct FakeExecutor;
    impl DuckExecutor for FakeExecutor {
        async fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
            // The batched stats query carries both spellings, so it is matched
            // first: `count(*) AS n, count(DISTINCT …) AS d0, …`.
            if sql.contains("count(DISTINCT") {
                return Ok(vec![serde_json::json!({ "n": 3, "d0": 3, "d1": 2 })]);
            }
            if sql.contains("count(*)") {
                return Ok(vec![serde_json::json!({ "n": 3 })]);
            }
            // samples query
            Ok(vec![
                serde_json::json!({ "sample": "30" }),
                serde_json::json!({ "sample": "41" }),
            ])
        }
    }

    /// Executor backed by a closure — canned responses keyed on the SQL, so a
    /// verb's wiring (param → SQL → result shaping) is tested without real `DuckDB`
    /// (SQL correctness is the fossil-layout integration test's job).
    struct FnExecutor<F: Fn(&str) -> Vec<Value>>(F);
    impl<F: Fn(&str) -> Vec<Value>> DuckExecutor for FnExecutor<F> {
        async fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
            Ok(self.0(sql))
        }
    }

    #[test]
    fn aggregate_count_groups_and_maps_rows() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            assert!(sql.contains("count(*)"), "count agg, no measure: {sql}");
            assert!(sql.contains("GROUP BY"));
            vec![
                serde_json::json!({ "grp": "a", "val": 2.0 }),
                serde_json::json!({ "grp": "b", "val": 1.0 }),
            ]
        });
        let v = run(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "name".into(),
                agg: Aggregation::Count,
                measure: None,
                bins: None,
                limit: 1000,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: AggregateResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0].group, serde_json::json!("a"));
        assert!((r.rows[0].value - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_sum_requires_measure() {
        let m = fixture();
        let err = run(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "name".into(),
                agg: Aggregation::Sum,
                measure: None,
                bins: None,
                limit: 1000,
            }),
            &m,
            &FakeExecutor,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphError::InvalidParams {
                verb: "aggregate",
                ..
            }
        ));
    }

    #[test]
    fn read_projects_identity_and_user_columns_ordered_and_capped() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            assert!(sql.contains("ORDER BY") && sql.contains("DESC") && sql.contains("LIMIT 5"));
            // The writer's columns are the tiles' business, not the algebra's.
            assert!(!sql.contains("dense_id"), "reserved cols filtered: {sql}");
            vec![serde_json::json!({ "name": "x", "age": 9 })]
        });
        let v = run(
            &Operation::Read(ReadParams {
                vertex_type: "Person".into(),
                r#where: None,
                order_by: Some("age".into()),
                descending: true,
                limit: 5,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: ReadResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.rows.len(), 1);
    }

    #[test]
    fn read_by_subject_is_what_get_vertex_was() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            assert!(sql.contains("WHERE subject = 'urn:a'"), "predicate: {sql}");
            vec![serde_json::json!({ "age": 30, "name": "Alice" })]
        });
        let v = run(
            &Operation::Read(ReadParams {
                vertex_type: "Person".into(),
                r#where: Some("subject = 'urn:a'".into()),
                order_by: None,
                descending: false,
                limit: 1,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: ReadResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].get("name").and_then(Value::as_str), Some("Alice"));
    }

    #[test]
    fn read_that_matches_nothing_is_no_rows() {
        let m = fixture();
        let exec = FnExecutor(|_sql: &str| Vec::new());
        let v = run(
            &Operation::Read(ReadParams {
                vertex_type: "Person".into(),
                r#where: Some("subject = 'urn:nope'".into()),
                order_by: None,
                descending: false,
                limit: 1,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: ReadResult = serde_json::from_value(v).unwrap();
        assert!(r.rows.is_empty());
    }

    #[test]
    fn aggregate_over_ranges_is_dense_and_carries_its_edges() {
        let m = fixture();
        // age is int64 → binnable. First query = bounds, second = the grouping.
        let exec = FnExecutor(|sql: &str| {
            if sql.contains("min(") {
                vec![serde_json::json!({ "lo": 0.0, "hi": 4.0 })]
            } else {
                assert!(sql.contains("GROUP BY grp"), "still one GROUP BY: {sql}");
                vec![
                    serde_json::json!({ "grp": 0, "val": 3.0 }),
                    serde_json::json!({ "grp": 3, "val": 1.0 }),
                ]
            }
        });
        let v = run(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "age".into(),
                agg: Aggregation::Count,
                measure: None,
                bins: Some(4),
                limit: 1000,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: AggregateResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.edges, vec![0.0, 1.0, 2.0, 3.0, 4.0]); // bins + 1
        // Dense: the two bins nothing landed in are rows, not absences.
        let values: Vec<f64> = r.rows.iter().map(|row| row.value).collect();
        assert_eq!(values, vec![3.0, 0.0, 0.0, 1.0]);
        assert_eq!(r.rows[3].group, serde_json::json!(3));
    }

    #[test]
    fn aggregate_over_values_draws_no_axis() {
        // The two arms are told apart by `edges`, so a grouping over values
        // must not carry any: there is no range for a bin edge to bound.
        let m = fixture();
        let exec = FnExecutor(|_: &str| vec![serde_json::json!({ "grp": "a", "val": 1.0 })]);
        let v = run(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "name".into(),
                agg: Aggregation::Count,
                measure: None,
                bins: None,
                limit: 1000,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: AggregateResult = serde_json::from_value(v).unwrap();
        assert!(r.edges.is_empty());
    }

    #[test]
    fn aggregate_refuses_to_bin_a_categorical_column() {
        // `name` is a string: it has no ranges. Grouping it by value is the
        // answer, and it is one parameter away — so say so rather than quietly
        // return an answer of a different shape.
        let m = fixture();
        let err = run(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "name".into(),
                agg: Aggregation::Count,
                measure: None,
                bins: Some(10),
                limit: 1000,
            }),
            &m,
            &FakeExecutor,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphError::InvalidParams {
                verb: "aggregate",
                ..
            }
        ));
    }

    /// A [`SchemaParams`] naming nothing, one type, or one type + one field.
    fn schema_params(vertex_type: Option<&str>, field: Option<&str>) -> SchemaParams {
        SchemaParams {
            vertex_type: vertex_type.map(ToString::to_string),
            field: field.map(ToString::to_string),
        }
    }

    #[test]
    fn bare_schema_lists_both_halves_and_queries_no_field() {
        let m = fixture();
        // The property the collapse had to keep: naming no type must not cost a
        // query per column. Recording the SQL is the only way to assert it.
        let seen = std::cell::RefCell::new(Vec::<String>::new());
        let exec = FnExecutor(|sql: &str| {
            seen.borrow_mut().push(sql.to_string());
            vec![serde_json::json!({ "n": 3 })]
        });

        let v = run(&Operation::Schema(schema_params(None, None)), &m, &exec).unwrap();
        let r: SchemaResult = serde_json::from_value(v).unwrap();

        assert_eq!(r.vertices.len(), 1);
        let person = &r.vertices[0];
        assert_eq!(person.name, "Person");
        assert_eq!(person.iri, "http://example.org/Person");
        assert_eq!(person.count, 3);
        // dense_id hidden; user fields surfaced.
        assert_eq!(person.fields, vec!["age", "name"]);

        assert_eq!(r.edges.len(), 1);
        let knows = &r.edges[0];
        assert_eq!(knows.table_name, "Person_knows_Person");
        assert_eq!(knows.iri, "http://example.org/knows");
        assert_eq!(knows.count, 3);

        assert!(r.fields.is_empty(), "no vertex_type named → no field stats");
        assert!(
            seen.borrow().iter().all(|sql| !sql.contains("DISTINCT")),
            "a bare schema call must not run a per-field query: {:?}",
            seen.borrow(),
        );
    }

    #[test]
    fn schema_with_a_vertex_type_batches_fields_and_roles() {
        let m = fixture();
        // ONE query for every field: count(*) + a count(DISTINCT) per column.
        let exec = FnExecutor(|sql: &str| {
            if sql.contains("count(DISTINCT") {
                assert_eq!(sql.matches("count(DISTINCT").count(), 2, "one pass: {sql}");
                return vec![serde_json::json!({ "n": 3, "d0": 3, "d1": 2 })];
            }
            vec![serde_json::json!({ "n": 3 })]
        });
        let v = run(
            &Operation::Schema(schema_params(Some("Person"), None)),
            &m,
            &exec,
        )
        .unwrap();
        let r: SchemaResult = serde_json::from_value(v).unwrap();

        // dense_id filtered; age + name surfaced, in manifest order.
        assert_eq!(r.fields.len(), 2);
        assert_eq!(r.fields[0].name, "age");
        assert_eq!(r.fields[0].role, FieldRole::Measure); // int64
        assert_eq!(r.fields[0].distinct, 3);
        assert_eq!(r.fields[1].name, "name");
        assert_eq!(r.fields[1].role, FieldRole::Dimension); // string, 2/3 < 0.8
        assert_eq!(r.fields[1].distinct, 2);
        // Samples are a second query, so a per-type call does not pay for them.
        assert!(r.fields.iter().all(|f| f.samples.is_empty()));
    }

    #[test]
    fn schema_with_a_field_narrows_to_it_and_samples_it() {
        let m = fixture();
        let v = run(
            &Operation::Schema(schema_params(Some("Person"), Some("age"))),
            &m,
            &FakeExecutor,
        )
        .unwrap();
        let r: SchemaResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.fields.len(), 1, "narrowed to the named field");
        let age = &r.fields[0];
        assert_eq!(age.datatype, "int64");
        assert_eq!(age.role, FieldRole::Measure);
        assert_eq!(age.distinct, 3);
        assert_eq!(age.samples, vec!["30", "41"]);
        // The listings still come back — naming a field narrows the stats, not
        // the answer.
        assert_eq!(r.vertices.len(), 1);
    }

    #[test]
    fn infer_role_full_heuristic() {
        // id/uri/iri name wins, case-insensitive, at token boundaries.
        assert_eq!(
            infer_role("user_id", "string", None, None),
            FieldRole::Identifier
        );
        assert_eq!(
            infer_role("IRI", "string", None, None),
            FieldRole::Identifier
        );
        assert_eq!(
            infer_role("home.uri", "string", None, None),
            FieldRole::Identifier
        );
        // "candid" contains "id" but not at a boundary → not an identifier.
        assert_eq!(
            infer_role("candidate", "string", Some(1), Some(10)),
            FieldRole::Dimension
        );
        // numeric → measure (GraphAr spellings, incl. the int64 bug-fix case).
        assert_eq!(infer_role("age", "int64", None, None), FieldRole::Measure);
        assert_eq!(
            infer_role("score", "double", None, None),
            FieldRole::Measure
        );
        // bool / temporal → dimension.
        assert_eq!(
            infer_role("active", "bool", None, None),
            FieldRole::Dimension
        );
        assert_eq!(infer_role("born", "date", None, None), FieldRole::Dimension);
        // high cardinality → identifier; low → dimension.
        assert_eq!(
            infer_role("email", "string", Some(95), Some(100)),
            FieldRole::Identifier
        );
        assert_eq!(
            infer_role("dept", "string", Some(3), Some(100)),
            FieldRole::Dimension
        );
        assert_eq!(
            infer_role("huge", "string", Some(201), Some(100_000)),
            FieldRole::Identifier
        );
    }

    #[test]
    fn expand_all_walks_out_of_the_named_set() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            if sql.contains("WITH RECURSIVE") {
                assert!(
                    sql.contains("WHERE src IN ('urn:a')"),
                    "seeded by set: {sql}"
                );
                // depth-1 out-neighbours of the origin.
                vec![serde_json::json!({
                    "src": "urn:a",
                    "dst": "urn:b",
                    "dst_type": "Person",
                    "predicate": "knows",
                    "hop": 1,
                })]
            } else {
                // The one probe that resolves every named vertex's type.
                vec![serde_json::json!({ "iri": "urn:a", "t": "Person" })]
            }
        });
        let v = run(
            &Operation::Expand(ExpandParams {
                from: vec!["urn:a".into()],
                mode: ExpandMode::All,
                depth: 1,
                edge_types: Vec::new(),
                limit: 500,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: ExpandResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.edges[0].source, "urn:a");
        assert_eq!(r.edges[0].target, "urn:b");
        assert_eq!(r.edges[0].predicate, "knows");
        // seed (hop 0) + what it reached (hop 1).
        assert_eq!(r.vertices.len(), 2);
        assert_eq!(r.vertices[0].iri, "urn:a");
        assert_eq!(r.vertices[0].hop, 0);
        assert_eq!(r.vertices[0].vertex_type, "Person");
        assert_eq!(r.vertices[1].iri, "urn:b");
        assert_eq!(r.vertices[1].hop, 1);
    }

    #[test]
    fn expand_into_keeps_only_edges_with_both_ends_named() {
        let m = fixture();
        // The induced subgraph has no frontier, so there is no recursion and no
        // vertex the caller did not name.
        let exec = FnExecutor(|sql: &str| {
            if sql.contains("src IN") && sql.contains("dst IN") {
                assert!(!sql.contains("RECURSIVE"), "one pass, no walk: {sql}");
                return vec![serde_json::json!({
                    "src": "urn:a",
                    "dst": "urn:b",
                    "predicate": "knows",
                })];
            }
            vec![
                serde_json::json!({ "iri": "urn:a", "t": "Person" }),
                serde_json::json!({ "iri": "urn:b", "t": "Person" }),
            ]
        });
        let v = run(
            &Operation::Expand(ExpandParams {
                from: vec!["urn:a".into(), "urn:b".into()],
                mode: ExpandMode::Into,
                depth: 3, // ignored: an induced subgraph has no frontier.
                edge_types: Vec::new(),
                limit: 500,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: ExpandResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.vertices.len(), 2, "no vertex beyond the named set");
        assert!(r.vertices.iter().all(|v| v.hop == 0));
    }

    #[test]
    fn path_reconstructs_the_ordered_route() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            if sql.contains("WITH RECURSIVE") {
                vec![serde_json::json!({
                    "nodes": ["urn:a", "urn:b", "urn:c"],
                    "preds": ["knows", "knows"],
                    "types": ["Person", "Person", "Person"],
                })]
            } else {
                vec![serde_json::json!({ "iri": "urn:a", "t": "Person" })]
            }
        });
        let v = run(
            &Operation::Path(PathParams {
                source_iri: "urn:a".into(),
                target_iri: "urn:c".into(),
                max_hops: 5,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: PathResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.vertices.len(), 3);
        assert_eq!(r.vertices[2].iri, "urn:c");
        assert_eq!(r.vertices[2].hop, 2);
        assert_eq!(r.edges.len(), 2);
        assert_eq!(r.edges[1].source, "urn:b");
        assert_eq!(r.edges[1].target, "urn:c");
    }

    #[test]
    fn execute_sql_caps_rows_and_reports_columns() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            // row_cap 2 → fetch 3 (cap + 1) to detect truncation.
            assert!(sql.contains("LIMIT 3"));
            vec![
                serde_json::json!({ "a": 1, "b": "x" }),
                serde_json::json!({ "a": 2, "b": "y" }),
                serde_json::json!({ "a": 3, "b": "z" }),
            ]
        });
        let v = run(
            &Operation::ExecuteSql(ExecuteSqlParams {
                sql: "SELECT * FROM t".into(),
                row_cap: 2,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: ExecuteSqlResult = serde_json::from_value(v).unwrap();
        assert!(r.truncated);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.columns.len(), 2);
    }
}
