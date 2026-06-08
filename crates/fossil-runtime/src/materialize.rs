//! `GraphAr` materialiser — executes a [`fossil_sinks::writer::WriteSqlPlan`]
//! against a `DuckDB` connection and writes the companion manifests via a
//! caller-supplied YAML writer.
//!
//! This is W0b/4: the native execution side of the writer split announced
//! in W0b/1 (`crates/fossil-sinks/src/writer.rs`). The planning side
//! (fossil-sinks) is WASM-clean; the execution side (here) is native-only
//! and lives behind fossil-runtime's wasm32 `compile_error!` tripwire.
//!
//! ## API shape — why a closure for YAML writes
//!
//! [`materialize`] takes a `write_yaml` `FnMut(rel_path, content)`
//! callback rather than an explicit trait + impls because:
//!
//! 1. The two real hosts have very different YAML write paths. A native
//!    CLI writes to a `tempdir` then uploads via the same `ResolvedPath`
//!    plumbing; a Keasy server proxy writes to S3/Azure via its own
//!    storage abstraction. A trait would force both hosts to allocate an
//!    object with the same vtable shape — a closure lets each host pass
//!    a captured-environment function.
//! 2. Tests need a one-line in-memory writer (`|p, c| { map.insert(p, c); }`).
//!    Closures make that trivial; a trait would need a test-only impl.
//!
//! The Parquet writes go through `DuckDB COPY` directly — no callback —
//! because `DuckDB` owns the byte-write path per ADR-0017.
//!
//! ## Larger-than-RAM honesty (W0b/4)
//!
//! The materialiser installs the dest's scoped `CREATE SECRET` into the
//! connection BEFORE the COPY statements run. For cloud destinations the
//! secret configures `DuckDB`'s HTTP/S3/Azure extension; the COPY then
//! streams data directly cloud↔cloud without ever materialising the Parquet
//! in the runtime process's memory. Local-file destinations carry no secret
//! and `DuckDB` writes through its native filesystem path.

use duckdb::Connection;
use fossil_resolver::ResolvedPath;
use fossil_sinks::writer::{ManifestSet, WriteSqlPlan};

/// Errors materialising a write plan.
///
/// `Source` and `Yaml` carry stringified causes so callers (CLI miette,
/// server JSON envelope, future browser host) translate to their native
/// error shape without depending on this crate's underlying types.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    /// Installing the scoped `CREATE SECRET` failed before any COPY ran.
    /// Usually means a secret parameter the resolver supplied is rejected by
    /// `DuckDB` (typo, missing extension).
    #[error("installing cloud secret `{name}` failed: {source}")]
    Secret { name: String, source: duckdb::Error },
    /// A vertex COPY statement failed.
    #[error("vertex `{type_name}` COPY failed: {source}")]
    VertexCopy {
        type_name: String,
        source: duckdb::Error,
    },
    /// An edge COPY (CSR or CSC) failed.
    #[error("edge `{edge_dir}` {orientation} COPY failed: {source}")]
    EdgeCopy {
        edge_dir: String,
        /// Either `"by_source"` (CSR) or `"by_target"` (CSC) — surfaces
        /// which of the two failed so the host's error message points
        /// at the right file.
        orientation: &'static str,
        source: duckdb::Error,
    },
    /// The host's `write_yaml` callback returned an error. Carries a
    /// stringified message — the host knows its own typed error shape
    /// and surfaces it before we see it.
    #[error("manifest YAML write failed for `{rel_path}`: {message}")]
    YamlWrite { rel_path: String, message: String },
}

/// Execute a write plan against the given `DuckDB` connection and write
/// the companion manifests via `write_yaml`.
///
/// Execution order:
///   1. Install the dest's scoped `CREATE SECRET` (skipped when the resolver
///      supplied no secret — local-file destinations).
///   2. Run each `vertex_statements[i].copy_sql` in declared order.
///   3. Run each `edge_statements[i].copy_csr_sql` then `copy_csc_sql`.
///   4. Call `write_yaml(manifest.rel_path, &manifest.yaml)` for every
///      vertex + edge manifest.
///
/// Vertex COPYs MUST complete before edge COPYs — the edge JOINs read
/// the freshly-written vertex Parquets to resolve `dense_id`. The order
/// is encoded here so callers can't accidentally swap the phases.
///
/// # Errors
///
/// Returns [`MaterializeError`] at the first failure; no rollback (the
/// caller is expected to drop the partially-written destination on error
/// — typical pattern is to materialise to a tempdir then rename / upload
/// only on success).
pub fn materialize<F>(
    conn: &Connection,
    plan: &WriteSqlPlan,
    manifests: &ManifestSet,
    dest: &ResolvedPath,
    mut write_yaml: F,
) -> Result<(), MaterializeError>
where
    F: FnMut(&str, &str) -> Result<(), String>,
{
    install_secret(conn, dest, "__fossil_dest")?;

    for stmt in &plan.vertex_statements {
        conn.execute_batch(&stmt.copy_sql)
            .map_err(|source| MaterializeError::VertexCopy {
                type_name: stmt.type_name.clone(),
                source,
            })?;
    }

    for stmt in &plan.edge_statements {
        conn.execute_batch(&stmt.copy_csr_sql)
            .map_err(|source| MaterializeError::EdgeCopy {
                edge_dir: stmt.edge_dir_name.clone(),
                orientation: "by_source",
                source,
            })?;
        conn.execute_batch(&stmt.copy_csc_sql)
            .map_err(|source| MaterializeError::EdgeCopy {
                edge_dir: stmt.edge_dir_name.clone(),
                orientation: "by_target",
                source,
            })?;
    }

    // Graph info first — it's the aggregate index the query side reads to
    // discover the per-type manifests it then resolves.
    write_yaml(&manifests.graph.rel_path, &manifests.graph.yaml).map_err(|message| {
        MaterializeError::YamlWrite {
            rel_path: manifests.graph.rel_path.clone(),
            message,
        }
    })?;
    for m in &manifests.vertices {
        write_yaml(&m.rel_path, &m.yaml).map_err(|message| MaterializeError::YamlWrite {
            rel_path: m.rel_path.clone(),
            message,
        })?;
    }
    for m in &manifests.edges {
        write_yaml(&m.rel_path, &m.yaml).map_err(|message| MaterializeError::YamlWrite {
            rel_path: m.rel_path.clone(),
            message,
        })?;
    }

    Ok(())
}

/// Install a [`ResolvedPath`]'s scoped `CREATE SECRET` on a `DuckDB` connection
/// under the given `name`, BEFORE any `read_*`/`COPY` that dereferences a cloud
/// URL under it. A no-op when the path carries no secret (local / public URLs).
/// Shared by [`materialize`] (dest writes) and host callers that read cloud
/// sources (e.g. the CLI's `@conn/path` source resolution), so secret rendering
/// lives in exactly one place ([`ResolvedPath::create_secret_sql`]).
///
/// The secret is scoped to the path's URL, so installing one per connection
/// lets a single job span distinct cloud accounts with no collision — `DuckDB`
/// applies each by longest-prefix scope match.
///
/// # Errors
///
/// Returns [`MaterializeError::Secret`] if `DuckDB` rejects the statement.
pub fn install_secret(
    conn: &Connection,
    path: &ResolvedPath,
    name: &str,
) -> Result<(), MaterializeError> {
    if let Some(sql) = path.create_secret_sql(name) {
        conn.execute_batch(&sql)
            .map_err(|source| MaterializeError::Secret {
                name: name.to_string(),
                source,
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};

    use super::*;
    use fossil_sinks::writer::{EdgeSpec, VertexSpec, WriteOptions, plan_manifests, plan_writes};
    use tempfile::TempDir;

    /// End-to-end: seed two raw tables in `DuckDB`, plan + materialise a
    /// two-vertex/one-edge `GraphAr` to a tempdir, then read back the
    /// resulting Parquet files and assert the W0b column shape lands
    /// correctly. This is the W0b/4 acceptance test — it proves the
    /// vertex-side `row_number() OVER ()` streams a `dense_id` 0..N-1,
    /// the edge-side JOIN resolves IRIs to dense indices, and the
    /// manifests get written to the right `rel_paths`.
    #[allow(clippy::too_many_lines)] // integration test — naturally long
    #[test]
    fn materialize_two_vertices_and_one_edge_to_tempdir() {
        let dir = TempDir::new().expect("tempdir");
        let dest_url = format!("file://{}", dir.path().display());

        // Pre-create the vertex/edge subdirectories so DuckDB COPY
        // doesn't need its own mkdir (the angelip2303 writer relied on
        // its host's mkdir; we mirror the contract).
        std::fs::create_dir_all(dir.path().join("vertex")).unwrap();
        std::fs::create_dir_all(dir.path().join("edge/person_works_at_org")).unwrap();

        let conn = Connection::open_in_memory().expect("duckdb");

        // Seed raw inputs: 3 persons (with a duplicate to exercise
        // dedup_subjects), 2 orgs, 3 works_at edges (2 unique persons,
        // both targeting Acme).
        conn.execute_batch(
            "CREATE TABLE raw_person (subject VARCHAR, name VARCHAR, age VARCHAR);
             INSERT INTO raw_person VALUES
               ('urn:p:alice', 'Alice', '30'),
               ('urn:p:bob',   'Bob',   '25'),
               ('urn:p:alice', 'Alice', '30');
             CREATE TABLE raw_org (subject VARCHAR, legal_name VARCHAR);
             INSERT INTO raw_org VALUES
               ('urn:o:acme', 'Acme Corp'),
               ('urn:o:beta', 'Beta Inc.');
             CREATE TABLE raw_edge (src_iri VARCHAR, dst_iri VARCHAR);
             INSERT INTO raw_edge VALUES
               ('urn:p:alice', 'urn:o:acme'),
               ('urn:p:bob',   'urn:o:acme');",
        )
        .unwrap();

        let vertices = vec![
            VertexSpec {
                name: "person".to_string(),
                iri: "http://example.org/Person".to_string(),
                source_relation: "SELECT subject, name, age FROM raw_person".to_string(),
                property_columns: vec!["name".to_string(), "age".to_string()],
                dedup_subjects: true,
                column_iris: BTreeMap::new(),
            },
            VertexSpec {
                name: "org".to_string(),
                iri: "http://example.org/Org".to_string(),
                source_relation: "SELECT subject, legal_name FROM raw_org".to_string(),
                property_columns: vec!["legal_name".to_string()],
                dedup_subjects: true,
                column_iris: BTreeMap::new(),
            },
        ];
        let edges = vec![EdgeSpec {
            label: "works_at".to_string(),
            iri: "http://example.org/worksAt".to_string(),
            source_type: "person".to_string(),
            target_type: "org".to_string(),
            source_relation: "SELECT src_iri, dst_iri FROM raw_edge".to_string(),
        }];

        let opts = WriteOptions::default();
        let plan = plan_writes(&vertices, &edges, &dest_url, &opts).unwrap();
        let manifests = plan_manifests(&vertices, &edges, &opts).unwrap();
        let dest = ResolvedPath::new(dest_url);

        // RefCell-wrapped sink so the FnMut closure can mutate it
        // across calls — pattern the materialize doc comment outlines.
        let yamls: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
        materialize(&conn, &plan, &manifests, &dest, |p, c| {
            yamls.borrow_mut().insert(p.to_string(), c.to_string());
            Ok(())
        })
        .expect("materialize");

        // Vertex Parquet exists + has the W0b column shape + dedup ran.
        let path_person = dir.path().join("vertex/person.parquet");
        assert!(
            path_person.exists(),
            "vertex parquet missing: {path_person:?}"
        );
        let count: u32 = conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM read_parquet('{}')",
                    path_person.display()
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "dedup_subjects must collapse the alice duplicate");

        // dense_id is 0..N-1 sequential.
        let max_dense: u32 = conn
            .query_row(
                &format!(
                    "SELECT max(dense_id) FROM read_parquet('{}')",
                    path_person.display()
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(max_dense, 1, "dense_id must be N-1 for N=2");

        // Layout placeholder columns are present and zero.
        let (x_sum, y_sum, c_sum): (f32, f32, u32) = conn
            .query_row(
                &format!(
                    "SELECT sum(x), sum(y), sum(cluster_id) FROM read_parquet('{}')",
                    path_person.display()
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (x_sum, y_sum, c_sum),
            (0.0, 0.0, 0),
            "W0b placeholders are 0"
        );

        // Edge CSR + CSC: 2 edges each, src_dense ∈ {0, 1}, dst_dense ∈ {0, 1}.
        for f in [
            "edge/person_works_at_org/by_source.parquet",
            "edge/person_works_at_org/by_target.parquet",
        ] {
            let p = dir.path().join(f);
            assert!(p.exists(), "edge parquet missing: {p:?}");
            let n: u32 = conn
                .query_row(
                    &format!("SELECT count(*) FROM read_parquet('{}')", p.display()),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(n, 2, "{f}: expected 2 edges, got {n}");
        }

        // Manifests landed at the expected rel_paths.
        let written = yamls.borrow();
        assert!(written.contains_key("vertex/person.vertex.yml"));
        assert!(written.contains_key("vertex/org.vertex.yml"));
        assert!(written.contains_key("edge/person_works_at_org/person_works_at_org.edge.yml",));
        assert!(
            written["vertex/person.vertex.yml"].contains("name: dense_id"),
            "manifest YAML must declare dense_id"
        );
    }

    /// Yaml write failures bubble up as `MaterializeError::YamlWrite`.
    #[test]
    fn yaml_write_failure_propagates() {
        let dir = TempDir::new().unwrap();
        let dest_url = format!("file://{}", dir.path().display());
        std::fs::create_dir_all(dir.path().join("vertex")).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE raw_person (subject VARCHAR, name VARCHAR);
             INSERT INTO raw_person VALUES ('urn:p:a', 'A');",
        )
        .unwrap();

        let vertices = vec![VertexSpec {
            name: "person".to_string(),
            iri: String::new(),
            source_relation: "SELECT subject, name FROM raw_person".to_string(),
            property_columns: vec!["name".to_string()],
            dedup_subjects: false,
            column_iris: BTreeMap::new(),
        }];
        let plan = plan_writes(&vertices, &[], &dest_url, &WriteOptions::default()).unwrap();
        let manifests = plan_manifests(&vertices, &[], &WriteOptions::default()).unwrap();
        let dest = ResolvedPath::new(dest_url);

        let err = materialize(&conn, &plan, &manifests, &dest, |_, _| {
            Err("disk full (simulated)".to_string())
        })
        .unwrap_err();
        match err {
            MaterializeError::YamlWrite { rel_path, message } => {
                // Graph info is written first (the aggregate index), so it's
                // the first write_yaml call and thus the one that fails here.
                assert_eq!(rel_path, "graph.graph.yml");
                assert!(message.contains("disk full"));
            }
            other => panic!("expected YamlWrite, got {other:?}"),
        }
    }
}
