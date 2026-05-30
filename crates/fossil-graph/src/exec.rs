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
    fn unimplemented_verb_reports_its_name() {
        let m = fixture();
        let err = dispatch(
            &Operation::FindPath(crate::operations::discovery::FindPathParams {
                source_iri: "a".into(),
                target_iri: "b".into(),
                max_hops: 3,
            }),
            &m,
            &FakeExec,
        )
        .unwrap_err();
        assert!(matches!(err, GraphError::NotImplemented("find_path")));
    }
}
