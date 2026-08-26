//! fossil-mcp — the verb surface as a server-side service.
//!
//! `fossil-graph` owns the verb→SQL logic (WASM-clean); this crate runs it
//! NATIVELY: open a `DuckDB` connection, install the dataset's cloud secret,
//! register the `GraphAr` views the verbs query, load the manifest, and
//! [`fossil_graph::dispatch`] the operation via [`ConnectionExecutor`].
//!
//! The MCP transport (`main.rs`, `rmcp`) is a thin shell over [`dispatch_json`]:
//! the host (keasy) spawns this binary, calls the verb tool with a
//! [`Dataset`] + an [`fossil_graph::Operation`], and gets the verb's JSON
//! result. Secrets ride the MCP message on stdin (never argv/env), honouring
//! the same invariant as `fossil run --creds-stdin`.

use std::collections::HashMap;

pub mod executor;
pub use executor::ConnectionExecutor;

use duckdb::Connection;
use fossil_graph::executor::quote_ident;
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
    // No memory budget: a budget is an input of the command that declares it,
    // and an MCP dataset does not declare one. This named
    // `fossil_layout::apply_memory_budget` as what it would call when one does;
    // that function is deleted, because this comment was the only thing in the
    // workspace that mentioned it and nothing ever called it.
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;

    if let Some(spec) = &dataset.secret {
        let params: HashMap<String, SecretString> = spec
            .params
            .iter()
            .map(|(k, v)| (k.clone(), SecretString::from(v.clone())))
            .collect();
        let resolved = ResolvedPath::with_secret(
            &dataset.dest,
            CloudSecret::new(spec.secret_type.clone(), params),
        );
        // `fossil_layout::install_secret` stood here and was this crate's ONLY
        // use of `fossil-layout`, so the dependency is gone with it. The
        // rendering is `ResolvedPath::create_secret_sql`, in `fossil-resolver`,
        // which this crate already depends on and which is where the tests are.
        if let Some(sql) = resolved.create_secret_sql("__fossil_mcp_dest") {
            conn.execute_batch(&sql).map_err(|e| e.to_string())?;
        }
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
        let exec = ConnectionExecutor::new(&conn);
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
/// layout. The view names match what the verbs query.
///
/// # Vertices are tiles, and the manifest says where
///
/// This read `<dest>/vertex/<Type>.parquet` until 2026-08-06, which is a file
/// the writer **deletes**: `c416e07` made the layout pass emit one tile per
/// 4,096-row `dense_id` range and then remove the single staged file
/// (`crates/fossil-engine/src/lib.rs:494`). Every verb over a freshly written
/// corpus failed to find its vertices, and the test below asserted the stale
/// path, so nothing went red — the same commit left four call sites naming a
/// path that no longer exists.
///
/// The fix is not a new hard-coded string. `VertexInfo::prefix` is the
/// directory the writer declares it emitted into, so the reader asks the
/// manifest instead of re-deriving the convention; the day the tile naming
/// changes again, this does not.
///
/// Edges are unaffected: the layout pass rewrites `by_source.parquet` in place
/// (`crates/fossil-layout/src/layout.rs:630`) and emits its tiles alongside it.
fn register_views_sql(manifest: &Manifest, dest: &str) -> String {
    use std::fmt::Write;
    let base = dest.trim_end_matches('/');
    let mut sql = String::new();
    for v in manifest.vertices() {
        let name = &v.vertex_type;
        let dir = v.prefix.trim_end_matches('/');
        let _ = writeln!(
            sql,
            "CREATE OR REPLACE VIEW {ident} AS SELECT * FROM read_parquet('{base}/{path}/*.parquet');",
            ident = quote_ident(name),
            path = escape_lit(dir),
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

/// A `DuckDB` single-quoted string literal's INNARDS — the quotes are written
/// by the `format!` around it, which is what makes this different from
/// `fossil_graph::executor::sql_str_lit` and why it is not that function.
///
/// Its sibling `quote_ident` WAS a byte-identical copy of
/// [`fossil_graph::executor::quote_ident`], and is now a call: `fossil-mcp` already
/// depends on `fossil-graph`, so nothing but privacy held the copy in place.
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
        use fossil_sinks::manifest::{
            DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup, VertexInfo,
        };

        let mut person = VertexInfo::new(
            "Person",
            3,
            DEFAULT_CHUNK_SIZE,
            "vertex/Person/",
            vec![PropertyGroup {
                file_type: "parquet".into(),
                properties: vec![Property {
                    name: "dense_id".into(),
                    data_type: "uint32".into(),
                    // The address is never the primary. This fixture declares
                    // no identity column, so it declares no primary either.
                    is_primary: false,
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
            vec!["vertex/Person.vertex.yml".into()],
            vec!["edge/Person_knows_Person/Person_knows_Person.edge.yml".into()],
        );
        let mut map = HashMap::new();
        map.insert(
            "graph.graph.yml".to_string(),
            graph.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "vertex/Person.vertex.yml".to_string(),
            person.to_yaml().unwrap().into_bytes(),
        );
        map.insert(
            "edge/Person_knows_Person/Person_knows_Person.edge.yml".to_string(),
            edge.to_yaml().unwrap().into_bytes(),
        );
        let _ = MapSource(HashMap::new()).fetch("x"); // touch the seam
        let manifest = Manifest::load(&MapSource(map)).unwrap();

        let sql = register_views_sql(&manifest, "s3://bucket/run/");
        // The vertex view globs the tile directory the manifest declares, not a
        // single file. It asserted `vertex/Person.parquet` until 2026-08-06 —
        // a path the writer deletes — which is why this test stayed green while
        // every verb over a real corpus failed. It must read the fixture's
        // `prefix`, so a future change to the tile naming lands here as a diff.
        assert!(
            sql.contains(
                "CREATE OR REPLACE VIEW \"Person\" AS SELECT * FROM read_parquet('s3://bucket/run/vertex/Person/*.parquet')"
            ),
            "vertex view did not glob the declared prefix, got: {sql}"
        );
        assert!(sql.contains(
            "CREATE OR REPLACE VIEW \"Person_knows_Person\" AS SELECT * FROM read_parquet('s3://bucket/run/edge/Person_knows_Person/by_source.parquet')"
        ));
    }

    /// **The glob is one star, and the difference between one and two is a
    /// vertex type that reads its own index as payload.**
    ///
    /// Since `55f573e` a vertex type may publish an identity index, and it
    /// lives INSIDE that type's own prefix — `vertex/Person/index/tile{k}.parquet`,
    /// beside `vertex/Person/chunk{k}.parquet`. `*` matches one directory level
    /// and `**` recurses, so a reader that reached for the recursive form would
    /// union five payload tiles of five columns with five index tiles of two.
    ///
    /// The test above pins the SQL by string and would catch that as a diff,
    /// but it would report it as *"vertex view did not glob the declared
    /// prefix"* — which is not what went wrong, and the star is not the part of
    /// that string a reader's eye stops on. Its own comment records that it
    /// stayed green for months while every verb over a real corpus failed,
    /// because a string it asserted was a path the writer deletes. So this one
    /// asserts the NUMBER, against a corpus that actually has an index: the
    /// view is exactly the payload, and `vertex_count` is what says so.
    #[test]
    fn the_vertex_view_does_not_read_the_identity_index_as_payload() {
        use duckdb::Connection;
        use std::path::Path;

        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/corpus/conformance/corpus")
            .canonicalize()
            .expect("the conformance corpus is on disk");

        // Read the real manifests rather than build fixtures: the point is a
        // corpus whose `index:` block is the one `fossil run` writes.
        let mut map = HashMap::new();
        for rel in [
            "graph.graph.yml",
            "vertex/Person.vertex.yml",
            "edge/Person_knows_Person/Person_knows_Person.edge.yml",
        ] {
            map.insert(
                rel.to_string(),
                std::fs::read(corpus.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}")),
            );
        }
        let manifest = Manifest::load(&MapSource(map)).expect("the conformance manifest loads");

        // The fixture is only worth anything if it HAS one. A corpus that lost
        // its index would make every assertion below pass on nothing.
        assert!(
            manifest.vertices()[0].index.is_some(),
            "the conformance corpus declares no index, so this test proves nothing",
        );

        let conn = Connection::open_in_memory().expect("open duckdb");
        conn.execute_batch(&register_views_sql(&manifest, corpus.to_str().unwrap()))
            .expect("the views register over the corpus on disk");

        let rows: u64 = conn
            .query_row("SELECT count(*) FROM \"Person\"", [], |r| r.get(0))
            .expect("the vertex view is readable");
        let declared = manifest.vertices()[0].vertex_count;
        assert_eq!(
            rows, declared,
            "the vertex view reads {rows} rows where the manifest declares {declared}; \
             a recursive glob would add the index tiles to the payload",
        );

        // And the schema is the payload's. A union with a two-column index
        // would show up here even if the row count happened to survive it.
        let columns: u64 = conn
            .query_row(
                "SELECT count(*) FROM (DESCRIBE SELECT * FROM \"Person\")",
                [],
                |r| r.get(0),
            )
            .expect("describe the vertex view");
        assert_eq!(
            columns, 5,
            "Person's payload is dense_id, subject, x, y, cluster_id"
        );
    }
}
