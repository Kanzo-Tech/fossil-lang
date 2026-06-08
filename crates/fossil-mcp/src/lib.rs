//! fossil-mcp — the verb surface as a server-side service.
//!
//! `fossil-graph` owns the verb→SQL logic (WASM-clean); this crate runs it
//! NATIVELY: open a `DuckDB` connection, install the dataset's cloud secret,
//! register the `GraphAr` views the verbs query, load the manifest, and
//! [`fossil_graph::dispatch`] the operation via [`fossil_runtime::DuckRuntime`].
//!
//! The MCP transport (`main.rs`, `rmcp`) is a thin shell over [`dispatch_json`]:
//! the host (keasy) spawns this binary, calls the verb tool with a
//! [`Dataset`] + an [`fossil_graph::Operation`], and gets the verb's JSON
//! result. Secrets ride the MCP message on stdin (never argv/env), honouring
//! the same invariant as `fossil run --creds-stdin`.

use std::collections::HashMap;

use duckdb::Connection;
use fossil_graph::manifest::{Manifest, ManifestSource, edge_table_name};
use fossil_graph::{GraphError, Operation, Result as GraphResult};
use fossil_resolver::{CloudSecret, ResolvedPath};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::Value;

/// The per-call dataset context: where the `GraphAr` dataset lives, the cloud
/// secret to read it, and the manifest YAMLs (keyed by dataset-relative path).
#[derive(Debug, Deserialize)]
pub struct Dataset {
    /// Dataset base URL (the writer's `--dest`), e.g. `s3://bucket/prefix`.
    pub dest: String,
    /// Cloud secret for reading `dest` over httpfs; `None` ⇒ local / public.
    #[serde(default)]
    pub secret: Option<SecretSpec>,
    /// `GraphAr` manifest YAMLs (`graph.graph.yml` + per-type), keyed by
    /// dataset-relative path — the same blobs keasy serves via `/discover/manifest`.
    pub manifest_files: HashMap<String, String>,
}

/// A provider-typed cloud secret (mirrors `fossil run`'s `--creds-stdin`
/// `SecretSpec`): `type` is the `DuckDB` provider (`s3`/`azure`/`gcs`), `params`
/// are `CREATE SECRET` parameter names. Values arrive over the MCP stdin
/// message and are wrapped in [`SecretString`] before use.
#[derive(Debug, Deserialize)]
pub struct SecretSpec {
    #[serde(rename = "type")]
    pub secret_type: String,
    pub params: HashMap<String, String>,
}

struct MapSource(HashMap<String, Vec<u8>>);
impl ManifestSource for MapSource {
    fn fetch(&self, rel_path: &str) -> GraphResult<Vec<u8>> {
        self.0
            .get(rel_path)
            .cloned()
            .ok_or_else(|| GraphError::InvalidManifest(format!("missing {rel_path}")))
    }
}

/// Open a connection, install the secret, register `GraphAr` views, load the
/// manifest — ready for [`fossil_graph::dispatch`].
///
/// # Errors
///
/// Returns a stringified error if the connection, secret install, manifest
/// parse, or view registration fails.
pub fn open(dataset: &Dataset) -> std::result::Result<(Connection, Manifest), String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    fossil_runtime::apply_resource_limits(&conn).map_err(|e| e.to_string())?;

    if let Some(spec) = &dataset.secret {
        let params: HashMap<String, SecretString> = spec
            .params
            .iter()
            .map(|(k, v)| (k.clone(), SecretString::from(v.clone())))
            .collect();
        let resolved =
            ResolvedPath::with_secret(&dataset.dest, CloudSecret::new(spec.secret_type.clone(), params));
        fossil_runtime::install_secret(&conn, &resolved, "__fossil_mcp_dest")
            .map_err(|e| e.to_string())?;
    }

    let files: HashMap<String, Vec<u8>> = dataset
        .manifest_files
        .iter()
        .map(|(k, v)| (k.clone(), v.clone().into_bytes()))
        .collect();
    let manifest = Manifest::load(&MapSource(files)).map_err(|e| e.to_string())?;

    conn.execute_batch(&register_views_sql(&manifest, &dataset.dest))
        .map_err(|e| e.to_string())?;

    Ok((conn, manifest))
}

/// Dispatch one [`Operation`] against the dataset, returning the verb's JSON.
///
/// The native `DuckDB` executor is `!Send` (single-threaded by design), so the
/// whole synchronous setup+dispatch runs on a `spawn_blocking` thread — only
/// the `Send` JSON result crosses the await. The verb future never actually
/// suspends (the native executor returns ready values), so a noop-waker drive
/// completes it in one poll.
///
/// # Errors
///
/// Propagates setup or verb-execution errors as strings.
pub async fn dispatch_json(
    dataset: Dataset,
    operation: Operation,
) -> std::result::Result<Value, String> {
    tokio::task::spawn_blocking(move || {
        let (conn, manifest) = open(&dataset)?;
        let exec = fossil_runtime::DuckRuntime::new(&conn);
        block_on(fossil_graph::dispatch(&operation, &manifest, &exec)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("dispatch task panicked: {e}"))?
}

/// Drive a never-suspending future to completion synchronously (noop waker).
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

/// `CREATE OR REPLACE VIEW` per vertex/edge type, over the writer's Parquet
/// layout (`<dest>/vertex/<Type>.parquet`, `<dest>/edge/<table>/by_source.parquet`
/// — see `fossil-sinks::writer`). The view names match what the verbs query.
fn register_views_sql(manifest: &Manifest, dest: &str) -> String {
    use std::fmt::Write;
    let base = dest.trim_end_matches('/');
    let mut sql = String::new();
    for v in manifest.vertices() {
        let name = &v.vertex_type;
        let _ = writeln!(
            sql,
            "CREATE OR REPLACE VIEW {ident} AS SELECT * FROM read_parquet('{base}/vertex/{path}.parquet');",
            ident = quote_ident(name),
            path = escape_lit(name),
        );
    }
    for e in manifest.edges() {
        let table = edge_table_name(e);
        let _ = writeln!(
            sql,
            "CREATE OR REPLACE VIEW {ident} AS SELECT * FROM read_parquet('{base}/edge/{path}/by_source.parquet');",
            ident = quote_ident(&table),
            path = escape_lit(&table),
        );
    }
    sql
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn escape_lit(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset_json() -> Value {
        serde_json::json!({
            "dest": "s3://bucket/run123/",
            "secret": { "type": "s3", "params": { "KEY_ID": "k", "SECRET": "s" } },
            "manifest_files": { "graph.graph.yml": "..." }
        })
    }

    #[test]
    fn dataset_deserialises_with_secret() {
        let d: Dataset = serde_json::from_value(dataset_json()).unwrap();
        assert_eq!(d.dest, "s3://bucket/run123/");
        let spec = d.secret.expect("secret");
        assert_eq!(spec.secret_type, "s3");
        assert_eq!(spec.params.get("KEY_ID").map(String::as_str), Some("k"));
        assert!(d.manifest_files.contains_key("graph.graph.yml"));
    }

    #[test]
    fn dataset_deserialises_without_secret() {
        let d: Dataset = serde_json::from_value(serde_json::json!({
            "dest": "/tmp/local",
            "manifest_files": {}
        }))
        .unwrap();
        assert!(d.secret.is_none());
    }

    #[test]
    fn register_views_sql_uses_writer_layout() {
        use fossil_graph::manifest::ManifestSource as _;
        use fossil_sinks::manifest::{DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo};

        let mut person = VertexInfo::new(
            "Person",
            DEFAULT_CHUNK_SIZE,
            "vertex/Person/",
            vec![PropertyGroup {
                file_type: "parquet".into(),
                properties: vec![Property {
                    name: "dense_id".into(),
                    data_type: "uint32".into(),
                    is_primary: true,
                    is_nullable: Some(false),
                }],
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
        map.insert("graph.graph.yml".to_string(), graph.to_yaml().unwrap().into_bytes());
        map.insert("vertex/Person.vertex.yml".to_string(), person.to_yaml().unwrap().into_bytes());
        map.insert(
            "edge/Person_knows_Person/Person_knows_Person.edge.yml".to_string(),
            edge.to_yaml().unwrap().into_bytes(),
        );
        let _ = MapSource(HashMap::new()).fetch("x"); // touch the seam
        let manifest = Manifest::load(&MapSource(map)).unwrap();

        let sql = register_views_sql(&manifest, "s3://bucket/run/");
        assert!(sql.contains(
            "CREATE OR REPLACE VIEW \"Person\" AS SELECT * FROM read_parquet('s3://bucket/run/vertex/Person.parquet')"
        ));
        assert!(sql.contains(
            "CREATE OR REPLACE VIEW \"Person_knows_Person\" AS SELECT * FROM read_parquet('s3://bucket/run/edge/Person_knows_Person/by_source.parquet')"
        ));
    }
}
