//! **A cloud `@conn` source type-checks and cannot be run, and the two halves
//! use two engines.** Measured against a real S3 (`MinIO`), 2026-08-23.
//!
//! This is the asymmetry `docs/design/one-engine.mdx` had inferred from a grep
//! and could not confirm. It is confirmed:
//!
//! - **Introspection reaches it.** The host installs the connection's
//!   `CREATE SECRET` and asks `DESCRIBE SELECT * FROM read_csv_auto('s3://…')`
//!   through `DuckDB`. The columns come back typed — `id: Integer`,
//!   `name: String` — so the program type-checks against the columns the file
//!   actually has, not against a fallback.
//! - **Execution does not.** `DataFusion` has no object store registered for the
//!   scheme: `register_object_store` appears in exactly ONE file in this
//!   workspace and it is `fossil-df-wasm`, staging bytes the browser already
//!   fetched. `object_store` is not even a workspace dependency.
//!
//! Before this file, the second half surfaced as
//! `Internal error: No suitable object store found for s3://sales/users.csv.
//! See RuntimeEnv::register_object_store.` — an error naming a `DataFusion` API
//! and nothing the operator wrote.
//!
//! # What this file pins, and what it does not
//!
//! It pins **today**: that the refusal is a sentence about the program, and that
//! the introspection half really does work — because a test that only asserted
//! the failure would also pass if the source were unreachable for a dull reason
//! (bad credential, wrong endpoint), which is the opposite finding.
//!
//! It is NOT an argument that a cloud source should be refused. The day the
//! native host registers an object store this file goes red, which is the
//! correct way for it to fail.
//!
//! # Why it skips instead of failing without `MinIO`
//!
//! A real S3 is the only thing that can tell these two halves apart, and no
//! other test in this tree needs one. `FOSSIL_TEST_S3_ENDPOINT` (default
//! `127.0.0.1:9000`) is asked for its health first, and the test returns if
//! nothing answers. A skipped guard is worth having only because it says so out
//! loud — see the `eprintln!`.
//!
//! `text
//! docker run -d --name fossil-minio -p 9000:9000 \
//!   -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
//!   -v /tmp/fossil-minio-data:/data quay.io/minio/minio server /data
//! mkdir -p /tmp/fossil-minio-data/sales   # a top-level dir IS a bucket
//! `

#![cfg(not(target_arch = "wasm32"))]
// `{User.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
User := io.csv(\"@sales/users.csv\")
People : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name     = User.name
";

const DOCUMENT: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string
}
";

/// The endpoint, host:port and no scheme — which is the spelling `DuckDB`'s
/// `CREATE SECRET` takes.
fn endpoint() -> String {
    std::env::var("FOSSIL_TEST_S3_ENDPOINT").unwrap_or_else(|_| "127.0.0.1:9000".to_string())
}

/// Is something answering `MinIO`'s liveness probe? One connect, no retry: this
/// decides whether to run, not whether the server is healthy.
fn minio_live() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    endpoint()
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok())
}

/// The `--creds-stdin` payload a host would send for this connection. The four
/// `MinIO`-specific parameters ride through `ResolvedPath::create_secret_sql`
/// untouched — it renders whatever parameter names the payload carries, which
/// is what makes a non-AWS S3 reachable without this crate knowing about one.
fn creds_json() -> String {
    format!(
        r#"{{ "connections": {{ "sales": {{ "url": "s3://sales",
             "secret": {{ "type": "s3", "params": {{
               "KEY_ID": "minioadmin", "SECRET": "minioadmin",
               "ENDPOINT": "{}", "USE_SSL": "false", "URL_STYLE": "path" }} }} }} }} }}"#,
        endpoint()
    )
}

/// Put the CSV in the bucket, through `DuckDB` — a genuine S3 PUT rather than a
/// file dropped into `MinIO`'s data directory, which newer `MinIO` does not serve.
fn seed_bucket() {
    let c = duckdb::Connection::open_in_memory().expect("open duckdb");
    c.execute_batch("INSTALL httpfs; LOAD httpfs;")
        .expect("httpfs");
    c.execute_batch(&format!(
        "CREATE OR REPLACE SECRET seed (TYPE s3, KEY_ID 'minioadmin', SECRET 'minioadmin', \
         ENDPOINT '{}', USE_SSL false, URL_STYLE 'path', SCOPE 's3://sales');",
        endpoint()
    ))
    .expect("secret");
    c.execute_batch(
        "COPY (SELECT 1 AS id, 'Alice' AS name UNION ALL SELECT 2, 'Bob') \
         TO 's3://sales/users.csv' (FORMAT CSV, HEADER);",
    )
    .expect("seed the bucket");
}

#[test]
fn a_cloud_source_introspects_through_duckdb_and_the_run_refuses_it() {
    if !minio_live() {
        eprintln!(
            "SKIPPED: no S3 at {} — this is the only test that needs one; see this file's header",
            endpoint()
        );
        return;
    }
    seed_bucket();

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.path().join("person.shex"), DOCUMENT).expect("write document");
    let path = dir.path().join("prog.fossil");

    let creds = fossil_introspect::RunCreds::from_json(&creds_json()).expect("the payload parses");
    let urls = fossil_introspect::connection_urls(&creds.connections);
    let system = fossil_cli::host_system(&path);
    fossil_introspect::introspect_program(&*system, &path, &urls, &creds).expect("introspect");

    // HALF ONE: the DESCRIBE reached the bucket and came back with real types.
    // Asserted on the TYPES and not merely on the descriptor existing — an entry
    // with no columns is what a failed introspection would leave.
    let descriptor = system
        .descriptors()
        .and_then(|cache| cache.get("@sales/users.csv"))
        .expect("the cloud source is introspected under the URI the program wrote");
    let columns: Vec<(String, String)> = descriptor
        .columns
        .iter()
        .map(|c| (c.name.to_string(), format!("{:?}", c.primitive)))
        .collect();
    assert_eq!(
        columns,
        vec![
            ("id".to_string(), "Integer".to_string()),
            ("name".to_string(), "String".to_string()),
        ],
        "`DuckDB` read the object and typed its columns"
    );

    // HALF TWO: the run refuses, in a sentence about the program.
    let err = fossil_cli::run(
        &path,
        &dir.path().join("out").to_string_lossy(),
        &urls,
        None,
        None,
    )
    .expect_err("the DataFusion path has no object store for `s3://`");
    let message = format!("{err}");
    assert!(
        message.contains("s3://sales/users.csv"),
        "the refusal must name the source it refused; got `{message}`"
    );
    assert!(
        !message.contains("register_object_store"),
        "the operator must not be shown a DataFusion API as the explanation; got `{message}`"
    );
}
