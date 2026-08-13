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

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

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

// A program mixing `@conn` references (data + schema) in a destructuring io.rdf
// with a literal local-path csv — exercising both roles and both the aliased and
// unaliased forms. The two members share one (data, schema) pair.
const PROGRAM: &str = r#"prefix ex: <https://ex.org/>

{ KB, Project } := io.rdf("@data/graph.ttl", schema = io.shex("@vocab/graph.shex"))
plain := io.csv("local.csv")

KB : ex:KB from KB
    iri = .subject
    ex:label = .label
"#;

#[test]
fn refs_lists_typed_references_with_connection_aliases() {
    let bin = fossil_binary();
    let dir = common::unique_workdir("fossil-cli-refs", "typed-references");
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
    assert_eq!(
        schema_refs, 1,
        "schema ref deduped across members: {refs:?}"
    );

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
