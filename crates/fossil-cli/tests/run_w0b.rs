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

use std::path::{Path, PathBuf};
use std::process::Command;
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
/// (`crates/fossil-engine/src/lib.rs:494`). Three assertions in this file went
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

/// `--output-json` emits the manifest as one JSON object keasy can parse via
/// `serde_json::from_str` after `Command::output()`.
///
/// **It is the manifest and not a second account of one.** Every key asserted
/// below is a `fossil_sinks::manifest` field, spelled the way the YAML on disk
/// spells it — `type`, `vertex_count`, `prefix`, `property_groups` — so a host
/// that learns the format learns stdout for free, and the two cannot drift.
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
    assert_eq!(
        parsed["graph"]["vertices"][0].as_str(),
        Some("vertex/Person.vertex.yml"),
        "the index names the per-type document; got: {parsed}"
    );

    let first = &parsed["vertices"][0];
    assert_eq!(
        first["type"].as_str(),
        Some("Person"),
        "vertex.type expected; got: {first}"
    );
    assert_eq!(
        first["prefix"].as_str(),
        Some("vertex/Person/"),
        "the chunk prefix a reader addresses, not the staged file; got: {first}"
    );
    assert_eq!(
        first["vertex_count"].as_u64(),
        Some(5),
        "hello.fossil writes 5 Persons (examples/users.csv); got: {first}"
    );
    assert!(
        first["property_groups"][0]["properties"]
            .as_array()
            .is_some_and(|c| c.iter().any(|col| col["name"] == "name")),
        "the property group must declare the `name` column; got: {first}"
    );

    // The JSON on stdout and the YAML on disk are the same values. That is the
    // whole reason `RunStatus` is gone, and a string comparison of the two
    // serialisations is the cheapest thing that would notice them parting.
    let on_disk: serde_json::Value = serde_yaml_ng::from_str(
        &std::fs::read_to_string(dest.join("vertex/Person.vertex.yml")).expect("read the document"),
    )
    .expect("the document is YAML");
    assert_eq!(*first, on_disk, "stdout and the document disagree");
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
        .unwrap_or_else(|| panic!("no Order→Person edge in the report: {parsed}"));
    assert_eq!(
        placed_by["edge_count"].as_u64(),
        Some(3),
        "3 orders join to persons; got: {placed_by}"
    );

    // And every one of the three resolved, so the drop beside it is zero —
    // stated rather than omitted, because «no key» and «nothing dropped» are
    // not the same answer.
    let drops = parsed["dropped"]
        .as_array()
        .expect("dropped array")
        .iter()
        .find(|d| d["prefix"] == placed_by["prefix"])
        .unwrap_or_else(|| panic!("no drop entry for the edge: {parsed}"));
    assert_eq!(drops["dropped"].as_u64(), Some(0), "got: {drops}");

    // The literal `ex:amount` stayed a property on Order; `ex:placedBy` did NOT.
    let order_v = parsed["vertices"]
        .as_array()
        .expect("vertices array")
        .iter()
        .find(|v| v["type"] == "Order")
        .expect("Order vertex in the report");
    let cols: Vec<&str> = order_v["property_groups"][0]["properties"]
        .as_array()
        .expect("properties array")
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
    // consumes — the full shape IRI per vertex type, derived from the mapping
    // alone (no ShEx, no host re-derivation).
    //
    // **It used to carry more, and this is where the loss is visible.**
    // `RunStatus` also gave a `rdf_uri` and an `xsd_datatype` per COLUMN, and
    // `fossil_sinks::manifest::Property` has neither: it declares `name`,
    // `data_type`, `is_primary` and `is_nullable`. `VertexInfo::iri` and
    // `EdgeInfo::iri` exist and a property's predicate does not, which is an
    // asymmetry in the format rather than a decision — the fix, if the DCAT
    // projection needs it back, is a `Property::iri` beside the other two, not
    // a second description of the corpus.
    let person_v = parsed["vertices"]
        .as_array()
        .expect("vertices array")
        .iter()
        .find(|v| v["type"] == "Person")
        .expect("Person vertex in the report");
    assert_eq!(
        person_v["iri"].as_str(),
        Some("https://example.org/Person"),
        "vertex carries its full RDF type IRI; got: {person_v}"
    );
    let name_col = person_v["property_groups"][0]["properties"]
        .as_array()
        .expect("properties array")
        .iter()
        .find(|c| c["name"] == "name")
        .expect("name column on Person");
    assert_eq!(
        name_col["data_type"].as_str(),
        Some("string"),
        "column carries its GraphAr storage spelling; got: {name_col}"
    );
}

// **There is no cloud-destination test here, and there was one.**
//
// It was `run_w0b_writes_to_cloud_dest_with_stdin_creds`: env-gated on
// `FOSSIL_TEST_S3_ENDPOINT`, so it skipped on every machine that has ever run
// this suite, and it asserted that `fossil run --dest s3://…` succeeds. It
// cannot. `fossil_engine::run` resolves its destination through
// `local_dest_dir`, which returns `None` for any scheme but `file://`, and the
// run is refused before a byte is written. The test had also drifted out of the
// wire it drove: it piped `{"dest":{"config":{…}}}` while the payload type
// declared `{"dest":{"secret":{…}}}`, and neither was ever read.
//
// A skipped test asserting a capability that does not exist is worse than no
// test: it is the capability's only documentation, and it reads as coverage.
// What replaced it is `crates/fossil-engine/tests/cloud_dest.rs`, which asserts
// the refusal — and which goes red the day the write path learns an object
// store, so the removal cannot be forgotten.
