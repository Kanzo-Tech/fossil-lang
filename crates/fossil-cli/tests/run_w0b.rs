// The embedded `.fossil` fixtures use template syntax (`${ex:}…/${.id}`) that
// clippy mistakes for format args in a plain string literal — they are not.
#![allow(clippy::literal_string_with_formatting_args)]

//! `fossil run --dest <url>` W0b path integration test.
//!
//! Drives the canonical `examples/hello.fossil` through the W0b writer +
//! materializer. The output descriptor is program-resident (synthesised from
//! the typed mapping — no `--shape`), so the run produces the W0b column shape
//! (`dense_id` / `subject` / `x` / `y` / `cluster_id` on vertices) from the
//! program alone, and the `--output-json` switch emits a parseable status
//! object on stdout.
//!
//! The `DuckDB` bundled-build cost (~75-85s cold) is incurred once and
//! amortised across the test cases here.

#![cfg(not(target_arch = "wasm32"))]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "fossil-cli", "--bin", "fossil"])
            .status()
            .expect("spawn cargo build");
        assert!(status.success(), "cargo build -p fossil-cli failed");
        let bin = repo_root().join("target").join("debug").join("fossil");
        assert!(bin.exists(), "fossil binary missing at {}", bin.display());
        bin
    })
}

/// Per-test workdir with the canonical hello.fossil + users.csv.
fn fresh_workdir(test_name: &str) -> PathBuf {
    let root = repo_root();
    let tmp = std::env::temp_dir().join(format!("fossil-cli-w0b-{test_name}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("examples")).expect("create examples subdir");
    // The CLI reads `examples/users.csv` (the io.csv binding in
    // hello.fossil resolves to this path).
    for f in ["hello.fossil", "users.csv"] {
        std::fs::copy(root.join("examples").join(f), tmp.join("examples").join(f))
            .unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    tmp
}

/// Acceptance: the W0b path produces `GraphAr` Parquet + YAML manifests
/// under --dest, with the W0b vertex column shape declared in the
/// vertex.yml manifest. The vertex type `Person` is derived from the mapping's
/// shape IRI (`ex:Person`) — no `--shape` needed.
#[test]
fn run_w0b_writes_graph_ar_under_dest() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("dest");
    let dest = workdir.join("graph");
    let dest_url = format!("file://{}", dest.display());

    let output = Command::new(bin)
        .args(["run", "examples/hello.fossil", "--dest", &dest_url])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil run");

    assert!(
        output.status.success(),
        "fossil run W0b exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The Person vertex Parquet is the W0b artefact.
    let vertex_parquet = dest.join("vertex/Person.parquet");
    assert!(
        vertex_parquet.exists(),
        "vertex Parquet missing at {}",
        vertex_parquet.display()
    );

    // W3.1b: the layout pass must have replaced the placeholder x/y (0 for every
    // vertex) with real coordinates — at least one vertex now carries a non-zero
    // coordinate.
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let nonzero: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM read_parquet('{}') WHERE x <> 0 OR y <> 0",
                vertex_parquet.display().to_string().replace('\'', "''")
            ),
            [],
            |r| r.get(0),
        )
        .expect("query layout x/y");
    assert!(
        nonzero > 0,
        "W3.1b layout must populate non-zero x/y (placeholder was 0); got {nonzero} non-zero rows",
    );

    let vertex_yaml = dest.join("vertex/Person.vertex.yml");
    assert!(
        vertex_yaml.exists(),
        "vertex YAML missing at {}",
        vertex_yaml.display()
    );
    let yaml_text = std::fs::read_to_string(&vertex_yaml).expect("read vertex.yml");
    // W0b column shape declared.
    assert!(
        yaml_text.contains("name: dense_id"),
        "vertex YAML must declare dense_id; got:\n{yaml_text}"
    );
    assert!(
        yaml_text.contains("name: x"),
        "vertex YAML must declare layout placeholder `x`; got:\n{yaml_text}"
    );
    assert!(
        yaml_text.contains("name: cluster_id"),
        "vertex YAML must declare cluster_id placeholder; got:\n{yaml_text}"
    );
    assert!(
        yaml_text.contains("version: gar/v1"),
        "vertex YAML must carry GraphAr v1 spec version; got:\n{yaml_text}"
    );
}

/// `--output-json` emits a status object that keasy can parse via
/// `serde_json::from_str` after `Command::output()`.
#[test]
fn run_w0b_output_json_is_parseable() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("json");
    let dest = workdir.join("graph");
    let dest_url = format!("file://{}", dest.display());

    let output = Command::new(bin)
        .args([
            "run",
            "examples/hello.fossil",
            "--dest",
            &dest_url,
            "--output-json",
        ])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil run");

    assert!(
        output.status.success(),
        "exit {}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("--output-json must emit a single JSON object; got: {stdout:?} (parse error: {e})")
    });
    assert_eq!(parsed["dest"], serde_json::Value::String(dest_url));
    assert!(
        parsed["vertices"].is_array(),
        "json.vertices must be an array; got: {parsed}"
    );
    // Each vertex carries its type, file, row count, and property columns — the
    // structure the keasy host consumes (it has no DuckDB to re-introspect with).
    let first = &parsed["vertices"][0];
    assert_eq!(
        first["type"].as_str(),
        Some("Person"),
        "vertex.type expected; got: {first}"
    );
    assert!(
        first["file"]
            .as_str()
            .is_some_and(|f| f.starts_with("vertex/")),
        "vertex.file rel_path expected; got: {first}"
    );
    assert_eq!(
        first["count"].as_i64(),
        Some(5),
        "hello.fossil writes 5 Persons (examples/users.csv); got: {first}"
    );
    assert!(
        first["columns"]
            .as_array()
            .is_some_and(|c| c.iter().any(|col| col["name"] == "name")),
        "vertex.columns must include the `name` property; got: {first}"
    );
}

/// A workdir seeded with arbitrary `(rel_path, contents)` files (for the
/// multi-mapping fixture below, which is not part of the canonical examples).
fn workdir_with_files(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("fossil-cli-w0b-{test_name}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create workdir");
    for (rel, contents) in files {
        let path = tmp.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture subdir");
        }
        std::fs::write(&path, contents).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }
    tmp
}

/// Slice 8 end-to-end: a TWO-mapping program with NO `--shape`. The synthesised
/// (`AcceptAll`) descriptor must (a) materialise BOTH vertex types and (b) turn
/// `Order.ex:placedBy = ${ex:}person/${.user_id}` into a cross-type edge to the
/// `Person` mapping (same subject-template skeleton `${ex:}person/${.id}`), whose
/// CSR Parquet joins the order subjects to the person subjects. Proves Phase B
/// edge synthesis (8a) + multi-mapping merge/materialisation (8b) on real `DuckDB`.
#[test]
fn run_no_shape_writes_cross_type_edge_from_two_mappings() {
    let bin = fossil_binary();
    let program = "\
prefix ex: <https://example.org/>

people := io.csv(\"people.csv\")
orders := io.csv(\"orders.csv\")

Person : ex:Person from people
    iri = `${ex:}person/${.id}`
    ex:name = .name

Order : ex:Order from orders
    iri = `${ex:}order/${.order_id}`
    ex:placedBy = `${ex:}person/${.user_id}`
    ex:amount = .amount
";
    let workdir = workdir_with_files(
        "cross-type-edge",
        &[
            ("prog.fossil", program),
            ("people.csv", "id,name\n1,Ada\n2,Linus\n3,Grace\n"),
            (
                "orders.csv",
                "order_id,user_id,amount\no1,1,10\no2,2,20\no3,1,30\n",
            ),
        ],
    );
    let dest = workdir.join("graph");
    let dest_url = format!("file://{}", dest.display());

    let output = Command::new(bin)
        .args(["run", "prog.fossil", "--dest", &dest_url, "--output-json"])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil run");

    assert!(
        output.status.success(),
        "no-shape multi-mapping run exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Both vertex types materialised.
    assert!(
        dest.join("vertex/Person.parquet").exists(),
        "Person vertex Parquet missing"
    );
    assert!(
        dest.join("vertex/Order.parquet").exists(),
        "Order vertex Parquet missing"
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--output-json parseable");

    // The cross-type edge Order--placedBy-->Person is present with one edge per
    // order (3 orders, each referencing a real person → 3 CSR rows).
    let edges = parsed["edges"].as_array().expect("edges array");
    let placed_by = edges
        .iter()
        .find(|e| e["src_type"] == "Order" && e["dst_type"] == "Person")
        .unwrap_or_else(|| panic!("no Order→Person edge in status: {parsed}"));
    assert_eq!(
        placed_by["count"].as_i64(),
        Some(3),
        "3 orders join to persons; got: {placed_by}"
    );

    // The literal `ex:amount` stayed a property on Order; `ex:placedBy` did NOT.
    let order_v = parsed["vertices"]
        .as_array()
        .expect("vertices array")
        .iter()
        .find(|v| v["type"] == "Order")
        .expect("Order vertex in status");
    let cols: Vec<&str> = order_v["columns"]
        .as_array()
        .expect("columns array")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(
        cols.contains(&"amount"),
        "amount is a property; got: {cols:?}"
    );
    assert!(
        !cols.contains(&"placedBy"),
        "placedBy is an edge, not a property column; got: {cols:?}"
    );

    // #5a: the manifest carries the RDF output spec the governance layer (DCAT)
    // consumes — full shape IRI per vertex, predicate IRI + XSD datatype per
    // column — derived from the mapping alone (no ShEx, no host re-derivation).
    let person_v = parsed["vertices"]
        .as_array()
        .expect("vertices array")
        .iter()
        .find(|v| v["type"] == "Person")
        .expect("Person vertex in status");
    assert_eq!(
        person_v["rdf_type"].as_str(),
        Some("https://example.org/Person"),
        "vertex carries its full RDF type IRI; got: {person_v}"
    );
    let name_col = person_v["columns"]
        .as_array()
        .expect("columns array")
        .iter()
        .find(|c| c["name"] == "name")
        .expect("name column on Person");
    assert_eq!(
        name_col["rdf_uri"].as_str(),
        Some("https://example.org/name"),
        "column carries its full predicate IRI; got: {name_col}"
    );
    assert_eq!(
        name_col["xsd_datatype"].as_str(),
        Some("http://www.w3.org/2001/XMLSchema#string"),
        "column carries its XSD datatype IRI; got: {name_col}"
    );
}

/// End-to-end cloud write (W0 subprocess slices 1+2): pipe `DuckDB` cloud-config
/// on stdin (`--creds-stdin`), write `GraphAr` to an `s3://` destination, and
/// prove the round-trip — the `--output-json` `count` is computed by reading the
/// just-written *cloud* Parquet back, so a successful `count == 5` exercises the
/// full creds-stdin → SET dance → cloud COPY → cloud read path.
///
/// Env-gated: skips unless an S3-compatible endpoint is configured (a `MinIO` /
/// `LocalStack` fixture), so the hermetic suite stays runnable everywhere. Set:
///   `FOSSIL_TEST_S3_ENDPOINT`  e.g. `localhost:9000`
///   `FOSSIL_TEST_S3_BUCKET`    a writable bucket
///   `FOSSIL_TEST_S3_KEY` / `FOSSIL_TEST_S3_SECRET`
///   `FOSSIL_TEST_S3_REGION`    (optional, default `us-east-1`)
#[test]
fn run_w0b_writes_to_cloud_dest_with_stdin_creds() {
    let Ok(endpoint) = std::env::var("FOSSIL_TEST_S3_ENDPOINT") else {
        eprintln!("skipping cloud e2e: FOSSIL_TEST_S3_ENDPOINT unset");
        return;
    };
    let bucket = std::env::var("FOSSIL_TEST_S3_BUCKET").expect("FOSSIL_TEST_S3_BUCKET");
    let key = std::env::var("FOSSIL_TEST_S3_KEY").expect("FOSSIL_TEST_S3_KEY");
    let secret = std::env::var("FOSSIL_TEST_S3_SECRET").expect("FOSSIL_TEST_S3_SECRET");
    let region = std::env::var("FOSSIL_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string());

    let bin = fossil_binary();
    let workdir = fresh_workdir("cloud");
    let dest_url = format!("s3://{bucket}/fossil-cli-test/w0b");

    // DuckDB-spelt cloud config — the vocabulary keasy will project from its
    // provider schema. `s3_url_style=path` + `s3_use_ssl=false` suit a local
    // MinIO over http; a real AWS fixture overrides via env as needed.
    let creds = serde_json::json!({
        "dest": { "config": {
            "s3_endpoint": endpoint,
            "s3_access_key_id": key,
            "s3_secret_access_key": secret,
            "s3_region": region,
            "s3_url_style": "path",
            "s3_use_ssl": "false",
        }}
    })
    .to_string();

    let mut child = Command::new(bin)
        .args([
            "run",
            "examples/hello.fossil",
            "--dest",
            &dest_url,
            "--output-json",
            "--creds-stdin",
        ])
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fossil run");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(creds.as_bytes())
        .expect("write creds to stdin");
    let output = child.wait_with_output().expect("wait fossil run");

    assert!(
        output.status.success(),
        "cloud run exit {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--output-json parseable");
    assert_eq!(parsed["dest"], serde_json::Value::String(dest_url));
    // count is read back FROM the cloud Parquet just written → proves the
    // write+read round-trip through the object store.
    assert_eq!(
        parsed["vertices"][0]["count"].as_i64(),
        Some(5),
        "cloud round-trip count mismatch; got: {parsed}"
    );
}

/// `fossil catalog` (#5-grande slice 2): a `CatalogInput` on stdin materialises
/// the DCAT-AP graph through the same W0b writer. Proves the catalog vertex/edge
/// shape lands on real `DuckDB` and the cross-type edges (Catalog→Dataset, etc.)
/// resolve against the vertex subject URNs.
#[test]
fn catalog_subcommand_materialises_dcat_ap_graph() {
    let bin = fossil_binary();
    let workdir = workdir_with_files("catalog", &[]);
    let dest = workdir.join("catalog");
    let dest_url = format!("file://{}", dest.display());

    let payload = serde_json::json!({
        "catalog": {
            "job_id": "job-1",
            "job_name": "My Run",
            "completed_at": "2026-06-02T00:00:00Z",
            "publisher_name": "Acme Org",
            "license_uri": "https://ex.org/license",
            "contact_email": "a@b.com",
            "datasets": [{
                "type_name": "Person",
                "rdf_type": "https://example.org/Person",
                "entity_count": 5,
                "fields": [{
                    "name": "name",
                    "rdf_uri": "https://example.org/name",
                    "datatype": "http://www.w3.org/2001/XMLSchema#string"
                }],
                "distributions": [{
                    "destination": "file:///out/vertex/Person.parquet",
                    "media_type": "application/parquet"
                }]
            }]
        }
    })
    .to_string();

    let mut child = Command::new(bin)
        .args(["catalog", "--dest", &dest_url, "--output-json"])
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fossil catalog");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(payload.as_bytes())
        .expect("write catalog payload");
    let output = child.wait_with_output().expect("wait fossil catalog");

    assert!(
        output.status.success(),
        "fossil catalog exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The DCAT-AP vertex Parquets are written.
    for vtype in [
        "Catalog",
        "Dataset",
        "Distribution",
        "Agent",
        "Contact",
        "Field",
    ] {
        assert!(
            dest.join(format!("vertex/{vtype}.parquet")).exists(),
            "missing vertex/{vtype}.parquet"
        );
    }

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--output-json parseable");

    // Dataset vertex carries its DCAT-AP RDF type + a conforms_to column.
    let dataset = parsed["vertices"]
        .as_array()
        .expect("vertices array")
        .iter()
        .find(|v| v["type"] == "Dataset")
        .expect("Dataset vertex");
    assert_eq!(
        dataset["rdf_type"].as_str(),
        Some("http://www.w3.org/ns/dcat#Dataset"),
        "Dataset rdf_type; got: {dataset}"
    );

    // The Catalog→Dataset edge resolved its endpoints against the vertex URNs.
    let edges = parsed["edges"].as_array().expect("edges array");
    let cat_ds = edges
        .iter()
        .find(|e| e["src_type"] == "Catalog" && e["dst_type"] == "Dataset")
        .unwrap_or_else(|| panic!("no Catalog→Dataset edge: {parsed}"));
    assert_eq!(
        cat_ds["count"].as_i64(),
        Some(1),
        "one dataset edge; got: {cat_ds}"
    );
}
