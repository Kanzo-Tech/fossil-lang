//! `fossil_wasm::refs_native` — the in-process (no-JS) core behind the WASM
//! `refs()` export, and **the side with the consumer**: keasy runs this through
//! `@fossil-lang/wasm` and spawns no `fossil` binary, so the CLI verb its twin
//! drives is the surface and this is the shipping path. Proves the BROWSER path
//! (a transient `WasmDb` +
//! `fossil_lineage::source_refs`) produces the SAME typed lineage as the native
//! `fossil refs` CLI, over the SAME program shape: `@conn` data + schema refs in
//! a destructuring `io.rdf` plus an unaliased local csv. Parse-only — no `DuckDB`,
//! no JS runtime (the `#[wasm_bindgen]` `refs()` wrapper's `serde_wasm_bindgen`
//! call panics on native, so we exercise the `refs_native` core — the same
//! `check` ↔ `check_rows` split this crate uses throughout).

#![cfg(not(target_arch = "wasm32"))]

use fossil_lineage::RefRole;
use fossil_wasm::refs_native;

// Mirrors `fossil-cli/tests/refs.rs`, byte for byte, because a parity test whose
// two sides read different programs proves parity of nothing: two destructuring
// members share one (data, schema) pair (must dedup to one each), plus an
// unaliased local csv.
//
// It opened `prefix ex: <https://ex.org/>` and closed on a CURIE mapping with
// leading-dot references, and was green for the same reason
// its native twin was — `source_refs` reads `DefMap::sources`, so the only lines
// it can see are the two `:=` bindings, and those were already in the live
// surface. The retired half never reached an assertion. See that file's header
// for the whole of it.
const PROGRAM: &str = r#"type { Entry } := io.shex("@vocab/graph.shex")

{ KB, Project } := io.rdf("@data/graph.ttl", schema = io.shex("@vocab/graph.shex"))
plain := io.csv("local.csv")

Entries : Entry from KB
    @subject = "https://ex.org/kb/{KB.subject}"
    label = KB.label
"#;

#[test]
fn refs_native_lists_typed_references_with_connection_aliases() {
    let refs = refs_native(PROGRAM);

    let find = |conn: &str, role: RefRole| {
        refs.iter()
            .find(|r| r.connection.as_deref() == Some(conn) && r.role == role)
            .unwrap_or_else(|| panic!("no {role:?} ref for @{conn}: {refs:?}"))
    };
    assert_eq!(find("data", RefRole::Data).path, "graph.ttl");
    // The schema ref — invisible to a regex over the data URI — carries @vocab.
    assert_eq!(find("vocab", RefRole::Schema).path, "graph.shex");

    // The destructuring members share one (data, schema): refs are deduplicated.
    let data_refs = refs
        .iter()
        .filter(|r| r.connection.as_deref() == Some("data") && r.role == RefRole::Data)
        .count();
    assert_eq!(data_refs, 1, "data ref deduped across members: {refs:?}");
    let schema_refs = refs
        .iter()
        .filter(|r| r.connection.as_deref() == Some("vocab") && r.role == RefRole::Schema)
        .count();
    assert_eq!(
        schema_refs, 1,
        "schema ref deduped across members: {refs:?}"
    );

    // The literal local path has no connection alias.
    let plain = refs
        .iter()
        .find(|r| r.path == "local.csv")
        .expect("local.csv ref");
    assert_eq!(plain.connection, None);
    assert_eq!(plain.role, RefRole::Data);

    // The distinct connections a job would attach: exactly {data, vocab}.
    let mut conns: Vec<&str> = refs
        .iter()
        .filter_map(|r| r.connection.as_deref())
        .collect();
    conns.sort_unstable();
    conns.dedup();
    assert_eq!(conns, vec!["data", "vocab"]);
}
