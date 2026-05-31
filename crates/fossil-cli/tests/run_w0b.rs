//! `fossil run --dest <url> --shape <file>` W0b path integration test.
//!
//! Drives the canonical `examples/hello.fossil` + `packages/examples/src/hello/hello.shex`
//! through the new W0b writer + materializer (commit chain
//! `34dcd57..a41b97a` + bridge `6392bb7`). Asserts the resulting
//! `GraphAr` layout matches the W0b column shape (`dense_id` /
//! `subject` / `x` / `y` / `cluster_id` on vertices) and the
//! `--output-json` switch emits a parseable status object on stdout.
//!
//! The `DuckDB` bundled-build cost (~75-85s cold) is incurred once and
//! amortised across the two test cases here + the existing
//! `run_summary.rs` walking-skeleton check.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Per-test workdir with the canonical hello.fossil + hello.csv + hello.shex.
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
    // ShEx target lives under packages/examples/src/hello/.
    std::fs::copy(
        root.join("packages/examples/src/hello/hello.shex"),
        tmp.join("hello.shex"),
    )
    .expect("copy hello.shex");
    tmp
}

/// Acceptance: the W0b path produces `GraphAr` Parquet + YAML manifests
/// under --dest, with the W0b vertex column shape declared in the
/// vertex.yml manifest.
#[test]
fn run_w0b_writes_graph_ar_under_dest() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("dest");
    let dest = workdir.join("graph");
    let dest_url = format!("file://{}", dest.display());

    let output = Command::new(bin)
        .args([
            "run",
            "examples/hello.fossil",
            "--shape",
            "hello.shex",
            "--dest",
            &dest_url,
        ])
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
            "--shape",
            "hello.shex",
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
    let first_vertex = parsed["vertices"][0]
        .as_str()
        .expect("at least one vertex path");
    assert!(
        first_vertex.starts_with("vertex/"),
        "vertex rel_path expected; got: {first_vertex}"
    );
}
