//! fossil-mcp — the verb surface as a server-side service.
//!
//! `fossil-graph` owns the verb→SQL logic (WASM-clean); this crate runs it
//! NATIVELY: open a `DuckDB` connection, install the dataset's cloud secret,
//! register the `GraphAr` views the verbs query, load the manifest, and
//! [`fossil_graph::dispatch`] the operation via [`ConnectionExecutor`].
//!
//! The MCP transport (`main.rs`, `rmcp`) is a thin shell over [`dispatch_json`]:
//! the host spawns this binary against one [`Dataset`], the six typed tools of
//! [`tools`] turn a tool call into an [`fossil_graph::Operation`], and the
//! caller gets the verb's JSON result. The dataset — including its secret —
//! comes from the file `--dataset` names, never from argv or the environment,
//! honouring the same invariant as `fossil run --creds-stdin`.

use std::collections::HashMap;

pub mod executor;
pub mod tools;
pub use executor::ConnectionExecutor;

use duckdb::Connection;
use fossil_graph::executor::quote_ident;
use fossil_graph::manifest::{Manifest, ManifestSource, edge_table_name};
use fossil_graph::{GraphError, Operation, Result as GraphResult};
use fossil_resolver::{CloudSecret, ResolvedPath};
use fossil_sinks::manifest::{Container, TILES_FILE};
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
/// # Every path is composed, and none is discovered
///
/// This globbed `<prefix>/*.parquet` for a vertex type and read
/// `by_source.parquet` for an edge, and neither is an address. Expanding a
/// wildcard is listing a directory — the one operation the corpus is designed so
/// that a reader never has to perform — and `by_source.parquet` was the uncut
/// relation published beside its own tiles, so `fossil-mcp` and every other
/// reader disagreed about where a corpus's bytes are. It is not there now: the
/// layout pass emits tiles and the caller deletes what it read.
///
/// So the paths come out of the manifest, which carries the two things that
/// decide them: `container` says whether a tile is a file or a row group, and
/// `prefix` says where the set is. Under [`Container::RowGroups`] a set is one
/// file and there is nothing to enumerate. Under [`Container::Files`] a vertex
/// type's tiles are enumerated from `vertex_count` and `chunk_size` — the count
/// is declared precisely so that nobody has to list — and an edge orientation's
/// are not, because a tile whose vertices have no edges is a file that was never
/// written and a run of them is a gap no arithmetic predicts. That one stays a
/// glob, and it is the only one.
///
/// It read `<dest>/vertex/<Type>.parquet` until 2026-08-06, a file the writer
/// deletes, and the test below asserted the stale path so nothing went red.
/// Hence the second test, which asserts a row count against a corpus on disk
/// rather than a string.
fn register_views_sql(manifest: &Manifest, dest: &str) -> String {
    use std::fmt::Write;
    let base = dest.trim_end_matches('/');
    let container = manifest.graph().container;
    let mut sql = String::new();
    for v in manifest.vertices() {
        let prefix = format!("{base}/{}", v.prefix.trim_end_matches('/'));
        let source = match container {
            Container::RowGroups => sql_str_lit(&format!("{prefix}/{TILES_FILE}")),
            Container::Files => {
                let tiles = if v.chunk_size == 0 {
                    0
                } else {
                    v.vertex_count.div_ceil(v.chunk_size)
                };
                sql_list((0..tiles).map(|k| format!("{prefix}/chunk{k}.parquet")))
            }
        };
        let _ = writeln!(
            sql,
            "CREATE OR REPLACE VIEW {ident} AS SELECT * FROM read_parquet({source});",
            ident = quote_ident(&v.vertex_type),
        );
    }
    for e in manifest.edges() {
        // The source-ordered orientation, because that is the one a verb reads
        // the whole relation from; `aligned_by` is what says which prefix that
        // is, and an edge type that publishes no source-ordered adjacency gets
        // no view rather than a composed path that 404s.
        let Some(adj) = e.adj_lists.iter().find(|a| a.aligned_by == "src") else {
            continue;
        };
        let prefix = format!(
            "{base}/{}{}",
            e.prefix.trim_end_matches('/').to_string() + "/",
            adj.prefix.trim_end_matches('/')
        );
        let source = match container {
            Container::RowGroups => sql_str_lit(&format!("{prefix}/{TILES_FILE}")),
            // A tile with no rows is not written, so the set of tile numbers is
            // a property of the data and not of the count. Nothing composes it.
            Container::Files => sql_str_lit(&format!("{prefix}/tile*.parquet")),
        };
        let _ = writeln!(
            sql,
            "CREATE OR REPLACE VIEW {ident} AS SELECT * FROM read_parquet({source});",
            ident = quote_ident(&edge_table_name(e)),
        );
    }
    sql
}

/// One path as a `DuckDB` string literal.
fn sql_str_lit(path: &str) -> String {
    format!("'{}'", escape_lit(path))
}

/// Several paths as a `DuckDB` list literal, which `read_parquet` takes as the
/// enumerated set it is — no wildcard, so nothing is expanded against a
/// directory. An empty set reads as an empty relation, which is what a type
/// declaring zero vertices is.
fn sql_list(paths: impl Iterator<Item = String>) -> String {
    let items: Vec<String> = paths.map(|p| sql_str_lit(&p)).collect();
    format!("[{}]", items.join(", "))
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
            AdjList, Container, DEFAULT_CHUNK_SIZE, EdgeInfo, GraphInfo, Property, PropertyGroup,
            VertexInfo,
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
            // The source-ordered orientation, because the view reads the whole
            // relation and `aligned_by` is what says which prefix that is. This
            // was `vec![]` while the path was the hard-coded `by_source.parquet`
            // — a fixture that declared no adjacency and got a view over one.
            adj_lists: vec![AdjList {
                ordered: true,
                aligned_by: "src".into(),
                prefix: "by_source/".into(),
                file_type: "parquet".into(),
            }],
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
        // One file per set, and no star anywhere. It asserted
        // `vertex/Person.parquet` until 2026-08-06 — a path the writer deletes —
        // which is why this test stayed green while every verb over a real
        // corpus failed, and then `vertex/Person/*.parquet`, which is a
        // directory listing spelled as a path. Both paths below come out of the
        // manifest's `prefix` and its `container`.
        assert!(
            sql.contains(
                "CREATE OR REPLACE VIEW \"Person\" AS SELECT * FROM read_parquet('s3://bucket/run/vertex/Person/tiles.parquet')"
            ),
            "vertex view did not address the declared prefix, got: {sql}"
        );
        assert!(sql.contains(
            "CREATE OR REPLACE VIEW \"Person_knows_Person\" AS SELECT * FROM read_parquet('s3://bucket/run/edge/Person_knows_Person/by_source/tiles.parquet')"
        ), "edge view did not address the declared adjacency prefix, got: {sql}");
        assert!(
            !sql.contains("*.parquet"),
            "the row-group container has nothing to expand, got: {sql}"
        );
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

        // And the edge view, which is the half decision 12 was written about:
        // this read `by_source.parquet`, the uncut relation the corpus published
        // beside its own tiles, and that file is gone. `DuckDB` binds a view
        // early, so a path that names nothing fails at `CREATE VIEW` — which is
        // why the assertion above already proves the edge path resolves, and why
        // this one only has to prove it resolves to the RELATION rather than to
        // one tile of it.
        let edges: u64 = conn
            .query_row("SELECT count(*) FROM \"Person_knows_Person\"", [], |r| {
                r.get(0)
            })
            .expect("the edge view is readable");
        let declared = manifest.edges()[0].edge_count;
        assert_eq!(
            edges, declared,
            "the edge view reads {edges} rows where the manifest declares {declared}",
        );
    }

    /// **How much of this binding still hands the engine a wildcard, counted.**
    ///
    /// A glob is the one thing the addressing rule forbids — expanding a
    /// wildcard IS listing a directory, and there is no listing over HTTP — and
    /// this function is the last place in the read path that emits one. The
    /// number is the claim, so it is measured here for both containers over the
    /// same corpus rather than argued in a comment:
    ///
    /// - **`rowgroups`: zero.** Every payload set is one file named by the
    ///   manifest's own prefix, so there is nothing to expand.
    /// - **`files`: one per edge orientation, and none anywhere else.** A
    ///   vertex type's tiles are composed from `vertex_count` and `chunk_size`.
    ///   An edge orientation's are not, and cannot be: a tile whose vertices
    ///   have no edges is a file that was never written, and a run of them is a
    ///   gap no arithmetic predicts.
    ///
    /// **What bounds the exposure is which container fossil writes**, and that
    /// was measured rather than read off a call site: `fossil run
    /// examples/hello.fossil` emits `container: rowgroups`, and
    /// `crates/fossil-df/src/lib.rs` has the one `GraphInfo::new` call that
    /// decides it, passing [`Container::RowGroups`] unconditionally. So **a
    /// corpus fossil wrote registers no wildcard at all.**
    ///
    /// The fixture below is the other one on purpose. `apps/corpus`'s
    /// conformance corpus is `container: files` — it is written through `DuckDB`,
    /// which clamps a row group under its 2,048-row vector, so a `chunk_size`
    /// 64 corpus cannot be in the row-group container — which makes it the
    /// corpus that actually HAS the glob, and therefore the honest fixture.
    ///
    /// What this does NOT claim is that the remaining glob is cheap. Left to
    /// the engine, an edge view over a `files` corpus reads whatever the
    /// expansion returns, and nothing here measures that. Closing it needs a
    /// per-orientation tile census in the manifest — a format change, not a
    /// binding change — and until somebody writes a `files` corpus that this
    /// server is pointed at, the cost is a cost nobody is paying.
    #[test]
    fn the_only_wildcard_left_is_an_edge_orientation_in_the_container_fossil_does_not_write() {
        use fossil_graph::manifest::GRAPH_INFO_PATH;
        use fossil_sinks::manifest::Container;
        use std::path::Path;

        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/corpus/conformance/corpus")
            .canonicalize()
            .expect("the conformance corpus is on disk");

        let read = |rel: &str| {
            String::from_utf8(std::fs::read(corpus.join(rel)).expect("read a manifest"))
                .expect("utf8")
        };
        let graph_yaml = read(GRAPH_INFO_PATH);
        // The conformance corpus is the `files` one, and that is not an
        // accident: it is written through DuckDB, which clamps a row group
        // smaller than its 2,048-row vector, so a `chunk_size` 64 corpus cannot
        // be in the other container (`crates/fossil-df/src/files.rs`). Which
        // makes it the right fixture for this measurement — it is the corpus
        // that HAS the glob.
        assert!(
            graph_yaml.contains("container: files"),
            "the fixture is not the container this test is about: {graph_yaml}",
        );

        let load = |container: Container| {
            let yaml = match container {
                Container::Files => graph_yaml.clone(),
                Container::RowGroups => {
                    graph_yaml.replace("container: files", "container: rowgroups")
                }
            };
            let mut map = HashMap::new();
            map.insert(GRAPH_INFO_PATH.to_string(), yaml.into_bytes());
            for rel in [
                "vertex/Person.vertex.yml",
                "edge/Person_knows_Person/Person_knows_Person.edge.yml",
            ] {
                map.insert(rel.to_string(), read(rel).into_bytes());
            }
            let manifest = Manifest::load(&MapSource(map)).expect("the manifest loads");
            assert_eq!(manifest.graph().container, container);
            register_views_sql(&manifest, corpus.to_str().unwrap())
        };

        // Count stars in the PATHS, not in the statement: every view projects
        // `SELECT *`, which is a star this measurement is not about. What the
        // engine expands is the argument of `read_parquet`.
        let globbed = |sql: &str| -> Vec<String> {
            sql.lines()
                .filter_map(|line| {
                    let open = line.find("read_parquet(")? + "read_parquet(".len();
                    let close = line.rfind(')')?;
                    line.get(open..close)
                        .filter(|paths| paths.contains('*'))
                        .map(|_| line.to_string())
                })
                .collect()
        };

        let rowgroups = load(Container::RowGroups);
        assert!(
            globbed(&rowgroups).is_empty(),
            "the container fossil writes composes every path; got: {rowgroups}",
        );

        let files = load(Container::Files);
        let globs = globbed(&files);
        assert_eq!(
            globs.len(),
            1,
            "the fixture has one source-ordered adjacency, so it bounds the count at one; got: {files}",
        );
        assert!(
            globs[0].contains("Person_knows_Person") && globs[0].contains("tile*.parquet"),
            "the surviving wildcard is not an edge orientation; got: {}",
            globs[0],
        );
    }
}
