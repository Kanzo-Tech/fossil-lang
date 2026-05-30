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
use crate::operations::schema::{
    DescribeFieldParams, DescribeFieldResult, EdgeTypeSummary, FieldRole, ListEdgeTypesResult,
    ListVertexTypesResult, VertexTypeSummary,
};
use crate::{GraphError, Operation, Result};

/// The thin DuckDB seam (ADR-0003 thin-DB-trait). A binding implements exactly
/// this — run a query, hand back rows as JSON objects — and inherits every
/// verb's SQL for free.
pub trait DuckExecutor {
    /// Run `sql`, returning each result row as a JSON object (column → value).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::Execution`] (carrying the binding's stringified
    /// DB error) when the query fails.
    fn query_json(&self, sql: &str) -> Result<Vec<Value>>;
}

/// Everything a verb needs to execute: the parsed [`Manifest`] + a DuckDB seam.
/// Internal — the public entry point is the free [`dispatch`] function so there
/// is a single way to run a verb.
struct Context<'a, E: DuckExecutor> {
    manifest: &'a Manifest,
    exec: &'a E,
}

/// Dispatch one [`Operation`] against the manifest + executor, returning the
/// verb's `Result` as JSON. Verbs gated on writer-W3 columns (GraphRAG,
/// `search_by_label`) and not-yet-wired verbs return [`GraphError::NotImplemented`].
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
        let info = self.manifest.lookup_vertex(&p.vertex_type)?;
        let datatype = info
            .property_groups
            .iter()
            .flat_map(|g| g.properties.iter())
            .find(|prop| prop.name == p.field)
            .map(|prop| prop.data_type.clone())
            .ok_or_else(|| GraphError::UnknownEntity {
                kind: "field",
                name: p.field.clone(),
            })?;

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
    /// wiring is testable without a real DuckDB (SQL correctness is covered by
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
