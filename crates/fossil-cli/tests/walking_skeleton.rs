//! Workspace-level walking-skeleton e2e integration test.
//!
//! The canonical gate: `fossil run
//! examples/hello.fossil --dest <tmp>` produces a valid `GraphAr` dataset
//! end-to-end through every compiler+runtime crate. The output descriptor is
//! program-resident (synthesised from the typed mapping — no `--shape`).
//!
//! This test goes deeper than mere existence — it asserts the **content** of
//! the produced `vertex/Person.parquet` matches the mapping verbatim (5 Person
//! vertices with the expanded subject IRIs `https://example.org/user/{1..5}`
//! and the five names from `examples/users.csv` on the `name` property column).
//! It is the strongest form of the walking-skeleton invariant: any regression
//! that silently changes the produced graph (wrong template substitution,
//! dropped property, broken IRI prefix expansion, off-by-one row drop) is
//! caught here before it can land.
//!
//! Once this test passes, every subsequent commit must keep it green per the
//! walking-skeleton invariant (`CLAUDE.md`, "Hard Rules"): a refactor that
//! breaks it for more than three days is reverted and broken into smaller steps.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

mod common;

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

/// Materialise a fresh per-test working directory holding everything the
/// program NAMES: `hello.fossil`, the `users.csv` it reads, and the
/// `hello.shex` that is its output contract. The path is unique per process —
/// see `common::unique_workdir`, which explains why the test name alone was not
/// enough.
///
/// **The shape document is not optional and its absence is silent.** Ruling 3
/// of 2026-08-11 makes a property key the last segment of a predicate IRI the
/// shape declares, so a workdir without `hello.shex` gives the mapping no
/// output contract, `name` resolves to nothing, and the run writes a `Person`
/// with no `name` column — five vertices, no error, and the content assertions
/// below are the only thing that would catch it. The list is spelled out here
/// rather than globbed because a file this test needs and does not copy is
/// exactly that failure.
fn fresh_workdir(test_name: &str) -> PathBuf {
    let root = repo_root();
    let tmp = common::unique_workdir("fossil-walking-skeleton", test_name);
    std::fs::create_dir_all(tmp.join("examples")).expect("create examples subdir");
    for f in ["hello.fossil", "users.csv", "hello.shex"] {
        std::fs::copy(root.join("examples").join(f), tmp.join("examples").join(f))
            .unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    tmp
}

#[test]
#[allow(clippy::too_many_lines)] // E2E test with three logical assertion blocks: artefact existence, manifest shape, parquet content. Splitting hurts readability more than it helps.
fn walking_skeleton_run_writes_5_person_vertices_with_expected_content() {
    let bin = fossil_binary();
    let workdir = fresh_workdir("content");
    let dest = workdir.join("graph");
    let dest_url = format!("file://{}", dest.display());

    // 1. Run `fossil run examples/hello.fossil --dest <tmp>` in the isolated
    //    workdir — the canonical gate, GraphAr W0b output.
    let output = Command::new(bin)
        .args(["run", "examples/hello.fossil", "--dest", &dest_url])
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

    // The `Person` vertex type name is derived from the mapping's shape IRI
    // (`User : ex:Person` → local name `Person`), not the mapping name.
    //
    // A vertex is TILES, not a file: `c416e07` made the layout pass emit one per
    // 4,096-row `dense_id` range under `vertex/<Type>/` and delete the single
    // staged `vertex/<Type>.parquet`. This asserted the deleted path and had been
    // red since — the last of the five failures that commit left behind.
    let tiles = dest.join("vertex").join("Person");
    let manifest = dest.join("vertex/Person.vertex.yml");
    assert!(
        tiles.is_dir(),
        "vertex tile directory missing at {}",
        tiles.display()
    );
    assert!(
        !dest.join("vertex/Person.parquet").exists(),
        "the staged single file survived — readers would see it and the tiles"
    );
    assert!(
        manifest.exists(),
        "vertex/Person.vertex.yml missing at {}",
        manifest.display()
    );

    // 2. Manifest shape sanity — the GraphAr v1 vertex manifest declares the W0b
    //    column shape (`dense_id` + layout placeholders) plus the `name`
    //    property derived from the typed mapping.
    let manifest_text = std::fs::read_to_string(&manifest).expect("read vertex.yml");
    assert!(
        manifest_text.contains("version: gar/v1"),
        "manifest missing GraphAr v1 version anchor; got:\n{manifest_text}",
    );
    for col in ["dense_id", "subject", "name"] {
        assert!(
            manifest_text.contains(&format!("name: {col}")),
            "manifest missing `{col}` column; got:\n{manifest_text}",
        );
    }

    // 3. Parquet content. Open via DuckDB native. Build the path as a Display so
    //    platform-specific separators round-trip through the SQL string literal.
    let conn = duckdb::Connection::open_in_memory().expect("open in-memory duckdb");
    let parquet_path = format!("{}/*.parquet", tiles.display()).replace('\'', "''");

    // 3a. Row count == 5 (one Person per row of users.csv).
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{parquet_path}')"),
            [],
            |row| row.get(0),
        )
        .expect("query parquet row count");
    assert_eq!(count, 5, "expected 5 Person vertices, got {count}");

    // 3b. Subject column matches the IRI template `${ex:}user/${.id}` =
    //     `https://example.org/user/{1..5}`. Order by subject for determinism.
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

    // 3c. The `name` property column matches the `.name` field of users.csv.
    //     Order by name alphabetically so Alice..Eve is the expected sequence.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT name FROM read_parquet('{parquet_path}') ORDER BY name"
        ))
        .expect("prepare ordered name query");
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("run ordered name query")
        .map(|r| r.expect("read name row"))
        .collect();
    assert_eq!(
        names,
        vec![
            "Alice".to_string(),
            "Bob".to_string(),
            "Carol".to_string(),
            "Dave".to_string(),
            "Eve".to_string(),
        ],
        "name property should carry the 5 names from users.csv",
    );
}
