//! `fossil refs` — the typed lineage of a program's external references.
//!
//! Parses a `.fossil` and emits one `SourceRefInfo` per DISTINCT URI-valued
//! source argument (data + `schema =`), each tagged with the `@conn` alias it
//! targets (or `null` for a direct URL/path). It sees `@conn` refs in EVERY
//! position, not just the data URI, which is the whole reason a typed answer
//! beats regex-matching `@name/` in the script text. A destructuring
//! `{ A, B } := io.rdf(...)` reports its shared data + schema ONCE (not once
//! per member). Parse-only: no `DuckDB`, no credentials.
//!
//! **This verb has no production consumer.** keasy reaches the same lineage
//! through `@fossil-lang/wasm` and spawns no `fossil` binary, so the live path
//! is `fossil-wasm`'s `refs_native` and this is the CLI surface beside it. Kept
//! because the parity is the point — `crates/fossil-wasm/tests/refs.rs` runs the
//! same program through the browser core.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

/// The `fossil` binary this test drives — cargo's own path for it.
///
/// Never a hard-coded `target/debug/fossil`: with `CARGO_TARGET_DIR` set the
/// build lands elsewhere, so that path holds whatever was left there last and
/// the test passes against a binary it did not build. `CARGO_BIN_EXE_<name>`
/// is cargo's answer — the binary of THIS build, already built before the
/// test runs, with no path to guess and no `cargo build` from inside a test.
fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| PathBuf::from(env!("CARGO_BIN_EXE_fossil")))
}

// A program mixing `@conn` references (data + schema) in a destructuring io.rdf
// with a literal local-path csv — exercising both roles and both the aliased and
// unaliased forms. The two members share one (data, schema) pair.
//
// # What this fixture used to spell, and why it did not go red
//
// It opened `prefix ex: <https://ex.org/>` and closed with
// `KB : ex:KB from KB / iri = .subject / ex:label = .label` — four retired
// spellings in three lines (`retired::PREFIX_DECL`, `retired::ABSOLUTE_IRI`,
// `retired::CURIE`, `retired::LEADING_DOT`), and it stayed green through the
// whole of step 8. The reason is worth writing down rather than fixing
// quietly: `fossil refs` reads `DefMap::sources` and nothing else, so the only
// lines it can see are the two `:=` bindings — which were already in the live
// surface. The retired half was inert scenery. It parsed to errors the command
// does not consult, `refs` exits 0 regardless, and the assertions below never
// touched it.
//
// So the transcription changes what the program SAYS and not what the test
// PROVES, which is the point: the mapping is here so the fixture is a whole
// program, and a whole program written in a language nobody can compile proves
// less than no mapping at all. `type { … } := io.shex(…)` binds TYPES and takes
// no `SourceEntry` (`def_map.rs`, the `TYPE_DEF` arm), so adding the binding
// the bare shape name needs adds no ref — and if that ever changes, the
// `conns == [data, vocab]` assertion at the foot of each test is what says so.
const PROGRAM: &str = r#"type { Entry } := io.shex("@vocab/graph.shex")

{ KB, Project } := io.rdf("@data/graph.ttl", schema = io.shex("@vocab/graph.shex"))
plain := io.csv("local.csv")

Entries : Entry from KB
    @subject = "https://ex.org/kb/{KB.subject}"
    label = KB.label
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
