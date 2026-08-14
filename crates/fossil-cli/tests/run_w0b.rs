// The embedded `.fossil` fixture carries `"…{people.id}"` interpolation holes —
// LITERAL fossil source, which clippy mistakes for format args in a plain Rust
// string literal. Same allow, same reason, as `fossil-engine`'s
// `provider_registry.rs`.
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

mod common;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

/// The `fossil` binary this test drives — cargo's own path for it.
///
/// **It used to shell out to `cargo build` and then hard-code
/// `<repo>/target/debug/fossil`**, which is a test that can pass against a
/// binary it did not build: with `CARGO_TARGET_DIR` set — which is how this
/// repository's own instructions say to drive the suite — the build lands
/// elsewhere and that path holds whatever was left there last. Measured on
/// 2026-08-13: the file at the hard-coded path was **29 hours old**, older than
/// the parser rewrite, the provider registry, `@rename` and the edge
/// constructor. Everything this file reported that day was about a compiler
/// nobody had edited.
///
/// `CARGO_BIN_EXE_<name>` is cargo's answer: it is set for an integration test
/// and points at the binary of THIS build, which cargo has already built before
/// the test runs. No path to guess, and no `cargo build` spawned from inside a
/// test — the same fix `crates/fossil-lsp/tests/` took.
fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| PathBuf::from(env!("CARGO_BIN_EXE_fossil")))
}

/// Per-test workdir with the canonical hello.fossil + users.csv.
fn fresh_workdir(test_name: &str) -> PathBuf {
    let root = repo_root();
    let tmp = common::unique_workdir("fossil-cli-w0b", test_name);
    std::fs::create_dir_all(tmp.join("examples")).expect("create examples subdir");
    // Everything the program NAMES: the `users.csv` its `io.csv` binding reads
    // and the `hello.shex` its `type { Person }` binding names. Without the
    // document the mapping has no output contract and the run writes a `Person`
    // with no `name` column — no error, just a column that is not there.
    for f in ["hello.fossil", "users.csv", "hello.shex"] {
        std::fs::copy(root.join("examples").join(f), tmp.join("examples").join(f))
            .unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    tmp
}

/// Assert that `vtype`'s vertex tiles are on disk, and return a `read_parquet`
/// glob over them.
///
/// A vertex is **tiles**, not a file. `c416e07` made the layout pass emit one
/// tile per 4,096-row `dense_id` range under `vertex/<Type>/` and then delete
/// the single staged `vertex/<Type>.parquet`
/// (`crates/fossil-engine/src/lib.rs:502`). Three assertions in this file went
/// on naming the deleted path, so the suite went red the day the emitter landed
/// and stayed red — one of five failures across the repo from the same commit,
/// none of which were the emitter being wrong.
///
/// The convention lives here so the next change to tile naming is one diff.
fn assert_vertex_tiles(dest: &Path, vtype: &str) -> String {
    let dir = dest.join("vertex").join(vtype);
    assert!(
        dir.is_dir(),
        "vertex tile directory missing at {}",
        dir.display()
    );
    let tiles: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "parquet"))
        .collect();
    assert!(
        !tiles.is_empty(),
        "no vertex tiles under {} — the directory exists but the emitter wrote nothing",
        dir.display()
    );
    // And the staged single file must be gone; if it comes back, the emitter has
    // stopped cleaning up and every reader has two sources of truth.
    let staged = dest.join(format!("vertex/{vtype}.parquet"));
    assert!(
        !staged.exists(),
        "the staged single file survived at {} — readers would see it and the tiles",
        staged.display()
    );
    format!("{}/*.parquet", dir.display())
}

/// Acceptance: the W0b path produces `GraphAr` Parquet + YAML manifests
/// under --dest, with the W0b vertex column shape declared in the
/// vertex.yml manifest. The vertex type `Person` is derived from the IRI of the
/// shape its header names (`https://example.org/Person`, declared in
/// `hello.shex`) — no `--shape` flag needed.
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

    // The Person vertex tiles are the W0b artefact.
    let vertex_glob = assert_vertex_tiles(&dest, "Person");

    // W3.1b: the layout pass must have replaced the placeholder x/y (0 for every
    // vertex) with real coordinates — at least one vertex now carries a non-zero
    // coordinate.
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let nonzero: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM read_parquet('{}') WHERE x <> 0 OR y <> 0",
                vertex_glob.replace('\'', "''")
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
    let tmp = common::unique_workdir("fossil-cli-w0b", test_name);
    for (rel, contents) in files {
        let path = tmp.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture subdir");
        }
        std::fs::write(&path, contents).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }
    tmp
}

/// Slice 8 end-to-end: a TWO-mapping program with no `--shape` FLAG — the output
/// descriptor is program-resident, read from the `type { … } := io.shex(…)`
/// binding the program names. It must (a) materialise BOTH vertex types and (b)
/// turn `placedBy = Person(orders.user_id)` into a cross-type edge to the
/// `Person` mapping, whose CSR Parquet joins the order subjects to the person
/// subjects. Proves Phase B edge synthesis (8a) + multi-mapping
/// merge/materialisation (8b) on real `DuckDB`.
///
/// **The edge is now a CALL, and both halves of that moved.** It used to be
/// GUESSED: `ex:placedBy = ${ex:}person/${.user_id}` was matched against every
/// mapping's subject template by SKELETON — every per-row hole blanked to a
/// `\u{1}` marker — and a match made it a foreign key. `Person(orders.user_id)`
/// says it instead: the destination type applied to an expression, resolved
/// through the one identity template that type has. The `.shex` below is the
/// other half — `ex:placedBy @ex:Person` is what classifies the predicate as an
/// edge rather than a property, and `ex:amount` beside it stays a column.
#[test]
fn run_no_shape_writes_cross_type_edge_from_two_mappings() {
    let bin = fossil_binary();
    let program = "\
type { Person, Order } := io.shex(\"prog.shex\")

people := io.csv(\"people.csv\")
orders := io.csv(\"orders.csv\")

People : Person from people
    @subject = \"https://example.org/person/{people.id}\"
    name = people.name

Orders : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = Person(orders.user_id)
    amount = orders.amount
";
    let shex = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string
}

ex:Order {
  ex:placedBy @ex:Person ;
  ex:amount   .
}
";
    let workdir = workdir_with_files(
        "cross-type-edge",
        &[
            ("prog.fossil", program),
            ("prog.shex", shex),
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
    let _ = assert_vertex_tiles(&dest, "Person");
    let _ = assert_vertex_tiles(&dest, "Order");

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
        let _ = assert_vertex_tiles(&dest, vtype);
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
