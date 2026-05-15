//! End-to-end CLI integration tests — the **walking-skeleton gate**.
//!
//! These tests build the `fossil` binary (no-op if up-to-date), invoke it
//! against the canonical `examples/hello.fossil` from the repo root, and
//! assert that the produced `output.parquet` and `manifest.yaml` artefacts
//! exist with valid content. Use `env!("CARGO_MANIFEST_DIR")` to resolve the
//! repo root so the tests run from any working directory and on any
//! developer's machine (zero hardcoded absolute paths per ADR-0004).
//!
//! Once these tests pass, the **walking-skeleton invariant** is active: every
//! subsequent commit must keep them passing. See ROADMAP.md sequencing rule
//! #6 + CLAUDE.md "Hard Rules".

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Locate the repo root from `CARGO_MANIFEST_DIR` (= `.../crates/fossil-cli`).
/// Walks up two levels.
fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

// `Path::parent` is from `std::path::Path`; alias here so the chain reads
// naturally above without importing it at the module level.
use std::path::Path;

/// Build the `fossil` binary once per test process via `cargo build`.
/// Memoised through `OnceLock` so the second test does not pay the build cost
/// (and does not race against the first on `target/debug/fossil`).
fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "fossil-cli", "--bin", "fossil"])
            .status()
            .expect("spawn cargo build");
        assert!(status.success(), "cargo build -p fossil-cli failed");

        let bin = repo_root().join("target").join("debug").join("fossil");
        assert!(
            bin.exists(),
            "fossil binary not found at {} after cargo build",
            bin.display(),
        );
        bin
    })
}

/// Materialise a fresh per-test working directory containing
/// `examples/hello.fossil` + `examples/users.csv`. Each test gets its own
/// subdirectory under `std::env::temp_dir()` so concurrent test harness
/// invocations don't stomp on `output.parquet` / `manifest.yaml`.
fn fresh_workdir(test_name: &str) -> PathBuf {
    let root = repo_root();
    let tmp = std::env::temp_dir().join(format!("fossil-cli-test-{test_name}"));
    // Wipe any prior run's artefacts so existence assertions below mean
    // "this run wrote it".
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("examples")).expect("create examples subdir");
    std::fs::copy(
        root.join("examples").join("hello.fossil"),
        tmp.join("examples").join("hello.fossil"),
    )
    .expect("copy hello.fossil");
    std::fs::copy(
        root.join("examples").join("users.csv"),
        tmp.join("examples").join("users.csv"),
    )
    .expect("copy users.csv");
    tmp
}

#[test]
fn fossil_compile_hello_writes_output_parquet_and_manifest() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("compile");

    let output = Command::new(bin)
        .args(["compile", "examples/hello.fossil"])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil compile");

    assert!(
        output.status.success(),
        "fossil compile exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let parquet = workdir.join("output.parquet");
    let manifest = workdir.join("manifest.yaml");
    assert!(
        parquet.exists(),
        "output.parquet missing at {}",
        parquet.display()
    );
    assert!(
        manifest.exists(),
        "manifest.yaml missing at {}",
        manifest.display()
    );

    let parquet_size = std::fs::metadata(&parquet)
        .expect("stat output.parquet")
        .len();
    let manifest_size = std::fs::metadata(&manifest)
        .expect("stat manifest.yaml")
        .len();
    assert!(parquet_size > 0, "output.parquet should be non-empty");
    assert!(manifest_size > 0, "manifest.yaml should be non-empty");

    // Read the parquet back through DuckDB to confirm 5 triples
    // (RESEARCH.md Example 3 — the canonical Phase 1 expectation).
    let conn = duckdb::Connection::open_in_memory().expect("open in-memory duckdb");
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{}')", parquet.display()),
            [],
            |row| row.get(0),
        )
        .expect("query parquet row count");
    assert_eq!(count, 5, "output.parquet should have 5 triples");
}

#[test]
fn fossil_check_hello_exits_zero() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("check");

    let output = Command::new(bin)
        .args(["check", "examples/hello.fossil"])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil check");

    assert!(
        output.status.success(),
        "fossil check exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ok"),
        "fossil check should print 'ok'; got: {}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn fossil_run_hello_writes_output_parquet_and_manifest() {
    // `fossil run` aliases `fossil compile` in Phase 1; verify the alias
    // produces the same artefacts so a regression in dispatch is caught early.
    let bin = fossil_binary();
    let workdir = fresh_workdir("run");

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
    assert!(
        workdir.join("output.parquet").exists(),
        "output.parquet missing"
    );
    assert!(
        workdir.join("manifest.yaml").exists(),
        "manifest.yaml missing"
    );
}
