//! `fossil refs` — the typed lineage of a program's external references.
//!
//! Parses a `.fossil` and emits one `SourceRefInfo` per DISTINCT URI-valued
//! source argument (data + `schema =`), each tagged with the `@conn` alias it
//! targets (or `null` for a direct URL/path). keasy consumes this to derive a
//! job's connections WITHOUT regex-matching `@name/` in the script text — and,
//! unlike the regex, it sees `@conn` refs in EVERY position, not just the data
//! URI. A destructuring `{ A, B } := io.rdf(...)` reports its shared data +
//! schema ONCE (not once per member). Parse-only: no `DuckDB`, no credentials.

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

// A program mixing `@conn` references (data + schema) in a destructuring io.rdf
// with a literal local-path csv — exercising both roles and both the aliased and
// unaliased forms. The two members share one (data, schema) pair.
const PROGRAM: &str = r#"prefix ex: <https://ex.org/>

{ KB, Project } := io.rdf("@data/graph.ttl", schema = "@vocab/graph.shex")
plain := io.csv("local.csv")

KB : ex:KB from KB
    iri = .subject
    ex:label = .label
"#;

#[test]
fn refs_lists_typed_references_with_connection_aliases() {
    let bin = fossil_binary();
    let dir = std::env::temp_dir().join("fossil-cli-refs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create workdir");
    let prog = dir.join("p.fossil");
    std::fs::write(&prog, PROGRAM).expect("write program");

    let output = Command::new(bin)
        .args(["refs", prog.to_str().expect("utf8 path"), "--output-json"])
        .output()
        .expect("spawn fossil refs");
    assert!(
        output.status.success(),
        "fossil refs exited {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let refs: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
            .expect("--output-json parseable");
    let refs = refs.as_array().expect("refs array");

    let find = |conn: &str, role: &str| {
        refs.iter()
            .find(|r| r["connection"] == conn && r["role"] == role)
            .unwrap_or_else(|| panic!("no {role} ref for @{conn}: {refs:?}"))
    };
    assert_eq!(find("data", "data")["path"], "graph.ttl");
    // The schema ref — invisible to the old regex (it only saw the data URI) — is
    // reported with its `@vocab` alias.
    assert_eq!(find("vocab", "schema")["path"], "graph.shex");

    // The destructuring members share one (data, schema): the refs are
    // deduplicated, so exactly one data + one schema for @data/@vocab.
    let data_refs = refs
        .iter()
        .filter(|r| r["connection"] == "data" && r["role"] == "data")
        .count();
    assert_eq!(data_refs, 1, "data ref deduped across members: {refs:?}");
    let schema_refs = refs
        .iter()
        .filter(|r| r["connection"] == "vocab" && r["role"] == "schema")
        .count();
    assert_eq!(schema_refs, 1, "schema ref deduped across members: {refs:?}");

    // The literal local path has no connection alias.
    let plain = refs
        .iter()
        .find(|r| r["path"] == "local.csv")
        .expect("local.csv ref");
    assert_eq!(plain["connection"], serde_json::Value::Null);
    assert_eq!(plain["role"], "data");

    // The distinct connections a job would attach: exactly {data, vocab}.
    let mut conns: Vec<&str> = refs
        .iter()
        .filter_map(|r| r["connection"].as_str())
        .collect();
    conns.sort_unstable();
    conns.dedup();
    assert_eq!(conns, vec!["data", "vocab"]);
}
