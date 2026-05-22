//! `fossil run` result-summary test (CLI-03 / SC#1).
//!
//! Runs `fossil run examples/hello.fossil` and asserts the stdout summary line
//! reports the triple count (the v0.1 flat-triple output is a single
//! `output.parquet`; the summary counts its rows). This is the test that
//! exercises the real native-`DuckDB` run path end-to-end, so it lives here
//! rather than in a per-task `cargo build` verify (bundled `DuckDB` execution is
//! ~75-85s per invocation — see the plan's W-slow note).

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

/// Materialise a fresh per-test working directory with the canonical example +
/// its CSV so the run writes its artifacts in isolation.
fn fresh_workdir(test_name: &str) -> PathBuf {
    let root = repo_root();
    let tmp = std::env::temp_dir().join(format!("fossil-cli-run-{test_name}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("examples")).expect("create examples subdir");
    for f in ["hello.fossil", "users.csv"] {
        std::fs::copy(root.join("examples").join(f), tmp.join("examples").join(f))
            .unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    tmp
}

#[test]
fn run_hello_prints_triple_count_summary() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("summary");

    let output = Command::new(bin)
        .args(["run", "examples/hello.fossil"])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil run");

    assert!(
        output.status.success(),
        "fossil run exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("triples"),
        "run summary must mention triples; got: {stdout}"
    );
    // The canonical walking-skeleton produces exactly 5 triples.
    assert!(
        stdout.contains("5 triples"),
        "run summary should report 5 triples for hello.fossil; got: {stdout}"
    );
}
