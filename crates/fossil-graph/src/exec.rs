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

use serde_json::Value;

use crate::manifest::{Manifest, edge_table_name};
use crate::operations::aggregate::{
    AggregateParams, AggregateResult, AggregateRow, Aggregation, HistogramKind, HistogramParams,
    HistogramResult, TopKParams, TopKResult,
};
use crate::operations::discovery::{
    FindNeighborsParams, FindNeighborsResult, FindPathParams, FindPathResult, NeighborEdge,
    NeighborVertex,
};
use crate::operations::schema::{
    DescribeFieldParams, DescribeFieldResult, EdgeTypeSummary, FieldRole, ListEdgeTypesResult,
    ListVertexTypesResult, VertexTypeSummary,
};
use crate::{GraphError, Operation, Result};

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
    fn query_json(&self, sql: &str) -> Result<Vec<Value>>;
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
/// Returns the verb's `Result` as JSON. Verbs gated on writer-W3 columns
/// (`GraphRAG`, `search_by_label`) and not-yet-wired verbs return
/// [`GraphError::NotImplemented`].
///
/// # Errors
///
/// Propagates verb execution errors; returns [`GraphError::NotImplemented`]
/// for verbs without a W2 implementation.
pub fn dispatch<E: DuckExecutor>(op: &Operation, manifest: &Manifest, exec: &E) -> Result<Value> {
    Context { manifest, exec }.dispatch(op)
}

impl<E: DuckExecutor> Context<'_, E> {
    fn dispatch(&self, op: &Operation) -> Result<Value> {
        match op {
            Operation::ListVertexTypes(_) => to_json(&self.list_vertex_types()?),
            Operation::ListEdgeTypes(_) => to_json(&self.list_edge_types()?),
            Operation::DescribeField(p) => to_json(&self.describe_field(p)?),
            Operation::Aggregate(p) => to_json(&self.aggregate(p)?),
            Operation::Histogram(p) => to_json(&self.histogram(p)?),
            Operation::TopK(p) => to_json(&self.top_k(p)?),
            Operation::FindNeighbors(p) => to_json(&self.find_neighbors(p)?),
            Operation::FindPath(p) => to_json(&self.find_path(p)?),
            other => Err(GraphError::NotImplemented(other.verb_name())),
        }
    }

    // ── Schema verbs ──────────────────────────────────────────────────────

    fn list_vertex_types(&self) -> Result<ListVertexTypesResult> {
        let mut types = Vec::with_capacity(self.manifest.vertices().len());
        for info in self.manifest.vertices() {
            types.push(VertexTypeSummary {
                name: info.vertex_type.clone(),
                iri: info.iri.clone(),
                count: self.count_rows(&info.vertex_type)?,
                fields: self.manifest.vertex_fields(&info.vertex_type)?,
            });
        }
        Ok(ListVertexTypesResult { types })
    }

    fn list_edge_types(&self) -> Result<ListEdgeTypesResult> {
        let mut edges = Vec::with_capacity(self.manifest.edges().len());
        for info in self.manifest.edges() {
            let table_name = edge_table_name(info);
            let count = self.count_rows(&table_name)?;
            edges.push(EdgeTypeSummary {
                source_type: info.src_type.clone(),
                name: info.edge_type.clone(),
                target_type: info.dst_type.clone(),
                iri: info.iri.clone(),
                count,
                table_name,
            });
        }
        Ok(ListEdgeTypesResult { edges })
    }

    fn describe_field(&self, p: &DescribeFieldParams) -> Result<DescribeFieldResult> {
        let datatype = self.field_datatype(&p.vertex_type, &p.field)?;

        let table = quote_ident(&p.vertex_type);
        let field = quote_ident(&p.field);

        let distinct = scalar_u64(
            &self.exec.query_json(&format!(
                "SELECT count(DISTINCT {field}) AS distinct_count FROM {table}"
            ))?,
            "distinct_count",
        );

        let sample_rows = self.exec.query_json(&format!(
            "SELECT {field} AS sample FROM {table} WHERE {field} IS NOT NULL LIMIT 8"
        ))?;
        let samples = sample_rows
            .iter()
            .filter_map(|row| row.get("sample"))
            .map(value_to_string)
            .collect();

        Ok(DescribeFieldResult {
            role: infer_role(&datatype),
            datatype,
            distinct,
            samples,
        })
    }

    // ── Aggregation verbs ─────────────────────────────────────────────────

    fn aggregate(&self, p: &AggregateParams) -> Result<AggregateResult> {
        self.manifest.lookup_vertex(&p.vertex_type)?;
        let table = quote_ident(&p.vertex_type);
        let group = quote_ident(&p.group_by);
        let agg_expr = match p.agg {
            Aggregation::Count => "count(*)".to_string(),
            measured => {
                let measure = p.measure.as_deref().ok_or_else(|| GraphError::InvalidParams {
                    verb: "aggregate",
                    detail: format!("agg `{}` requires a `measure` column", agg_fn(measured)),
                })?;
                format!("{}({})", agg_fn(measured), quote_ident(measure))
            }
        };
        // group cardinality is capped by LIMIT → constant memory.
        let sql = format!(
            "SELECT {group} AS grp, {agg_expr}::DOUBLE AS val FROM {table} \
             GROUP BY {group} ORDER BY val DESC LIMIT {}",
            p.limit
        );
        let rows = self
            .exec
            .query_json(&sql)?
            .iter()
            .map(|r| AggregateRow {
                group: r.get("grp").cloned().unwrap_or(Value::Null),
                value: r.get("val").and_then(Value::as_f64).unwrap_or(0.0),
            })
            .collect();
        Ok(AggregateResult { rows })
    }

    fn top_k(&self, p: &TopKParams) -> Result<TopKResult> {
        self.manifest.lookup_vertex(&p.vertex_type)?;
        let table = quote_ident(&p.vertex_type);
        let order = quote_ident(&p.order_by);
        let dir = if p.descending { "DESC" } else { "ASC" };
        let sql = format!("SELECT * FROM {table} ORDER BY {order} {dir} LIMIT {}", p.k);
        Ok(TopKResult {
            rows: self.exec.query_json(&sql)?,
        })
    }

    fn histogram(&self, p: &HistogramParams) -> Result<HistogramResult> {
        let field_kind = histogram_kind(&self.field_datatype(&p.vertex_type, &p.field)?);
        let table = quote_ident(&p.vertex_type);
        let field = quote_ident(&p.field);
        let bins = p.bins.max(1);

        match field_kind {
            HistogramKind::Categorical => {
                // No numeric axis: return the top-`bins` category counts. Labels
                // aren't representable in the f64 `edges` contract, so edges carry
                // the bin ordinals; callers pair them with a separate label query.
                let rows = self.exec.query_json(&format!(
                    "SELECT count(*) AS n FROM {table} WHERE {field} IS NOT NULL \
                     GROUP BY {field} ORDER BY n DESC LIMIT {bins}"
                ))?;
                let counts: Vec<u64> = rows
                    .iter()
                    .map(|r| r.get("n").and_then(Value::as_u64).unwrap_or(0))
                    .collect();
                let edges = (0..counts.len())
                    .map(|i| f64::from(u32::try_from(i).unwrap_or(u32::MAX)))
                    .collect();
                Ok(HistogramResult {
                    edges,
                    counts,
                    field_kind,
                })
            }
            HistogramKind::Numeric | HistogramKind::Temporal => {
                let bounds = self.exec.query_json(&format!(
                    "SELECT min({field})::DOUBLE AS lo, max({field})::DOUBLE AS hi FROM {table}"
                ))?;
                let (Some(lo), Some(hi)) = (
                    scalar_f64(&bounds, "lo"),
                    scalar_f64(&bounds, "hi"),
                ) else {
                    // Empty column → no bins.
                    return Ok(HistogramResult {
                        edges: Vec::new(),
                        counts: Vec::new(),
                        field_kind,
                    });
                };
                let width = (hi - lo) / f64::from(bins);
                let edges = (0..=bins).map(|i| f64::from(i).mul_add(width, lo)).collect();
                let mut counts = vec![0u64; bins as usize];
                if width > 0.0 {
                    let rows = self.exec.query_json(&format!(
                        "SELECT least({bins} - 1, floor(({field}::DOUBLE - {lo}) / {width}))::BIGINT \
                         AS bin, count(*) AS n FROM {table} WHERE {field} IS NOT NULL \
                         GROUP BY bin ORDER BY bin"
                    ))?;
                    for r in &rows {
                        let (Some(bin), Some(n)) = (
                            r.get("bin").and_then(Value::as_u64),
                            r.get("n").and_then(Value::as_u64),
                        ) else {
                            continue;
                        };
                        if let Some(slot) =
                            usize::try_from(bin).ok().and_then(|i| counts.get_mut(i))
                        {
                            *slot = n;
                        }
                    }
                } else {
                    // All values equal → one populated bin.
                    counts[0] = self.count_rows(&p.vertex_type)?;
                }
                Ok(HistogramResult {
                    edges,
                    counts,
                    field_kind,
                })
            }
        }
    }

    // ── Discovery verbs ───────────────────────────────────────────────────

    fn find_neighbors(&self, p: &FindNeighborsParams) -> Result<FindNeighborsResult> {
        let Some(edge_relation) = self.edge_relation_sql(&p.edge_types) else {
            // No edge types match → the origin has no reachable neighbours.
            return Ok(FindNeighborsResult {
                vertices: self.origin_only(&p.iri),
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

        let rows = self.exec.query_json(&sql)?;
        let mut edges = Vec::with_capacity(rows.len());
        let mut vertices = self.origin_only(&p.iri);
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

    fn find_path(&self, p: &FindPathParams) -> Result<FindPathResult> {
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
        let src_type = sql_str_lit(&self.vertex_type_of(&p.source_iri).unwrap_or_default());

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

        let rows = self.exec.query_json(&sql)?;
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

    /// The origin vertex alone (hop 0), with its type resolved from whichever
    /// vertex table holds the subject. Used as the seed of a neighbour result.
    fn origin_only(&self, iri: &str) -> Vec<NeighborVertex> {
        vec![NeighborVertex {
            iri: iri.to_string(),
            label: iri.to_string(),
            vertex_type: self.vertex_type_of(iri).unwrap_or_default(),
            hop: 0,
        }]
    }

    /// Resolve which vertex type holds `iri` by probing each type's `subject`
    /// column. Returns the first match (subjects are unique across the graph).
    fn vertex_type_of(&self, iri: &str) -> Option<String> {
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
        let rows = self.exec.query_json(&format!("{probe} LIMIT 1")).ok()?;
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
    fn count_rows(&self, table: &str) -> Result<u64> {
        let sql = format!("SELECT count(*) AS n FROM {}", quote_ident(table));
        scalar_u64(&self.exec.query_json(&sql)?, "n")
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

/// Classify a `GraphAr` `data_type` spelling for histogram binning.
fn histogram_kind(datatype: &str) -> HistogramKind {
    match datatype {
        "int32" | "int64" | "float" | "double" => HistogramKind::Numeric,
        "date" | "timestamp" | "time" => HistogramKind::Temporal,
        _ => HistogramKind::Categorical,
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Infer a chart-axis role from the `GraphAr` `data_type` spelling. Numeric
/// columns default to `measure`; everything else to `dimension`. The
/// `identifier` role is reserved for the writer's `dense_id`/`subject`, which
/// `Manifest::vertex_fields` already filters out of describable fields.
fn infer_role(datatype: &str) -> FieldRole {
    match datatype {
        "int32" | "int64" | "float" | "double" => FieldRole::Measure,
        _ => FieldRole::Dimension,
    }
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
    struct FakeExec;
    impl DuckExecutor for FakeExec {
        fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
            let row = if sql.contains("count(*)") {
                serde_json::json!({ "n": 3 })
            } else if sql.contains("count(DISTINCT") {
                serde_json::json!({ "distinct_count": 2 })
            } else {
                // samples query
                return Ok(vec![
                    serde_json::json!({ "sample": "30" }),
                    serde_json::json!({ "sample": "41" }),
                ]);
            };
            Ok(vec![row])
        }
    }

    /// Executor backed by a closure — canned responses keyed on the SQL, so a
    /// verb's wiring (param → SQL → result shaping) is tested without real `DuckDB`
    /// (SQL correctness is the fossil-runtime integration test's job, W2-06).
    struct FnExec<F: Fn(&str) -> Vec<Value>>(F);
    impl<F: Fn(&str) -> Vec<Value>> DuckExecutor for FnExec<F> {
        fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
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
        let v = dispatch(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "name".into(),
                agg: Aggregation::Count,
                measure: None,
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
        let err = dispatch(
            &Operation::Aggregate(AggregateParams {
                vertex_type: "Person".into(),
                group_by: "name".into(),
                agg: Aggregation::Sum,
                measure: None,
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
        let v = dispatch(
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
    fn histogram_numeric_builds_edges_and_counts() {
        let m = fixture();
        // age is int64 → numeric. First query = bounds, second = buckets.
        let exec = FnExec(|sql: &str| {
            if sql.contains("min(") {
                vec![serde_json::json!({ "lo": 0.0, "hi": 4.0 })]
            } else {
                vec![
                    serde_json::json!({ "bin": 0, "n": 3 }),
                    serde_json::json!({ "bin": 3, "n": 1 }),
                ]
            }
        });
        let v = dispatch(
            &Operation::Histogram(HistogramParams {
                vertex_type: "Person".into(),
                field: "age".into(),
                bins: 4,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: HistogramResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.field_kind, HistogramKind::Numeric);
        assert_eq!(r.edges, vec![0.0, 1.0, 2.0, 3.0, 4.0]); // bins+1 edges
        assert_eq!(r.counts, vec![3, 0, 0, 1]); // bins counts, bucketed by index
    }

    #[test]
    fn histogram_categorical_returns_ordinal_edges() {
        let m = fixture();
        // name is string → categorical: top-bins counts, ordinal edges.
        let exec = FnExec(|sql: &str| {
            assert!(sql.contains("GROUP BY") && !sql.contains("min("));
            vec![
                serde_json::json!({ "n": 5 }),
                serde_json::json!({ "n": 2 }),
            ]
        });
        let v = dispatch(
            &Operation::Histogram(HistogramParams {
                vertex_type: "Person".into(),
                field: "name".into(),
                bins: 10,
            }),
            &m,
            &exec,
        )
        .unwrap();
        let r: HistogramResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.field_kind, HistogramKind::Categorical);
        assert_eq!(r.counts, vec![5, 2]);
        assert_eq!(r.edges, vec![0.0, 1.0]);
    }

    #[test]
    fn list_vertex_types_carries_iri_count_and_user_fields() {
        let m = fixture();
        let v = dispatch(
            &Operation::ListVertexTypes(crate::operations::schema::ListVertexTypesParams {}),
            &m,
            &FakeExec,
        )
        .unwrap();
        let r: ListVertexTypesResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.types.len(), 1);
        let p = &r.types[0];
        assert_eq!(p.name, "Person");
        assert_eq!(p.iri, "http://example.org/Person");
        assert_eq!(p.count, 3);
        // dense_id hidden; user fields surfaced.
        assert_eq!(p.fields, vec!["age", "name"]);
    }

    #[test]
    fn list_edge_types_builds_table_name_and_iri() {
        let m = fixture();
        let v = dispatch(
            &Operation::ListEdgeTypes(crate::operations::schema::ListEdgeTypesParams {}),
            &m,
            &FakeExec,
        )
        .unwrap();
        let r: ListEdgeTypesResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.edges.len(), 1);
        let e = &r.edges[0];
        assert_eq!(e.table_name, "Person_knows_Person");
        assert_eq!(e.iri, "http://example.org/knows");
        assert_eq!(e.count, 3);
    }

    #[test]
    fn describe_field_infers_measure_for_numeric() {
        let m = fixture();
        let v = dispatch(
            &Operation::DescribeField(DescribeFieldParams {
                vertex_type: "Person".into(),
                field: "age".into(),
            }),
            &m,
            &FakeExec,
        )
        .unwrap();
        let r: DescribeFieldResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.datatype, "int64");
        assert_eq!(r.role, FieldRole::Measure);
        assert_eq!(r.distinct, Some(2));
        assert_eq!(r.samples, vec!["30", "41"]);
    }

    #[test]
    fn describe_unknown_field_is_typed_error() {
        let m = fixture();
        let err = dispatch(
            &Operation::DescribeField(DescribeFieldParams {
                vertex_type: "Person".into(),
                field: "ghost".into(),
            }),
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
        let v = dispatch(
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
        let v = dispatch(
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

    #[test]
    fn unimplemented_verb_reports_its_name() {
        let m = fixture();
        let err = dispatch(
            &Operation::SearchByLabel(crate::operations::discovery::SearchByLabelParams {
                query: "x".into(),
                vertex_types: Vec::new(),
                top_k: 20,
            }),
            &m,
            &FakeExec,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphError::NotImplemented("search_by_label")
        ));
    }
}
