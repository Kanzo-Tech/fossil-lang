//! Verb execution: the [`DuckExecutor`] seam + per-verb SQL generation.
//!
//! The verb→SQL logic lives here (pure string building, WASM-safe) so every
//! binding reuses it: a binding differs only in how it satisfies
//! [`DuckExecutor`] — it never re-derives a verb's SQL. The one binding is
//! `fossil-graph-wasm`, over DuckDB-WASM.
//!
//! [`dispatch`] matches the [`Operation`] enum and returns the verb's `Result`
//! serialised to JSON — the transport-agnostic wire form every binding ships.
//!
//! The verb futures are intentionally not `Send`: DuckDB-WASM runs on one
//! browser thread. Requiring `Send`
//! would force the bound onto every executor (and DuckDB-WASM, which is not
//! `Send`) for zero benefit, so `future_not_send` is allowed crate-wide here.
#![allow(clippy::future_not_send)]

use std::fmt::Write;

use serde_json::Value;

use crate::manifest::RESERVED_VERTEX_COLUMNS;
use crate::manifest::{Manifest, edge_table_name};
use crate::operations::schema::{
    EdgeTypeSummary, FieldKind, FieldRole, FieldStat, SchemaParams, SchemaResult, VertexTypeSummary,
};
use crate::operations::sql::{ColumnDescriptor, ExecuteSqlParams, ExecuteSqlResult};
use crate::{GraphError, Operation, Result};

/// What [`DuckExecutor::query_columns`] gives back: the result rows, and the
/// `(name, type)` descriptor of each column.
///
/// It was `pub type ColumnedRows = (Vec<(String, String)>, Vec<Value>)`, and
/// the name and the shape were the same complaint. A two-`Vec` tuple is
/// destructured positionally at every call site — `let (columns, mut rows) =`
/// here — so which `Vec` is
/// which was carried by argument order and by a name («columned») that had to
/// be read twice. Two fields say it once.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// `(column_name, column_type)`, in the order the query returns them.
    ///
    /// The type string is whatever the binding can say: a binding that can
    /// read `DuckDB`'s own logical-type spelling overrides the default below,
    /// which derives it from the JSON value kinds, best-effort, and says so.
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
    /// access to real `DuckDB` column types overrides this.
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
    catalog: Option<&'a str>,
}

/// Dispatch one [`Operation`] against the manifest + executor.
///
/// Returns the verb's `Result` as JSON. The match is exhaustive — a verb with
/// no implementation is deleted rather than stubbed — so a new one cannot be
/// added without wiring it.
///
/// # Errors
///
/// Propagates verb execution errors.
///
/// `catalog` is the database the relations live in. `None` names them bare, as a
/// connection that holds one corpus registers them; a host that holds several
/// registers each in its own catalog and passes its name, so two corpora with the
/// same vertex type never resolve to each other's view.
pub async fn dispatch<E: DuckExecutor>(
    op: &Operation,
    manifest: &Manifest,
    catalog: Option<&str>,
    exec: &E,
) -> Result<Value> {
    Context {
        manifest,
        exec,
        catalog,
    }
    .dispatch(op)
    .await
}

impl<E: DuckExecutor> Context<'_, E> {
    async fn dispatch(&self, op: &Operation) -> Result<Value> {
        match op {
            Operation::Schema(p) => to_json(&self.schema(p).await?),
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
                stats: if p.stats {
                    self.field_stats(&info.vertex_type, None).await?
                } else {
                    Vec::new()
                },
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
            .properties()
            .iter()
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
                self.relation(vertex_type)
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
                    kind: FieldKind::of(datatype),
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
                table = self.relation(vertex_type),
            ))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get("sample"))
            .map(value_to_string)
            .collect())
    }

    // ── Escape hatch ──────────────────────────────────────────────────────

    async fn execute_sql(&self, p: &ExecuteSqlParams) -> Result<ExecuteSqlResult> {
        let cap = u64::from(p.row_cap);
        // Wrap so the row cap is enforced regardless of the user's own LIMIT;
        // fetch one extra row to detect truncation. Rows are the only bound:
        // neither host can interrupt a running statement, so a query that is
        // slow rather than large runs to completion.
        let wrapped = format!("SELECT * FROM ({}) AS _q LIMIT {}", p.sql.as_str(), cap + 1);
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

    // ── Helpers ───────────────────────────────────────────────────────────

    /// A relation as SQL names it: in [`Self::catalog`] when there is one.
    fn relation(&self, name: &str) -> String {
        self.catalog.map_or_else(
            || quote_ident(name),
            |catalog| format!("{}.{}", quote_ident(catalog), quote_ident(name)),
        )
    }

    /// Cheap row count — `DuckDB` answers from the Parquet footer metadata
    /// (O(1), no full scan) for `read_parquet`-backed views.
    async fn count_rows(&self, table: &str) -> Result<u64> {
        let sql = format!("SELECT count(*) AS n FROM {}", self.relation(table));
        scalar_u64(&self.exec.query_json(&sql).await?, "n")
            .ok_or_else(|| GraphError::Execution(format!("count(*) on `{table}` returned no row")))
    }
}

/// Quote a `DuckDB` identifier, doubling embedded quotes.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Best-effort `DuckDB` type name from a JSON value kind — the default
/// [`DuckExecutor::query_columns`] fallback when real column types aren't
/// available.
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
    match FieldKind::of(datatype) {
        FieldKind::Numeric => return FieldRole::Measure,
        FieldKind::Temporal => return FieldRole::Dimension,
        FieldKind::Categorical if matches!(datatype, "bool" | "boolean") => {
            return FieldRole::Dimension;
        }
        FieldKind::Categorical => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestSource;
    use crate::operations::raw_sql::{RawSql, RawSqlAccess};
    use crate::plan::GRAPH_INFO_PATH;
    use fossil_sinks::manifest::{
        Cardinality, Container, DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Projection, Property,
        VertexInfo,
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
        VertexInfo::new(
            "Person",
            3,
            DEFAULT_CHUNK_SIZE,
            "vertex/Person/",
            vec![Projection::payload(
                "",
                vec![
                    Property {
                        name: "dense_id".into(),
                        data_type: "uint32".into(),
                        // The address is never the primary; this fixture carries
                        // no identity column, so it carries no primary.
                        is_primary: false,
                        is_nullable: Some(false),
                        cardinality: Some(Cardinality::Single),
                    },
                    Property {
                        name: "age".into(),
                        data_type: "int64".into(),
                        is_primary: false,
                        is_nullable: None,
                        cardinality: None,
                    },
                    Property {
                        name: "name".into(),
                        data_type: "string".into(),
                        is_primary: false,
                        is_nullable: None,
                        cardinality: None,
                    },
                ],
            )],
        )
        .with_iri("http://example.org/Person")
    }

    fn fixture() -> Manifest {
        let person = person_info();
        let edge = EdgeInfo::new(
            "Person",
            "knows",
            "Person",
            2,
            DEFAULT_CHUNK_SIZE,
            "edge/Person_knows_Person/",
            vec![],
        )
        .with_iri("http://example.org/knows");
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
        block_on(dispatch(op, m, None, exec))
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
            Ok(vec![
                serde_json::json!({ "sample": "30" }),
                serde_json::json!({ "sample": "41" }),
            ])
        }
    }

    /// Executor backed by a closure — canned responses keyed on the SQL, so a
    /// verb's wiring (param → SQL → result shaping) is tested without real `DuckDB`.
    /// SQL correctness against a real engine is `@fossil-lang/corpus`'s e2e suite.
    struct FnExecutor<F: Fn(&str) -> Vec<Value>>(F);
    impl<F: Fn(&str) -> Vec<Value>> DuckExecutor for FnExecutor<F> {
        async fn query_json(&self, sql: &str) -> Result<Vec<Value>> {
            Ok(self.0(sql))
        }
    }

    /// A [`SchemaParams`] naming nothing, one type, or one type + one field.
    fn schema_params(vertex_type: Option<&str>, field: Option<&str>) -> SchemaParams {
        SchemaParams {
            vertex_type: vertex_type.map(ToString::to_string),
            field: field.map(ToString::to_string),
            stats: false,
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
        assert_eq!(r.fields[0].kind, FieldKind::Numeric);
        assert_eq!(r.fields[1].kind, FieldKind::Categorical);
        // Per-type stats are the `stats` flag's, not `vertex_type`'s.
        assert!(r.vertices.iter().all(|v| v.stats.is_empty()));
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
    fn schema_with_stats_answers_every_type_in_one_call() {
        let m = fixture();
        let exec = FnExecutor(|sql: &str| {
            if sql.contains("count(DISTINCT") {
                return vec![serde_json::json!({ "n": 3, "d0": 3, "d1": 2 })];
            }
            vec![serde_json::json!({ "n": 3 })]
        });
        let params = SchemaParams {
            stats: true,
            ..SchemaParams::default()
        };
        let r: SchemaResult =
            serde_json::from_value(run(&Operation::Schema(params), &m, &exec).unwrap()).unwrap();
        let person = r.vertices.iter().find(|v| v.name == "Person").unwrap();
        let names: Vec<_> = person.stats.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            person.fields.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(person.stats[0].kind, FieldKind::Numeric);
        // The per-type answer is on the summaries; the narrowed slot stays empty.
        assert!(r.fields.is_empty());
    }

    #[test]
    fn field_kind_reads_the_graphar_spelling() {
        for numeric in ["int8", "int64", "uint32", "float", "double"] {
            assert_eq!(FieldKind::of(numeric), FieldKind::Numeric, "{numeric}");
        }
        for temporal in ["date", "timestamp", "time"] {
            assert_eq!(FieldKind::of(temporal), FieldKind::Temporal, "{temporal}");
        }
        for categorical in ["string", "bool", "list<string>", ""] {
            assert_eq!(
                FieldKind::of(categorical),
                FieldKind::Categorical,
                "{categorical}"
            );
        }
        assert!(FieldKind::Temporal.is_binnable());
        assert!(!FieldKind::Categorical.is_binnable());
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
                sql: RawSql::new(RawSqlAccess::granted(), "SELECT * FROM t"),
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
