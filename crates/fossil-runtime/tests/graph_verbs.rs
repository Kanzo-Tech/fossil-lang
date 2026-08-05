//! W2-06 — integration: dispatch every implemented fossil-graph verb against a
//! real bundled `DuckDB`, validating the verb→SQL the unit tests only exercise
//! with fake executors.

use std::collections::HashMap;

use duckdb::Connection;
use fossil_graph::manifest::{Manifest, ManifestSource};
use fossil_graph::operations::aggregate::{AggregateParams, AggregateResult, Aggregation};
use fossil_graph::operations::discovery::{
    ExpandMode, ExpandParams, ExpandResult, PathParams, PathResult, ReadParams, ReadResult,
};
use fossil_graph::operations::schema::{FieldRole, SchemaParams, SchemaResult};
use fossil_graph::{GraphError, Operation, Result, dispatch};
use fossil_runtime::DuckRuntime;
use fossil_sinks::manifest::{
    DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
};

/// In-memory manifest source mirroring the writer's on-disk layout.
struct MapSource(HashMap<String, Vec<u8>>);
impl ManifestSource for MapSource {
    fn fetch(&self, rel_path: &str) -> Result<Vec<u8>> {
        self.0
            .get(rel_path)
            .cloned()
            .ok_or_else(|| GraphError::InvalidManifest(format!("missing {rel_path}")))
    }
}

fn prop(name: &str, ty: &str, primary: bool) -> Property {
    Property {
        name: name.to_string(),
        data_type: ty.to_string(),
        is_primary: primary,
        is_nullable: Some(false),
    }
}

/// Manifest for a single `Person` vertex type (fields age, name) + a
/// `Person knows Person` edge — matching the `DuckDB` tables created below.
fn manifest() -> Manifest {
    let mut person = VertexInfo::new(
        "Person",
        DEFAULT_CHUNK_SIZE,
        "vertex/Person/",
        vec![PropertyGroup {
            file_type: "parquet".into(),
            properties: vec![
                prop("dense_id", "uint32", true),
                prop("subject", "string", false),
                prop("age", "int64", false),
                prop("name", "string", false),
                prop("x", "float", false),
                prop("y", "float", false),
                prop("cluster_id", "uint32", false),
            ],
        }],
    );
    person.iri = "http://example.org/Person".into();
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
    map.insert("graph.graph.yml".into(), graph.to_yaml().unwrap().into_bytes());
    map.insert(
        "vertex/Person.vertex.yml".into(),
        person.to_yaml().unwrap().into_bytes(),
    );
    map.insert(
        "edge/Person_knows_Person/Person_knows_Person.edge.yml".into(),
        edge.to_yaml().unwrap().into_bytes(),
    );
    Manifest::load(&MapSource(map)).expect("manifest loads")
}

/// A connection with the GraphAr-shaped vertex/edge tables registered under the
/// names the verbs query (`"Person"`, `"Person_knows_Person"`). Path a→b→c.
fn connection() -> Connection {
    let conn = Connection::open_in_memory().expect("open duckdb");
    conn.execute_batch(
        r#"
        CREATE TABLE "Person" (
            dense_id UINTEGER, subject VARCHAR, age BIGINT, name VARCHAR,
            x REAL, y REAL, cluster_id UINTEGER
        );
        INSERT INTO "Person" VALUES
            (0, 'urn:a', 30, 'Ann', 0, 0, 0),
            (1, 'urn:b', 41, 'Bob', 0, 0, 0),
            (2, 'urn:c', 25, 'Cy',  0, 0, 0);
        CREATE TABLE "Person_knows_Person" (src_dense UINTEGER, dst_dense UINTEGER);
        INSERT INTO "Person_knows_Person" VALUES (0, 1), (1, 2);
        "#,
    )
    .expect("seed tables");
    conn
}

/// Drive the async dispatch to completion synchronously. The native runtime
/// never suspends, so a noop-waker poll returns on the first poll.
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

fn run<T: serde::de::DeserializeOwned>(conn: &Connection, m: &Manifest, op: &Operation) -> T {
    let exec = DuckRuntime::new(conn);
    let value = block_on(dispatch(op, m, &exec)).expect("verb dispatch");
    serde_json::from_value(value).expect("result deserialises")
}

#[test]
fn bare_schema_counts_both_halves_and_leaves_fields_alone() {
    let (conn, m) = (connection(), manifest());
    let r: SchemaResult = run(&conn, &m, &Operation::Schema(SchemaParams::default()));
    assert_eq!(r.vertices.len(), 1);
    let t = &r.vertices[0];
    assert_eq!(t.name, "Person");
    assert_eq!(t.iri, "http://example.org/Person");
    assert_eq!(t.count, 3);
    assert_eq!(t.fields, vec!["age", "name"]); // reserved cols hidden
    assert_eq!(r.edges.len(), 1);
    assert_eq!(r.edges[0].table_name, "Person_knows_Person");
    assert_eq!(r.edges[0].count, 2);
    assert!(r.fields.is_empty(), "no type named → no field statistics");
}

#[test]
fn schema_for_a_field_is_a_measure_with_samples() {
    let (conn, m) = (connection(), manifest());
    let r: SchemaResult = run(
        &conn,
        &m,
        &Operation::Schema(SchemaParams {
            vertex_type: Some("Person".into()),
            field: Some("age".into()),
        }),
    );
    assert_eq!(r.fields.len(), 1);
    let age = &r.fields[0];
    assert_eq!(age.datatype, "int64");
    assert_eq!(age.role, FieldRole::Measure);
    assert_eq!(age.distinct, 3);
    assert_eq!(age.samples.len(), 3);
}

#[test]
fn aggregate_count_by_name() {
    let (conn, m) = (connection(), manifest());
    let r: AggregateResult = run(
        &conn,
        &m,
        &Operation::Aggregate(AggregateParams {
            vertex_type: "Person".into(),
            group_by: "name".into(),
            agg: Aggregation::Count,
            measure: None,
            bins: None,
            limit: 100,
        }),
    );
    assert_eq!(r.rows.len(), 3);
    assert!(r.rows.iter().all(|row| (row.value - 1.0).abs() < f64::EPSILON));
}

#[test]
fn aggregate_avg_requires_and_uses_measure() {
    let (conn, m) = (connection(), manifest());
    let r: AggregateResult = run(
        &conn,
        &m,
        &Operation::Aggregate(AggregateParams {
            vertex_type: "Person".into(),
            group_by: "name".into(),
            agg: Aggregation::Avg,
            measure: Some("age".into()),
            bins: None,
            limit: 100,
        }),
    );
    // one row per name, value == that person's age.
    assert_eq!(r.rows.len(), 3);
    let max = r.rows.iter().map(|row| row.value).fold(0.0_f64, f64::max);
    assert!((max - 41.0).abs() < f64::EPSILON);
}

#[test]
fn aggregate_bins_age_over_real_ranges() {
    let (conn, m) = (connection(), manifest());
    let r: AggregateResult = run(
        &conn,
        &m,
        &Operation::Aggregate(AggregateParams {
            vertex_type: "Person".into(),
            group_by: "age".into(),
            agg: Aggregation::Count,
            measure: None,
            bins: Some(4),
            limit: 100,
        }),
    );
    assert_eq!(r.edges.len(), 5); // bins + 1
    assert!((r.edges[0] - 25.0).abs() < 1e-6);
    assert!((r.edges[4] - 41.0).abs() < 1e-6);
    assert_eq!(r.rows.len(), 4); // dense: one row per bin
    let total: f64 = r.rows.iter().map(|row| row.value).sum();
    assert!((total - 3.0).abs() < f64::EPSILON);
}

#[test]
fn read_orders_and_caps_what_top_k_used_to() {
    let (conn, m) = (connection(), manifest());
    let r: ReadResult = run(
        &conn,
        &m,
        &Operation::Read(ReadParams {
            vertex_type: "Person".into(),
            r#where: None,
            order_by: Some("age".into()),
            descending: true,
            limit: 2,
        }),
    );
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0]["name"], serde_json::json!("Bob")); // 41
    assert_eq!(r.rows[1]["name"], serde_json::json!("Ann")); // 30
    // Identity rides along; the writer's layout columns do not.
    assert_eq!(r.rows[0]["subject"], serde_json::json!("urn:b"));
    assert!(r.rows[0].get("dense_id").is_none());
    assert!(r.rows[0].get("x").is_none());
}

#[test]
fn read_by_subject_is_what_get_vertex_was() {
    let (conn, m) = (connection(), manifest());
    let r: ReadResult = run(
        &conn,
        &m,
        &Operation::Read(ReadParams {
            vertex_type: "Person".into(),
            r#where: Some("subject = 'urn:a'".into()),
            order_by: None,
            descending: false,
            limit: 1,
        }),
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0]["name"], serde_json::json!("Ann"));
}

#[test]
fn expand_all_one_hop() {
    let (conn, m) = (connection(), manifest());
    let r: ExpandResult = run(
        &conn,
        &m,
        &Operation::Expand(ExpandParams {
            from: vec!["urn:a".into()],
            mode: ExpandMode::All,
            depth: 1,
            edge_types: Vec::new(),
            limit: 100,
        }),
    );
    assert_eq!(r.edges.len(), 1);
    assert_eq!(r.edges[0].source, "urn:a");
    assert_eq!(r.edges[0].target, "urn:b");
    assert_eq!(r.edges[0].predicate, "knows");
    // origin (hop 0) + neighbour (hop 1).
    assert_eq!(r.vertices.len(), 2);
    assert_eq!(r.vertices[0].iri, "urn:a");
    assert_eq!(r.vertices[0].vertex_type, "Person");
    assert_eq!(r.vertices[1].iri, "urn:b");
    assert_eq!(r.vertices[1].hop, 1);
}

#[test]
fn expand_all_two_hops_reaches_c() {
    let (conn, m) = (connection(), manifest());
    let r: ExpandResult = run(
        &conn,
        &m,
        &Operation::Expand(ExpandParams {
            from: vec!["urn:a".into()],
            mode: ExpandMode::All,
            depth: 2,
            edge_types: Vec::new(),
            limit: 100,
        }),
    );
    let reached: Vec<&str> = r.vertices.iter().map(|v| v.iri.as_str()).collect();
    assert!(reached.contains(&"urn:b"));
    assert!(reached.contains(&"urn:c"));
}

#[test]
fn expand_into_drops_the_edge_that_leaves_the_set() {
    // a→b→c, and the set is {a, b}: only a→b survives, and c never appears.
    let (conn, m) = (connection(), manifest());
    let r: ExpandResult = run(
        &conn,
        &m,
        &Operation::Expand(ExpandParams {
            from: vec!["urn:a".into(), "urn:b".into()],
            mode: ExpandMode::Into,
            depth: 5, // ignored: an induced subgraph has no frontier.
            edge_types: Vec::new(),
            limit: 100,
        }),
    );
    assert_eq!(r.edges.len(), 1);
    assert_eq!(r.edges[0].source, "urn:a");
    assert_eq!(r.edges[0].target, "urn:b");
    let named: Vec<&str> = r.vertices.iter().map(|v| v.iri.as_str()).collect();
    assert_eq!(named, vec!["urn:a", "urn:b"]);
    assert!(r.vertices.iter().all(|v| v.vertex_type == "Person"));
}

#[test]
fn path_a_to_c() {
    let (conn, m) = (connection(), manifest());
    let r: PathResult = run(
        &conn,
        &m,
        &Operation::Path(PathParams {
            source_iri: "urn:a".into(),
            target_iri: "urn:c".into(),
            max_hops: 5,
        }),
    );
    let path: Vec<&str> = r.vertices.iter().map(|v| v.iri.as_str()).collect();
    assert_eq!(path, vec!["urn:a", "urn:b", "urn:c"]);
    assert_eq!(r.edges.len(), 2);
    assert_eq!(r.edges[0].source, "urn:a");
    assert_eq!(r.edges[1].target, "urn:c");
}

#[test]
fn execute_sql_real_columns_and_cap() {
    use fossil_graph::operations::sql::{ExecuteSqlParams, ExecuteSqlResult};
    let (conn, m) = (connection(), manifest());

    // Real column types come from DuckDB (Arrow logical-type spelling).
    let r: ExecuteSqlResult = run(
        &conn,
        &m,
        &Operation::ExecuteSql(ExecuteSqlParams {
            sql: "SELECT name, age FROM \"Person\" ORDER BY age".into(),
            row_cap: 100,
            timeout_ms: 10_000,
        }),
    );
    assert!(!r.truncated);
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.columns.len(), 2);
    assert_eq!(r.columns[0].name, "name");
    assert!(r.columns[1].duckdb_type.contains("Int")); // age BIGINT → Int64

    // row_cap truncates and reports it.
    let capped: ExecuteSqlResult = run(
        &conn,
        &m,
        &Operation::ExecuteSql(ExecuteSqlParams {
            sql: "SELECT * FROM \"Person\"".into(),
            row_cap: 2,
            timeout_ms: 10_000,
        }),
    );
    assert!(capped.truncated);
    assert_eq!(capped.rows.len(), 2);
}

#[test]
fn path_unreachable_is_empty() {
    let (conn, m) = (connection(), manifest());
    let r: PathResult = run(
        &conn,
        &m,
        &Operation::Path(PathParams {
            source_iri: "urn:c".into(), // c has no outgoing edges
            target_iri: "urn:a".into(),
            max_hops: 5,
        }),
    );
    assert!(r.vertices.is_empty());
    assert!(r.edges.is_empty());
}
