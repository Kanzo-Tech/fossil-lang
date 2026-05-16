//! Workspace-level walking-skeleton e2e integration test.
//!
//! Phase 1 success criterion #1 (ROADMAP.md): "`fossil compile
//! examples/hello.fossil` produces `output.parquet` + `manifest.yaml` on disk
//! with content readable by both native `DuckDB` and `DuckDB-WASM`."
//!
//! Plan 01-07's `cli_integration.rs` covers the existence + row-count half of
//! that criterion. This test goes deeper — it asserts the **content** of the
//! produced parquet matches `01-RESEARCH.md` Example 3 verbatim (5 triples
//! with specific subject IRIs, the constant `https://example.org/name`
//! predicate, and the five names from `examples/users.csv`). It is the
//! strongest form of the walking-skeleton invariant: any regression that
//! silently changes the produced triples (wrong template substitution, swapped
//! subject/object columns, broken IRI prefix expansion, off-by-one row drop)
//! is caught here before it can land.
//!
//! Plan 01-10 closing test. Lives in `fossil-cli/tests/` (option 2 from
//! plan 01-10 `<interfaces>` — colocated with the existing CLI integration
//! tests rather than a new top-level `tests/` crate, keeping the 15-crate
//! workspace count locked per ADR-0002). Once this test passes, every
//! subsequent commit must keep it green per the walking-skeleton invariant
//! (CLAUDE.md "Hard Rules" + ROADMAP.md sequencing rule #6).
//!
//! The DuckDB-WASM half of SC #1 ("readable by ... DuckDB-WASM") is a Phase 7
//! PLAY-02 deliverable — DuckDB-WASM is not in the Phase 1 dep tree.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Locate the repo root from `CARGO_MANIFEST_DIR` (= `.../crates/fossil-cli`).
/// Walks up two levels. Mirrors the pattern in `cli_integration.rs` so the two
/// test files have identical filesystem-rooting semantics.
fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

/// Build the `fossil` binary once per test process via `cargo build`.
/// Memoised through `OnceLock` so a future second test in this file does not
/// pay the build cost. Mirrors `cli_integration.rs::fossil_binary` — the two
/// test binaries are separate processes so they each pay the build once, but
/// Cargo's `--quiet` no-op rebuild is cheap when the binary is already
/// up-to-date from the sibling test run.
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
/// `examples/hello.fossil` + `examples/users.csv`. Distinct test-name prefix
/// so concurrent runs against `cli_integration.rs` do not stomp on each
/// other's `output.parquet` / `manifest.yaml` in `std::env::temp_dir()`.
fn fresh_workdir(test_name: &str) -> PathBuf {
    let root = repo_root();
    let tmp = std::env::temp_dir().join(format!("fossil-walking-skeleton-{test_name}"));
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
#[allow(clippy::too_many_lines)] // E2E test with three logical assertion blocks: file existence, parquet content, manifest content. Splitting hurts readability more than it helps.
fn walking_skeleton_compile_writes_5_triples_with_expected_content() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("content");

    // 1. Run `fossil compile examples/hello.fossil` in the isolated workdir.
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

    // 2. Manifest shape sanity — the Phase 1 hand-templated GraphAr manifest
    //    from `fossil-codegen::manifest::manifest_template`. Phase 5 SINK-02
    //    will promote this to programmatic generation; until then we assert
    //    the three lexically-stable anchors.
    let manifest_text = std::fs::read_to_string(&manifest).expect("read manifest.yaml");
    assert!(
        manifest_text.contains("graphar_version: 1.0.0"),
        "manifest missing graphar_version anchor; got:\n{manifest_text}",
    );
    assert!(
        manifest_text.contains("vertex_types:"),
        "manifest missing vertex_types anchor; got:\n{manifest_text}",
    );
    assert!(
        manifest_text.contains("Person"),
        "manifest missing Person vertex; got:\n{manifest_text}",
    );

    // 3. Parquet content — RESEARCH.md Example 3 verbatim. Open via DuckDB
    //    native (the WASM half is Phase 7 PLAY-02). Build the path as a
    //    Display so platform-specific separators round-trip through the SQL
    //    string literal.
    let conn = duckdb::Connection::open_in_memory().expect("open in-memory duckdb");
    let parquet_path = parquet.display().to_string();

    // 3a. Row count == 5 (one per row of users.csv).
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{parquet_path}')"),
            [],
            |row| row.get(0),
        )
        .expect("query parquet row count");
    assert_eq!(count, 5, "expected 5 triples, got {count}");

    // 3b. Predicate column is the constant IRI from `ex:name` after prefix
    //     expansion. `ex:` is bound to `<https://example.org/>` in
    //     `examples/hello.fossil`, so `ex:name` expands to
    //     `https://example.org/name`.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT DISTINCT predicate FROM read_parquet('{parquet_path}')"
        ))
        .expect("prepare distinct predicate query");
    let predicates: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("run distinct predicate query")
        .map(|r| r.expect("read predicate row"))
        .collect();
    assert_eq!(
        predicates,
        vec!["https://example.org/name".to_string()],
        "predicate column should be the single constant IRI ex:name",
    );

    // 3c. Subject column matches the IRI template
    //     `${ex:}user/${.id}` = `https://example.org/user/{1..5}`. Order by
    //     subject string so the assertion is deterministic regardless of
    //     parquet row-group ordering.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT subject FROM read_parquet('{parquet_path}') ORDER BY subject"
        ))
        .expect("prepare ordered subject query");
    let subjects: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("run ordered subject query")
        .map(|r| r.expect("read subject row"))
        .collect();
    assert_eq!(
        subjects,
        vec![
            "https://example.org/user/1".to_string(),
            "https://example.org/user/2".to_string(),
            "https://example.org/user/3".to_string(),
            "https://example.org/user/4".to_string(),
            "https://example.org/user/5".to_string(),
        ],
        "subjects should be the 5 expanded user IRIs",
    );

    // 3d. Object column matches the `.name` field of users.csv. Order by
    //     object alphabetically so Alice..Eve is the expected sequence.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT object FROM read_parquet('{parquet_path}') ORDER BY object"
        ))
        .expect("prepare ordered object query");
    let objects: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("run ordered object query")
        .map(|r| r.expect("read object row"))
        .collect();
    assert_eq!(
        objects,
        vec![
            "Alice".to_string(),
            "Bob".to_string(),
            "Carol".to_string(),
            "Dave".to_string(),
            "Eve".to_string(),
        ],
        "objects should be the 5 names from users.csv",
    );
}
