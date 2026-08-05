//! Verb execution: the [`DuckExecutor`] seam + per-verb SQL generation.
//!
//! The verb→SQL logic lives here (pure string building, WASM-safe) so every
//! binding reuses it: the native impl (`fossil-runtime`, `duckdb` crate) and
//! the WASM impl (`fossil-wasm`, DuckDB-WASM) differ only in how they satisfy
//! [`DuckExecutor`] — they never re-derive a verb's SQL
//! ([[`feedback_no_duplicate_logic_across_crates`]], [[`feedback_one_idiom_per_concern`]]).
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

use crate::manifest::{Manifest, edge_table_name};
use crate::operations::aggregate::{
    AggregateParams, AggregateResult, AggregateRow, Aggregation, TopKParams, TopKResult,
};
use crate::operations::discovery::{
    FindNeighborsParams, FindNeighborsResult, FindPathParams, FindPathResult, GetVertexParams,
    GetVertexResult, NeighborEdge, NeighborVertex,
};
use crate::operations::sql::{ColumnDescriptor, ExecuteSqlParams, ExecuteSqlResult};
use crate::operations::viewport::{
    MaterializeGraphParams, MaterializeGraphResult, MaterializedEdge, MaterializedVertex,
    ViewportEdge, ViewportMode, ViewportParams, ViewportResult, ViewportVertex,
};
use crate::operations::schema::{
    EdgeTypeSummary, FieldRole, FieldStat, SchemaParams, SchemaResult, VertexTypeSummary,
};
use crate::manifest::RESERVED_VERTEX_COLUMNS;
use crate::{GraphError, Operation, Result};

/// `(column_name, column_type)` descriptors paired with the JSON result rows —
/// the return of [`DuckExecutor::query_columns`].
pub type ColumnedRows = (Vec<(String, String)>, Vec<Value>);

/// The thin `DuckDB` seam (ADR-0003 thin-DB-trait). A binding implements
/// exactly this — run a query, hand back rows as JSON objects — and inherits
/// every verb's SQL for free.
pub trait DuckExecutor {
    /// Run `sql`, returning each result row as a JSON object (column → value).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::Execution`] (carrying the binding's stringified
    /// DB error) when the query fails.
    fn query_json(
        &self,
        sql: &str,
    ) -> impl std::future::Future<Output = Result<Vec<Value>>>;

    /// Run `sql`, returning `(column_name, column_type)` descriptors alongside
    /// the rows. Only [`Operation::ExecuteSql`] needs column types; the default
    /// derives them best-effort from the JSON value kinds, and a binding with
    /// access to real `DuckDB` column types (the native runtime) overrides this.
    ///
    /// # Errors
    ///
    /// As [`DuckExecutor::query_json`].
    fn query_columns(
        &self,
        sql: &str,
    ) -> impl std::future::Future<Output = Result<ColumnedRows>> {
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
            Ok((columns, rows))
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
            Operation::TopK(p) => to_json(&self.top_k(p).await?),
            Operation::FindNeighbors(p) => to_json(&self.find_neighbors(p).await?),
            Operation::FindPath(p) => to_json(&self.find_path(p).await?),
            Operation::GetVertex(p) => to_json(&self.get_vertex(p).await?),
            Operation::Viewport(p) => to_json(&self.viewport(p).await?),
            Operation::MaterializeGraph(p) => to_json(&self.materialize_graph(p).await?),
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
            Some(bins) => self.aggregate_by_range(p, bins.clamp(1, p.limit.max(1))).await,
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
        let edges = (0..=bins).map(|i| f64::from(i).mul_add(width, lo)).collect();
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

    async fn top_k(&self, p: &TopKParams) -> Result<TopKResult> {
        self.manifest.lookup_vertex(&p.vertex_type)?;
        let table = quote_ident(&p.vertex_type);
        let order = quote_ident(&p.order_by);
        let dir = if p.descending { "DESC" } else { "ASC" };
        let sql = format!("SELECT * FROM {table} ORDER BY {order} {dir} LIMIT {}", p.k);
        Ok(TopKResult {
            rows: self.exec.query_json(&sql).await?,
        })
    }

    // ── Escape hatch ──────────────────────────────────────────────────────

    async fn execute_sql(&self, p: &ExecuteSqlParams) -> Result<ExecuteSqlResult> {
        let cap = u64::from(p.row_cap);
        // Wrap so the row cap is enforced regardless of the user's own LIMIT;
        // fetch one extra row to detect truncation. (timeout_ms is enforced by
        // bindings that can set a statement timeout — the native runtime does.)
        let wrapped = format!("SELECT * FROM ({}) AS _q LIMIT {}", p.sql, cap + 1);
        let (columns, mut rows) = self.exec.query_columns(&wrapped).await?;
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

    // ── Viewport verb ─────────────────────────────────────────────────────

    /// The larger-than-RAM read path: what is in this rectangle, at this zoom,
    /// in at most `limit` marks.
    ///
    /// Two modes and they are not variants of each other. Below `lod_threshold`
    /// the reader is looking at everything, and everything is not a picture of
    /// anything — so the answer is one super-node per cluster rather than a
    /// million dots that overplot into a smear.
    async fn viewport(&self, p: &ViewportParams) -> Result<ViewportResult> {
        if p.zoom < p.lod_threshold {
            self.viewport_aggregate(p).await
        } else {
            self.viewport_detail(p).await
        }
    }

    /// One row per visible vertex, plus the edges both of whose endpoints are
    /// visible.
    ///
    /// **Indices are slice-local, not `dense_id`.** A `dense_id` numbers within
    /// one vertex type, so a union of two types repeats every value, and a
    /// `LIMIT` breaks the correspondence with position regardless. The
    /// consumer's next move is a GPU buffer upload, so the numbering it needs is
    /// the position in *this answer* — which `row_number()` assigns inside the
    /// CTE, before the limit, so the edge join speaks the same numbers the
    /// vertex array is built from. `dense_id` and `type_idx` still ride along:
    /// together they identify the vertex, which is what survives the slice.
    async fn viewport_detail(&self, p: &ViewportParams) -> Result<ViewportResult> {
        let Some(scan) = self.viewport_scan_sql(p) else {
            return Ok(ViewportResult {
                mode: ViewportMode::Detail,
                n: 0,
                vertices: Vec::new(),
                edges: Vec::new(),
            });
        };

        // The numbering is over what survives the limit. Numbering first and
        // limiting after would hand out indices into an array never built.
        let cte = format!(
            "WITH vis AS (SELECT *, (row_number() OVER (ORDER BY type_idx, dense_id) - 1)::UINTEGER \
             AS local FROM ({scan}) LIMIT {})",
            p.limit
        );

        let rows = self
            .exec
            .query_json(&format!(
                "{cte} SELECT local, dense_id, x, y, type_idx FROM vis ORDER BY local"
            ))
            .await?;
        let vertices: Vec<ViewportVertex> = rows.iter().map(row_to_viewport_vertex).collect();

        let edges = match self.viewport_edges_sql(p) {
            Some(edge_sql) => self
                .exec
                .query_json(&format!("{cte} {edge_sql}"))
                .await?
                .iter()
                .map(|r| ViewportEdge {
                    src_dense: json_u32(r, "src"),
                    dst_dense: json_u32(r, "dst"),
                })
                .collect(),
            None => Vec::new(),
        };

        // What *matched*, separately from what came back. Without it a truncated
        // view looks exactly like a complete one, which is the one thing a
        // bounded renderer must not do to its reader.
        let matched = self
            .exec
            .query_json(&format!("SELECT count(*) AS n FROM ({scan})"))
            .await?
            .first()
            .map_or(0, |r| json_u32(r, "n"));

        Ok(ViewportResult {
            mode: ViewportMode::Detail,
            n: matched,
            vertices,
            edges,
        })
    }

    /// Zoomed out far enough that individual vertices are not information: one
    /// super-node per `(type_idx, cluster_id)`, at its centroid, weighted by how
    /// many real vertices it stands for.
    ///
    /// Grouped by the pair rather than by `cluster_id` alone, because the writer
    /// partitions each vertex type independently — cluster 0 of `Person` and
    /// cluster 0 of `Org` are different clusters wearing the same number, and
    /// merging them would place a centroid between two communities that share
    /// nothing but an integer.
    ///
    /// The aggregation is a `GROUP BY` in `DuckDB`, so the bytes crossing the
    /// wire are already the answer rather than the input to it — which is the
    /// whole of the larger-than-RAM claim at this zoom.
    async fn viewport_aggregate(&self, p: &ViewportParams) -> Result<ViewportResult> {
        let Some(scan) = self.viewport_scan_sql(p) else {
            return Ok(ViewportResult {
                mode: ViewportMode::Aggregate,
                n: 0,
                vertices: Vec::new(),
                edges: Vec::new(),
            });
        };

        let sql = format!(
            "SELECT (row_number() OVER (ORDER BY type_idx, cluster_id) - 1)::UINTEGER AS local, \
             any_value(dense_id) AS dense_id, avg(x) AS x, avg(y) AS y, type_idx, cluster_id, \
             count(*)::UINTEGER AS cluster_size \
             FROM ({scan}) GROUP BY type_idx, cluster_id LIMIT {}",
            p.limit
        );
        let rows = self.exec.query_json(&sql).await?;
        let vertices: Vec<ViewportVertex> = rows
            .iter()
            .map(|r| ViewportVertex {
                dense_id: json_u32(r, "local"),
                x: json_f32(r, "x"),
                y: json_f32(r, "y"),
                type_idx: u8::try_from(json_u32(r, "type_idx")).unwrap_or(0),
                cluster_size: Some(json_u32(r, "cluster_size")),
                cluster_id: Some(json_u32(r, "cluster_id")),
            })
            .collect();

        let matched = self
            .exec
            .query_json(&format!("SELECT count(*) AS n FROM ({scan})"))
            .await?
            .first()
            .map_or(0, |r| json_u32(r, "n"));

        Ok(ViewportResult {
            mode: ViewportMode::Aggregate,
            n: matched,
            vertices,
            edges: Vec::new(),
        })
    }

    /// The bbox scan, one `SELECT` per requested vertex type, unioned.
    ///
    /// The `WHERE` is a plain range on `x`/`y` on purpose: that is the shape
    /// `DuckDB` pushes into Parquet row-group statistics, and the writer's
    /// Morton sort (`fossil-runtime::layout`) is what makes those statistics
    /// tight enough to skip most of the file. Written any other way — a
    /// function call over the columns, a computed distance — the pushdown is
    /// lost and the verb reads the whole corpus to answer about a window.
    ///
    /// `None` when no vertex type matches, which is a legitimate answer rather
    /// than an error: a caller may filter to a type this graph does not have.
    fn viewport_scan_sql(&self, p: &ViewportParams) -> Option<String> {
        let b = &p.bbox;
        let parts: Vec<String> = self
            .manifest
            .vertices()
            .iter()
            .filter(|v| {
                p.vertex_types.is_empty() || p.vertex_types.iter().any(|t| t == &v.vertex_type)
            })
            .filter_map(|v| {
                self.manifest
                    .vertex_type_idx(&v.vertex_type)
                    .map(|idx| (v, idx))
            })
            .map(|(v, idx)| {
                format!(
                    "SELECT dense_id, x, y, cluster_id, {idx}::UTINYINT AS type_idx FROM {tbl} \
                     WHERE x BETWEEN {xmin} AND {xmax} AND y BETWEEN {ymin} AND {ymax}",
                    tbl = quote_ident(&v.vertex_type),
                    xmin = b.x_min,
                    xmax = b.x_max,
                    ymin = b.y_min,
                    ymax = b.y_max,
                )
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" UNION ALL "))
    }

    /// Edges whose **both** endpoints are in `vis` — an edge with one end off
    /// screen has nowhere to land, so it is dropped rather than drawn to a
    /// vertex the consumer was never sent.
    ///
    /// The join is per edge type and matches on `type_idx` as well as
    /// `dense_id`, because a dense id alone is ambiguous across vertex types.
    fn viewport_edges_sql(&self, p: &ViewportParams) -> Option<String> {
        let parts: Vec<String> = self
            .manifest
            .edges()
            .iter()
            .filter(|e| {
                p.vertex_types.is_empty()
                    || (p.vertex_types.iter().any(|t| t == &e.src_type)
                        && p.vertex_types.iter().any(|t| t == &e.dst_type))
            })
            .filter_map(|e| {
                let src = self.manifest.vertex_type_idx(&e.src_type)?;
                let dst = self.manifest.vertex_type_idx(&e.dst_type)?;
                Some(format!(
                    "SELECT s.local AS src, t.local AS dst FROM {tbl} e \
                     JOIN vis s ON e.src_dense = s.dense_id AND s.type_idx = {src} \
                     JOIN vis t ON e.dst_dense = t.dense_id AND t.type_idx = {dst}",
                    tbl = quote_ident(&edge_table_name(e)),
                ))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" UNION ALL "))
    }

    /// Canvas-ready whole-graph snapshot: vertices with resolved
    /// `subject`/`label`/`type_name` + edges with dense→subject-mapped
    /// endpoints, capped by `limit`. Owns the `GraphAr` column convention so the
    /// host renders without hand-selecting `dense_id`/`src_dense`/`dst_dense`.
    async fn materialize_graph(
        &self,
        p: &MaterializeGraphParams,
    ) -> Result<MaterializeGraphResult> {
        let cap = usize::try_from(p.limit).unwrap_or(usize::MAX);

        // ── Vertices: one SELECT per type; label = first present of
        //    name/label/title, else subject (keasy's display-label rule). ──
        let parts: Vec<String> = self
            .manifest
            .vertices()
            .iter()
            .filter(|v| {
                p.vertex_types.is_empty() || p.vertex_types.iter().any(|t| t == &v.vertex_type)
            })
            .map(|v| {
                let has = |c: &str| {
                    v.property_groups
                        .iter()
                        .flat_map(|g| g.properties.iter())
                        .any(|prop| prop.name == c)
                };
                let label = ["name", "label", "title"]
                    .into_iter()
                    .find(|c| has(c))
                    .map_or_else(|| "subject".to_string(), quote_ident);
                format!(
                    "SELECT subject, {label} AS label, '{ty}' AS type_name FROM {tbl}",
                    ty = v.vertex_type,
                    tbl = quote_ident(&v.vertex_type),
                )
            })
            .collect();

        if parts.is_empty() {
            return Ok(MaterializeGraphResult {
                vertices: Vec::new(),
                edges: Vec::new(),
                truncated: false,
            });
        }

        // Fetch one extra row to detect truncation.
        let vsql = format!(
            "{} LIMIT {}",
            parts.join(" UNION ALL "),
            u64::from(p.limit).saturating_add(1)
        );
        let vrows = self.exec.query_json(&vsql).await?;
        let truncated = vrows.len() > cap;

        let mut vertices = Vec::with_capacity(vrows.len().min(cap));
        let mut included: std::collections::HashSet<String> = std::collections::HashSet::new();
        for r in vrows.into_iter().take(cap) {
            let Some(id) = r.get("subject").and_then(Value::as_str).map(ToString::to_string) else {
                continue;
            };
            let label = r
                .get("label")
                .and_then(Value::as_str)
                .map_or_else(|| id.clone(), ToString::to_string);
            let type_name = r
                .get("type_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            included.insert(id.clone());
            vertices.push(MaterializedVertex { id, label, type_name });
        }

        // ── Edges: dense→subject resolved (reuse edge_relation_sql), then drop
        //    orphans whose endpoint fell outside the materialized vertex set
        //    (mirrors the canvas's defensive orphan drop). ──
        let edges = match self.edge_relation_sql(&[]) {
            None => Vec::new(),
            Some(edge_rel) => {
                let esql = format!(
                    "SELECT src, dst, predicate FROM ({edge_rel}) AS _e LIMIT {}",
                    p.limit
                );
                self.exec
                    .query_json(&esql)
                    .await?
                    .iter()
                    .filter_map(|r| {
                        let src = r.get("src").and_then(Value::as_str)?;
                        let dst = r.get("dst").and_then(Value::as_str)?;
                        if !included.contains(src) || !included.contains(dst) {
                            return None;
                        }
                        Some(MaterializedEdge {
                            source: src.to_string(),
                            target: dst.to_string(),
                            predicate: r
                                .get("predicate")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        })
                    })
                    .collect()
            }
        };

        Ok(MaterializeGraphResult {
            vertices,
            edges,
            truncated,
        })
    }

    // ── Discovery verbs ───────────────────────────────────────────────────

    async fn find_neighbors(&self, p: &FindNeighborsParams) -> Result<FindNeighborsResult> {
        let Some(edge_relation) = self.edge_relation_sql(&p.edge_types) else {
            // No edge types match → the origin has no reachable neighbours.
            return Ok(FindNeighborsResult {
                vertices: self.origin_only(&p.iri).await,
                edges: Vec::new(),
            });
        };

        let root = sql_str_lit(&p.iri);
        let depth = u32::from(p.depth.max(1));
        // Breadth-first walk over the resolved (src, dst, predicate) relation,
        // bounded by depth and an outer LIMIT so a hub can't explode the result.
        let sql = format!(
            "WITH RECURSIVE edge_rel AS ({edge_relation}), \
             walk(src, dst, dst_type, predicate, hop) AS ( \
                 SELECT src, dst, dst_type, predicate, 1 FROM edge_rel WHERE src = {root} \
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
        let mut vertices = self.origin_only(&p.iri).await;
        let mut seen: std::collections::HashSet<String> =
            vertices.iter().map(|v| v.iri.clone()).collect();
        for r in &rows {
            let (Some(src), Some(dst), Some(predicate)) = (
                r.get("src").and_then(Value::as_str),
                r.get("dst").and_then(Value::as_str),
                r.get("predicate").and_then(Value::as_str),
            ) else {
                continue;
            };
            edges.push(NeighborEdge {
                source: src.to_string(),
                target: dst.to_string(),
                predicate: predicate.to_string(),
            });
            if seen.insert(dst.to_string()) {
                vertices.push(NeighborVertex {
                    iri: dst.to_string(),
                    label: dst.to_string(),
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
        Ok(FindNeighborsResult { vertices, edges })
    }

    async fn find_path(&self, p: &FindPathParams) -> Result<FindPathResult> {
        let empty = FindPathResult {
            vertices: Vec::new(),
            edges: Vec::new(),
        };
        let Some(edge_relation) = self.edge_relation_sql(&[]) else {
            return Ok(empty);
        };

        let source = sql_str_lit(&p.source_iri);
        let target = sql_str_lit(&p.target_iri);
        let max_hops = u32::from(p.max_hops.max(1));
        let src_type =
            sql_str_lit(&self.vertex_type_of(&p.source_iri).await.unwrap_or_default());

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
            .map(|(i, iri)| NeighborVertex {
                iri: iri.clone(),
                label: iri.clone(),
                vertex_type: types.get(i).cloned().unwrap_or_default(),
                hop: u8::try_from(i).unwrap_or(u8::MAX),
            })
            .collect();
        let edges = nodes
            .windows(2)
            .enumerate()
            .map(|(i, pair)| NeighborEdge {
                source: pair[0].clone(),
                target: pair[1].clone(),
                predicate: preds.get(i).cloned().unwrap_or_default(),
            })
            .collect();
        Ok(FindPathResult { vertices, edges })
    }

    /// Fetch one vertex's user-facing properties by subject IRI — the
    /// member-safe single-entity read (no `execute_sql` escape hatch). Reserved
    /// (writer) columns are filtered; `vertex` is `null` when no match.
    async fn get_vertex(&self, p: &GetVertexParams) -> Result<GetVertexResult> {
        let cols: Vec<String> = self
            .manifest
            .lookup_vertex(&p.vertex_type)?
            .property_groups
            .iter()
            .flat_map(|g| g.properties.iter())
            .map(|prop| prop.name.clone())
            .filter(|n| !RESERVED_VERTEX_COLUMNS.contains(&n.as_str()))
            .collect();

        if cols.is_empty() {
            return Ok(GetVertexResult {
                vertex: Some(Value::Object(serde_json::Map::new())),
            });
        }

        let select = cols
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {select} FROM {table} WHERE subject = {subj} LIMIT 1",
            table = quote_ident(&p.vertex_type),
            subj = sql_str_lit(&p.subject),
        );
        Ok(GetVertexResult {
            vertex: self.exec.query_json(&sql).await?.into_iter().next(),
        })
    }

    /// The origin vertex alone (hop 0), with its type resolved from whichever
    /// vertex table holds the subject. Used as the seed of a neighbour result.
    async fn origin_only(&self, iri: &str) -> Vec<NeighborVertex> {
        vec![NeighborVertex {
            iri: iri.to_string(),
            label: iri.to_string(),
            vertex_type: self.vertex_type_of(iri).await.unwrap_or_default(),
            hop: 0,
        }]
    }

    /// Resolve which vertex type holds `iri` by probing each type's `subject`
    /// column. Returns the first match (subjects are unique across the graph).
    async fn vertex_type_of(&self, iri: &str) -> Option<String> {
        let lit = sql_str_lit(iri);
        let probe = self
            .manifest
            .vertices()
            .iter()
            .map(|v| {
                format!(
                    "SELECT '{}' AS t FROM {} WHERE subject = {lit}",
                    v.vertex_type,
                    quote_ident(&v.vertex_type)
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        if probe.is_empty() {
            return None;
        }
        // One outer LIMIT over the whole union (subjects are unique).
        let rows = self
            .exec
            .query_json(&format!("{probe} LIMIT 1"))
            .await
            .ok()?;
        rows.first()
            .and_then(|r| r.get("t"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    /// The resolved-subject edge relation: every edge table joined to its src
    /// and dst vertex tables so `(src, dst)` are IRIs, not dense ids. `None`
    /// when no edge table survives the `edge_types` filter. Built once and used
    /// as the recursive-CTE base for [`find_neighbors`].
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
fn quote_ident(name: &str) -> String {
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

/// Extract a row column as `f32` (layout coordinates), defaulting to 0.0.
///
/// f64→f32 is an intentional narrowing: the viewport contract carries layout
/// coordinates as f32 so the consumer can ship them straight to a GPU buffer.
#[allow(clippy::cast_possible_truncation)]
fn json_f32(row: &Value, key: &str) -> f32 {
    row.get(key)
        .and_then(Value::as_f64)
        .map_or(0.0, |v| v as f32)
}

/// A row column as `u32`, `0` when absent or out of range.
fn json_u32(row: &Value, key: &str) -> u32 {
    row.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// One detail-mode row to a [`ViewportVertex`].
///
/// `dense_id` carries the **slice-local** index — the position in the answer
/// being built, which is what an edge refers to and what a GPU buffer is
/// indexed by. The vertex's own dense id is still selected by the query and
/// pairs with `type_idx` to identify it, for a caller that needs to ask about
/// one afterwards.
fn row_to_viewport_vertex(row: &Value) -> ViewportVertex {
    ViewportVertex {
        dense_id: json_u32(row, "local"),
        x: json_f32(row, "x"),
        y: json_f32(row, "y"),
        type_idx: u8::try_from(json_u32(row, "type_idx")).unwrap_or(0),
        cluster_size: None,
        cluster_id: None,
    }
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
            let measure = p.measure.as_deref().ok_or_else(|| GraphError::InvalidParams {
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
/// inferRole` (this surface is now the single source — [[`feedback_one_idiom_per_concern`]]):
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
/// numeric arm of role + histogram classification.
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
        DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
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
            DEFAULT_CHUNK_SIZE,
            "vertex/Person/",
            vec![PropertyGroup {
                file_type: "parquet".into(),
                properties: vec![
                    Property {
                        name: "dense_id".into(),
                        data_type: "uint32".into(),
                        is_primary: true,
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
    /// the fossil-runtime integration test, W2-06).
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

    struct FakeExec;
    impl DuckExecutor for FakeExec {
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
    /// (SQL correctness is the fossil-runtime integration test's job, W2-06).
    struct FnExec<F: Fn(&str) -> Vec<Value>>(F);
    impl<F: Fn(&str) -> Vec<Value>> DuckExecutor for FnExec<F> {
        async fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
            Ok(self.0(sql))
        }
    }

    #[test]
    fn aggregate_count_groups_and_maps_rows() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
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
            &FakeExec,
        )
        .unwrap_err();
        assert!(matches!(err, GraphError::InvalidParams { verb: "aggregate", .. }));
    }

    #[test]
    fn top_k_passes_rows_through() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
            assert!(sql.contains("ORDER BY") && sql.contains("DESC") && sql.contains("LIMIT 5"));
            vec![serde_json::json!({ "name": "x", "age": 9 })]
        });
        let v = run(
            &Operation::TopK(TopKParams {
                vertex_type: "Person".into(),
                order_by: "age".into(),
                k: 5,
                descending: true,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: TopKResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.rows.len(), 1);
    }

    #[test]
    fn aggregate_over_ranges_is_dense_and_carries_its_edges() {
        let m = fixture();
        // age is int64 → binnable. First query = bounds, second = the grouping.
        let exec = FnExec(|sql: &str| {
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
        let exec = FnExec(|_: &str| vec![serde_json::json!({ "grp": "a", "val": 1.0 })]);
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
            &FakeExec,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphError::InvalidParams { verb: "aggregate", .. }
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
        let exec = FnExec(|sql: &str| {
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
        let exec = FnExec(|sql: &str| {
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
            &FakeExec,
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
        assert_eq!(infer_role("user_id", "string", None, None), FieldRole::Identifier);
        assert_eq!(infer_role("IRI", "string", None, None), FieldRole::Identifier);
        assert_eq!(infer_role("home.uri", "string", None, None), FieldRole::Identifier);
        // "candid" contains "id" but not at a boundary → not an identifier.
        assert_eq!(infer_role("candidate", "string", Some(1), Some(10)), FieldRole::Dimension);
        // numeric → measure (GraphAr spellings, incl. the int64 bug-fix case).
        assert_eq!(infer_role("age", "int64", None, None), FieldRole::Measure);
        assert_eq!(infer_role("score", "double", None, None), FieldRole::Measure);
        // bool / temporal → dimension.
        assert_eq!(infer_role("active", "bool", None, None), FieldRole::Dimension);
        assert_eq!(infer_role("born", "date", None, None), FieldRole::Dimension);
        // high cardinality → identifier; low → dimension.
        assert_eq!(infer_role("email", "string", Some(95), Some(100)), FieldRole::Identifier);
        assert_eq!(infer_role("dept", "string", Some(3), Some(100)), FieldRole::Dimension);
        assert_eq!(infer_role("huge", "string", Some(201), Some(100_000)), FieldRole::Identifier);
    }

    #[test]
    fn get_vertex_returns_user_properties() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
            assert!(sql.contains("WHERE subject ="), "lookup by subject: {sql}");
            assert!(!sql.contains("dense_id"), "reserved cols filtered: {sql}");
            vec![serde_json::json!({ "age": 30, "name": "Alice" })]
        });
        let v = run(
            &Operation::GetVertex(GetVertexParams {
                vertex_type: "Person".into(),
                subject: "urn:a".into(),
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: GetVertexResult = serde_json::from_value(v).unwrap();
        let obj = r.vertex.expect("matched vertex");
        assert_eq!(obj.get("name").and_then(Value::as_str), Some("Alice"));
        assert!(obj.get("subject").is_none());
    }

    #[test]
    fn get_vertex_missing_subject_is_null() {
        let m = fixture();
        let exec = FnExec(|_sql: &str| Vec::new());
        let v = run(
            &Operation::GetVertex(GetVertexParams {
                vertex_type: "Person".into(),
                subject: "urn:nope".into(),
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: GetVertexResult = serde_json::from_value(v).unwrap();
        assert!(r.vertex.is_none());
    }

    #[test]
    fn materialize_graph_resolves_subjects_and_drops_orphans() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
            if sql.contains("type_name") {
                // vertex materialization: label = name column.
                vec![
                    serde_json::json!({ "subject": "urn:a", "label": "Alice", "type_name": "Person" }),
                    serde_json::json!({ "subject": "urn:b", "label": "Bob", "type_name": "Person" }),
                ]
            } else {
                // edge relation: one in-set edge + one orphan (urn:zzz not materialized).
                vec![
                    serde_json::json!({ "src": "urn:a", "dst": "urn:b", "predicate": "knows" }),
                    serde_json::json!({ "src": "urn:a", "dst": "urn:zzz", "predicate": "knows" }),
                ]
            }
        });
        let v = run(
            &Operation::MaterializeGraph(crate::operations::viewport::MaterializeGraphParams {
                vertex_types: Vec::new(),
                limit: 50_000,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: crate::operations::viewport::MaterializeGraphResult =
            serde_json::from_value(v).unwrap();
        assert_eq!(r.vertices.len(), 2);
        assert_eq!(r.vertices[0].id, "urn:a");
        assert_eq!(r.vertices[0].label, "Alice");
        assert_eq!(r.vertices[0].type_name, "Person");
        assert_eq!(r.edges.len(), 1, "orphan edge dropped");
        assert_eq!(r.edges[0].source, "urn:a");
        assert_eq!(r.edges[0].target, "urn:b");
        assert!(!r.truncated);
    }

    #[test]
    fn schema_unknown_field_is_typed_error() {
        let m = fixture();
        let err = run(
            &Operation::Schema(schema_params(Some("Person"), Some("ghost"))),
            &m,
            &FakeExec,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphError::UnknownEntity { kind: "field", .. }
        ));
    }

    #[test]
    fn find_neighbors_walks_resolved_edges() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
            if sql.contains("WITH RECURSIVE") {
                // depth-1 out-neighbours of the origin.
                vec![serde_json::json!({
                    "src": "urn:a",
                    "dst": "urn:b",
                    "dst_type": "Person",
                    "predicate": "knows",
                    "hop": 1,
                })]
            } else {
                // vertex_type_of probe for the origin.
                vec![serde_json::json!({ "t": "Person" })]
            }
        });
        let v = run(
            &Operation::FindNeighbors(FindNeighborsParams {
                iri: "urn:a".into(),
                depth: 1,
                edge_types: Vec::new(),
                limit: 500,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: crate::operations::discovery::FindNeighborsResult =
            serde_json::from_value(v).unwrap();
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.edges[0].source, "urn:a");
        assert_eq!(r.edges[0].target, "urn:b");
        assert_eq!(r.edges[0].predicate, "knows");
        // origin (hop 0) + neighbour (hop 1).
        assert_eq!(r.vertices.len(), 2);
        assert_eq!(r.vertices[0].iri, "urn:a");
        assert_eq!(r.vertices[0].hop, 0);
        assert_eq!(r.vertices[1].iri, "urn:b");
        assert_eq!(r.vertices[1].hop, 1);
        assert_eq!(r.vertices[1].vertex_type, "Person");
    }

    #[test]
    fn find_path_reconstructs_ordered_path() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
            if sql.contains("WITH RECURSIVE") {
                vec![serde_json::json!({
                    "nodes": ["urn:a", "urn:b", "urn:c"],
                    "preds": ["knows", "knows"],
                    "types": ["Person", "Person", "Person"],
                })]
            } else {
                vec![serde_json::json!({ "t": "Person" })]
            }
        });
        let v = run(
            &Operation::FindPath(FindPathParams {
                source_iri: "urn:a".into(),
                target_iri: "urn:c".into(),
                max_hops: 5,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: FindPathResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.vertices.len(), 3);
        assert_eq!(r.vertices[2].iri, "urn:c");
        assert_eq!(r.vertices[2].hop, 2);
        assert_eq!(r.edges.len(), 2);
        assert_eq!(r.edges[1].source, "urn:b");
        assert_eq!(r.edges[1].target, "urn:c");
    }

    /// The viewport params for a bbox big enough to hold the fixture, at `zoom`.
    fn viewport_params(zoom: f32) -> crate::operations::viewport::ViewportParams {
        crate::operations::viewport::ViewportParams {
            bbox: crate::operations::viewport::BoundingBox {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 10.0,
                y_max: 10.0,
            },
            zoom,
            lod_threshold: 0.5,
            limit: 1000,
            vertex_types: Vec::new(),
        }
    }

    #[test]
    fn viewport_detail_numbers_the_answer_and_joins_edges_to_it() {
        let m = fixture();
        // The verb asks three different questions, so the fake answers three —
        // a stub that returns one shape for every query cannot tell a vertex
        // scan from a count, and silently made `n` zero when it was asked.
        let exec = FnExec(|sql: &str| {
            if sql.contains("count(*)") {
                return vec![serde_json::json!({ "n": 7 })];
            }
            if sql.contains("AS src") {
                // Both endpoints are in `vis`, so the edge speaks slice-local.
                return vec![serde_json::json!({ "src": 0, "dst": 1 })];
            }
            assert!(sql.contains("x BETWEEN") && sql.contains("y BETWEEN"));
            assert!(sql.contains("row_number()"), "the answer must be numbered");
            vec![
                serde_json::json!({ "local": 0, "dense_id": 4, "x": 1.5, "y": 2.0, "type_idx": 0 }),
                serde_json::json!({ "local": 1, "dense_id": 9, "x": 3.0, "y": 4.0, "type_idx": 0 }),
            ]
        });
        let v = run(&Operation::Viewport(viewport_params(1.0)), &m, &exec).unwrap();

        let r: ViewportResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.mode, ViewportMode::Detail);
        // What matched, not what came back — the honesty column.
        assert_eq!(r.n, 7);
        assert_eq!(r.vertices.len(), 2);
        // The index is the position in this answer, not the corpus's dense id
        // (4 and 9 here), which repeats across types and dies at the limit.
        assert_eq!(r.vertices[0].dense_id, 0);
        assert_eq!(r.vertices[1].dense_id, 1);
        assert!((r.vertices[1].x - 3.0).abs() < f32::EPSILON);
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.edges[0].src_dense, 0);
        assert_eq!(r.edges[0].dst_dense, 1);
    }

    #[test]
    fn viewport_below_the_threshold_answers_super_nodes() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
            if sql.contains("count(*) AS n FROM (") {
                return vec![serde_json::json!({ "n": 5000 })];
            }
            // Grouped by the pair: cluster 0 of one type is not cluster 0 of
            // another, and merging them would centroid across two communities
            // that share nothing but an integer.
            assert!(sql.contains("GROUP BY type_idx, cluster_id"));
            vec![
                serde_json::json!({ "local": 0, "x": 1.0, "y": 1.0, "type_idx": 0, "cluster_id": 0, "cluster_size": 3000 }),
                serde_json::json!({ "local": 1, "x": 8.0, "y": 8.0, "type_idx": 0, "cluster_id": 1, "cluster_size": 2000 }),
            ]
        });
        let v = run(&Operation::Viewport(viewport_params(0.1)), &m, &exec).unwrap();

        let r: ViewportResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.mode, ViewportMode::Aggregate);
        // A view of everything is a few marks whatever the corpus holds.
        assert_eq!(r.n, 5000);
        assert_eq!(r.vertices.len(), 2);
        assert_eq!(r.vertices[0].cluster_size, Some(3000));
        assert_eq!(r.vertices[1].cluster_id, Some(1));
    }

    #[test]
    fn viewport_scan_keeps_the_predicate_pushdown_shape() {
        // A plain range on x/y is what DuckDB pushes into Parquet row-group
        // statistics, and the writer's Morton sort is what makes those
        // statistics tight. Written any other way the verb reads the whole
        // corpus to answer about a window, which is the entire point lost.
        let m = fixture();
        let exec = FnExec(|_: &str| Vec::new());
        let ctx = Context {
            manifest: &m,
            exec: &exec,
        };
        let sql = ctx.viewport_scan_sql(&viewport_params(1.0)).unwrap();
        assert!(sql.contains("WHERE x BETWEEN 0 AND 10 AND y BETWEEN 0 AND 10"));
        assert!(!sql.contains("sqrt") && !sql.contains("abs("));
    }

    #[test]
    fn execute_sql_caps_rows_and_reports_columns() {
        let m = fixture();
        let exec = FnExec(|sql: &str| {
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
                timeout_ms: 10_000,
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
