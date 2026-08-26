//! The conformance corpus, executed against the Rust reader.
//!
//! `apps/corpus/conformance/expected.json` is a table of ADDRESSES: what a reader
//! must compose from a manifest and what it must refuse to compose. **Nothing here
//! opens a byte.** Three implementations execute that table and none of them wrote
//! it:
//!
//! | reader | executed by | what it is |
//! | --- | --- | --- |
//! | [`fossil_graph::address`] | this file, and `verify.mjs`'s wasm leg | the Rust one, and the one that reaches a browser through `fossil-graph-wasm` |
//! | `apps/corpus/conformance/reader.mjs` | `apps/corpus/conformance/verify.mjs` | plain Node, written from the conventions and from nothing else |
//! | `packages/graph/src/address.ts` | `packages/graph/tests/conformance.test.ts` | the published module |
//!
//! Two of them were here before this one, and the gap the third closes is named
//! at the top of `crates/fossil-engine/tests/conformance.rs`: that file is one
//! writer read by one engine, and what it cannot see is a reader disagreeing with
//! another reader. `GraphAr`'s fourth implementation landed having re-derived the
//! path arithmetic differently from the other three *and* from the corpus on disk,
//! green on both sides, because its tests asserted hand-written strings instead of
//! resolving against a shared table.
//!
//! **The error messages are part of the contract.** `expected.json` names the
//! substring a refusal must carry, and all three readers produce it — a corpus
//! that cannot address itself has to say the same thing in every language, or the
//! reader who gets the vague one debugs the wrong file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fossil_graph::address::{Direction, ResolvedCorpus, resolve};
use serde_json::Value;

/// Where the shared table and its case roots live. Not a copy: the same bytes
/// `verify.mjs` and `conformance.test.ts` read.
fn conformance_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/corpus/conformance")
}

/// Every manifest under a case root, keyed the way a host that fetched them would
/// key them — dataset-relative, forward slashes.
fn manifest_files(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read case root") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "yml") {
                let rel = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(rel, std::fs::read_to_string(&path).expect("read manifest"));
            }
        }
    }
    files
}

fn table() -> Value {
    let path = conformance_dir().join("expected.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("expected.json is JSON")
}

fn s(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn u(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or_else(|| {
        value[key]
            .as_str()
            .and_then(|t| t.parse().ok())
            .unwrap_or_else(|| panic!("{key} is not a row count: {}", value[key]))
    })
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

fn direction(value: &str) -> Direction {
    Direction::parse(value).unwrap_or_else(|| panic!("{value} is not an orientation"))
}

/// One case's corpus, resolved, or the refusal it is expected to be.
fn resolved(root: &Path) -> fossil_graph::Result<ResolvedCorpus> {
    resolve(&manifest_files(root), "")
}

/// The vertex and edge types the table declares, with their prefixes, tile sizes
/// and shifts. A type the reader found and the table does not name is as much a
/// disagreement as one it missed.
fn check_types(label: &str, case: &Value, corpus: &ResolvedCorpus) {
    for want in list(case, "types") {
        let got = corpus
            .vertex_type(Some(&s(want, "type")))
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(
            got.prefix,
            s(want, "prefix"),
            "{label}: {} prefix",
            got.vertex_type
        );
        assert_eq!(got.chunk_size, u(want, "chunk_size"), "{label}: chunk_size");
        assert_eq!(u64::from(got.shift), u(want, "shift"), "{label}: shift");
    }
    assert_eq!(
        corpus.types.len(),
        list(case, "types").len(),
        "{label}: the reader found a vertex type the table does not name, or missed one"
    );

    for want in list(case, "edges") {
        let edge_type = s(want, "edge_type");
        let got = corpus
            .edges
            .iter()
            .find(|e| e.edge_type == edge_type)
            .unwrap_or_else(|| panic!("{label}: no edge type {edge_type}"));
        assert_eq!(got.src_type, s(want, "src_type"), "{label}: src_type");
        assert_eq!(got.dst_type, s(want, "dst_type"), "{label}: dst_type");
        assert_eq!(got.prefix, s(want, "prefix"), "{label}: edge prefix");
        let directions: Vec<String> = got
            .directions
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        let wanted: Vec<String> = list(want, "directions")
            .iter()
            .map(|d| d.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(directions, wanted, "{label}: {edge_type} directions");

        for adjacency in list(want, "adjacencies") {
            let d = direction(&s(adjacency, "direction"));
            let a = got
                .adjacency(d)
                .unwrap_or_else(|| panic!("{label}: {edge_type} publishes no {d:?}"));
            assert_eq!(
                a.prefix,
                s(adjacency, "prefix"),
                "{label}: adjacency prefix"
            );
            assert_eq!(
                a.column,
                s(adjacency, "column"),
                "{label}: adjacency column"
            );
            assert_eq!(
                a.chunk_size,
                u(adjacency, "chunk_size"),
                "{label}: adjacency chunk_size"
            );
            assert_eq!(
                u64::from(a.shift),
                u(adjacency, "shift"),
                "{label}: adjacency shift"
            );
        }
    }
}

/// `dense_id >> shift`, at the borders the table names, and the URLs those tiles
/// compose to. Returns how many addresses were checked, because a table whose keys
/// were renamed would leave the loop running zero times with the test green.
fn check_addresses(label: &str, case: &Value, root: &Path, corpus: &ResolvedCorpus) -> usize {
    for want in list(case, "tile_of") {
        let vertex = corpus
            .vertex_type(Some(&s(want, "type")))
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let dense_id = u(want, "dense_id");
        assert_eq!(
            vertex.tile_of(dense_id),
            u(want, "tile"),
            "{label}: tile_of({dense_id}) in {}",
            vertex.vertex_type
        );
    }

    let on_disk = case["on_disk"].as_bool().unwrap_or(false);
    let mut checked = 0;
    for address in list(case, "addresses") {
        let tile = u(address, "tile");
        let got = if s(address, "kind") == "vertex" {
            corpus
                .vertex_type(Some(&s(address, "type")))
                .unwrap_or_else(|e| panic!("{label}: {e}"))
                .tile_url(tile)
        } else {
            let edge_type = s(address, "edge_type");
            corpus
                .edges
                .iter()
                .find(|e| e.edge_type == edge_type)
                .and_then(|e| e.adjacency(direction(&s(address, "direction"))))
                .unwrap_or_else(|| panic!("{label}: no address for {edge_type}"))
                .tile_url(tile)
        };
        assert_eq!(got, s(address, "path"), "{label}: composed the wrong URL");
        // The whole point, on the one case that has bytes: a composed URL names a
        // file that is there. A URL that composes cleanly and 404s in a browser is
        // the failure this seam exists to prevent — and it is a plain existence
        // check, because which engine would open the file is not this table's
        // question.
        if on_disk {
            assert!(
                root.join(&got).exists(),
                "{label}: {got} composes and is not on disk"
            );
        }
        checked += 1;
    }
    checked
}

/// An orientation the corpus does not publish, and a vertex type it does not
/// carry. Neither is ever a string that 404s. Returns how many refusals were checked.
fn check_refusals(label: &str, case: &Value, corpus: &ResolvedCorpus) -> usize {
    let mut checked = 0;
    for refused in list(case, "refused") {
        let edge_type = s(refused, "edge_type");
        let edge = corpus
            .edges
            .iter()
            .find(|e| e.edge_type == edge_type)
            .unwrap_or_else(|| panic!("{label}: no edge type {edge_type}"));
        let d = direction(&s(refused, "direction"));
        assert!(
            edge.adjacency(d).is_none() && !edge.directions.contains(&d),
            "{label}: {edge_type} handed back an address for {d:?}, which it does not publish"
        );
        checked += 1;
    }

    for want in list(case, "throws") {
        let name = s(want, "vertex_type");
        let expected = s(want, "message");
        let error = corpus
            .vertex_type(Some(&name))
            .err()
            .unwrap_or_else(|| panic!("{label}: naming vertex type {name} did not refuse"));
        let message = error.to_string();
        assert!(
            message.contains(&expected),
            "{label}: naming vertex type {name} said \"{message}\", not \"{expected}\""
        );
        checked += 1;
    }
    checked
}

/// The URLs a set of vertex tiles addresses, and what that set is complete for.
fn check_windows(label: &str, case: &Value, corpus: &ResolvedCorpus) {
    for (i, want) in list(case, "windows").iter().enumerate() {
        let vertex_type = want.get("type").and_then(Value::as_str);
        let tiles: Vec<u64> = list(want, "tiles")
            .iter()
            .map(|t| t.as_u64().expect("a tile number"))
            .collect();
        let directions: Vec<Direction> = list(want, "directions")
            .iter()
            .map(|d| direction(d.as_str().unwrap_or_default()))
            .collect();
        let got = corpus
            .window(vertex_type, &tiles, &directions)
            .unwrap_or_else(|e| panic!("{label}: window {i}: {e}"));
        assert_eq!(
            serde_json::to_value(&got.vertex_urls).unwrap(),
            want["vertex_urls"],
            "{label}: window {i} vertex_urls"
        );
        assert_eq!(
            serde_json::to_value(&got.edge_urls).unwrap(),
            want["edge_urls"],
            "{label}: window {i} edge_urls"
        );
        assert_eq!(
            got.complete, want["complete"],
            "{label}: window {i} complete"
        );
        assert_eq!(
            serde_json::to_value(&got.gaps).unwrap(),
            want["gaps"],
            "{label}: window {i} gaps"
        );
    }
}

#[test]
fn every_address_in_the_table_reproduces() {
    let table = table();
    let here = conformance_dir();
    let cases = list(&table, "cases");
    assert!(
        !cases.is_empty(),
        "non-vacuity: expected.json declares no case, so every assertion below is over an \
         empty loop"
    );

    let mut checked_addresses = 0usize;
    let mut checked_refusals = 0usize;

    for case in cases {
        let label = s(case, "name");
        let root = here.join(s(case, "root"));

        // A manifest that cannot address itself, and the reason it gives. The
        // substring is the contract: all three readers say the same thing, or the
        // reader who gets the vague one debugs the wrong file.
        if let Some(expected) = case.get("resolve_throws").and_then(Value::as_str) {
            let error = resolved(&root)
                .err()
                .unwrap_or_else(|| panic!("{label}: resolved a manifest that addresses nothing"));
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "{label}: refused with \"{message}\", which does not say \"{expected}\""
            );
            checked_refusals += 1;
            continue;
        }

        let corpus = resolved(&root).unwrap_or_else(|e| panic!("{label}: {e}"));

        // Which container carries the tiles. It is the one thing about a corpus a
        // reader cannot work out — working it out means listing a directory — so it
        // is a manifest field, pinned here rather than inferred from the paths it
        // decides.
        assert_eq!(
            serde_json::to_value(corpus.container).unwrap(),
            case["container"],
            "{label}: container"
        );

        check_types(&label, case, &corpus);
        checked_addresses += check_addresses(&label, case, &root, &corpus);
        checked_refusals += check_refusals(&label, case, &corpus);
        check_windows(&label, case, &corpus);
    }

    // Non-vacuity, and it is not ceremony: every loop above is over a table read
    // from a file, and a table whose keys were renamed would leave every one of
    // them running zero times with the test green.
    assert!(
        checked_addresses >= 10,
        "non-vacuity: {checked_addresses} address(es) checked, so the table was not read"
    );
    assert!(
        checked_refusals >= 3,
        "non-vacuity: {checked_refusals} refusal(s) checked"
    );
}

/// The base is prepended and nothing else happens to it.
#[test]
fn the_base_is_prepended_and_nothing_else() {
    let table = table();
    let here = conformance_dir();
    let join = &table["base_join"];
    let name = s(join, "case");
    let case = list(&table, "cases")
        .iter()
        .find(|c| s(c, "name") == name)
        .unwrap_or_else(|| panic!("base_join names case {name}, which the table does not carry"));

    let root = here.join(s(case, "root"));
    let corpus = resolve(&manifest_files(&root), &s(join, "base")).expect("resolve");
    let path = s(join, "path");
    let tile: u64 = path
        .rsplit_once("chunk")
        .and_then(|(_, tail)| tail.strip_suffix(".parquet"))
        .and_then(|k| k.parse().ok())
        .unwrap_or_else(|| panic!("base_join.path {path} names no tile"));

    assert_eq!(
        corpus
            .vertex_type(None)
            .expect("a vertex type")
            .tile_url(tile),
        s(join, "url"),
        "the base was not prepended verbatim"
    );
}
