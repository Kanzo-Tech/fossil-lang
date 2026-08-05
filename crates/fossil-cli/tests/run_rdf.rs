//! `fossil run` over an `io.rdf` destructuring program — the single RDF path.
//!
//! The ONLY way to load RDF is the destructured form
//! `{ A, B, ... } := io.rdf("data.ttl", schema = "x.shex")`. The `.ttl` is read
//! and parsed ONCE per `io.rdf` call, yielding N typed relations (one per member).
//! Subject selection is ALWAYS by `rdf:type` (a shape's rows are the subjects
//! typed with its IRI) — there are NO `ShapeMaps`, no `select=`.
//!
//! The OUTPUT descriptor is program-resident: the `io.rdf(schema = "graph.shex")`
//! shape IS the output graph's shape, so the rich vertex/edge decomposition runs
//! and the result is a TYPED graph:
//!   - multi-shape vertices (`KB` + `Project`), each with its own columns;
//!   - a typed edge `KB --hasProject--> Project` (a `ShEx` shape-ref);
//!   - multi-valued: the two `hasProject` objects unroll (UNNEST) into two edges.
//!
//! This is the end-to-end proof of the RDF provider increment (multi-shape +
//! shape-ref edges + multi-valued UNNEST), the path keasy drives via subprocess.

#![cfg(not(target_arch = "wasm32"))]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

fn workdir_with_files(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("fossil-cli-rdf-{test_name}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create workdir");
    for (rel, contents) in files {
        let path = tmp.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture subdir");
        }
        std::fs::write(&path, contents).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }
    tmp
}

const GRAPH_TTL: &str = r#"@prefix ex: <https://ex.org/> .
<https://ex.org/kb/1> a ex:KB ; ex:label "Main KB" ; ex:hasProject <https://ex.org/proj/1>, <https://ex.org/proj/2> .
<https://ex.org/proj/1> a ex:Project ; ex:title "Alpha" .
<https://ex.org/proj/2> a ex:Project ; ex:title "Beta" .
"#;

// Multi-shape ShEx: `KB` has a literal `label` + a multi-valued (`*`) shape-ref
// `hasProject` → `Project` (an edge); `Project` has a literal `title`.
const GRAPH_SHEX: &str = r#"{ "@context": "http://www.w3.org/ns/shex.jsonld", "type": "Schema", "shapes": [
  {"type":"ShapeDecl","id":"https://ex.org/KB","shapeExpr":{"type":"Shape","expression":{"type":"EachOf","expressions":[
     {"type":"TripleConstraint","predicate":"https://ex.org/label","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}},
     {"type":"TripleConstraint","predicate":"https://ex.org/hasProject","valueExpr":"https://ex.org/Project","min":0,"max":-1}
  ]}}},
  {"type":"ShapeDecl","id":"https://ex.org/Project","shapeExpr":{"type":"Shape","expression":{
     "type":"TripleConstraint","predicate":"https://ex.org/title","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}}}}
] }"#;

// The single path: one destructuring `io.rdf` (read once) binding both members
// `KB` and `Project` — each member's local-name is a shape in the schema, and
// its rows are selected by `rdf:type`. NO `.smap`, no `select=`.
const CPI_FOSSIL: &str = r#"prefix ex: <https://ex.org/>

{ KB, Project } := io.rdf("graph.ttl", schema = "graph.shex")

KB : ex:KB from KB
    iri = .subject
    ex:label = .label
    ex:hasProject = .hasProject

Project : ex:Project from Project
    iri = .subject
    ex:title = .title
"#;

#[test]
fn run_rdf_writes_typed_multi_shape_graph_with_multivalued_edges() {
    let bin = fossil_binary();
    let workdir = workdir_with_files(
        "typed-graph",
        &[
            ("graph.ttl", GRAPH_TTL),
            ("graph.shex", GRAPH_SHEX),
            ("cpi.fossil", CPI_FOSSIL),
        ],
    );
    let dest = workdir.join("out");
    let dest_url = format!("file://{}", dest.display());

    let output = Command::new(bin)
        .args(["run", "cpi.fossil", "--dest", &dest_url, "--output-json"])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil run");
    assert!(
        output.status.success(),
        "fossil run (rdf) exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--output-json parseable");

    // Two vertex types, each materialised from ITS shape (multi-shape: the `KB`
    // member did not get the `Project` shape's columns or vice-versa).
    let vertices = parsed["vertices"].as_array().expect("vertices array");
    let kb = vertices
        .iter()
        .find(|v| v["type"] == "KB")
        .expect("KB vertex");
    assert_eq!(kb["count"].as_i64(), Some(1), "one KB; got: {kb}");
    let kb_cols: Vec<&str> = kb["columns"]
        .as_array()
        .expect("kb columns")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert_eq!(kb_cols, vec!["label"], "KB carries its own column; got: {kb_cols:?}");

    let project = vertices
        .iter()
        .find(|v| v["type"] == "Project")
        .expect("Project vertex");
    assert_eq!(project["count"].as_i64(), Some(2), "two Projects; got: {project}");
    let proj_cols: Vec<&str> = project["columns"]
        .as_array()
        .expect("project columns")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert_eq!(proj_cols, vec!["title"], "Project carries its own column; got: {proj_cols:?}");

    // The shape-ref `hasProject` is a TYPED edge KB→Project, NOT a column on KB.
    assert!(
        !kb_cols.contains(&"hasProject"),
        "hasProject is an edge, not a KB column; got: {kb_cols:?}"
    );
    let edges = parsed["edges"].as_array().expect("edges array");
    let edge = edges
        .iter()
        .find(|e| e["src_type"] == "KB" && e["dst_type"] == "Project")
        .unwrap_or_else(|| panic!("no KB→Project edge: {parsed}"));
    // Multi-valued: kb/1 references two projects → the LIST UNNESTs to two edges.
    assert_eq!(
        edge["count"].as_i64(),
        Some(2),
        "multi-valued hasProject unrolls to 2 edges; got: {edge}"
    );

    // The GraphAr edge Parquet pair + per-type vertex chunks exist on disk.
    //
    // Chunks, not `vertex/<Type>.parquet`: that single file is the layout pass's
    // input and `c678e63` deletes it once the chunks the manifest has always
    // declared are written. Asserting it still exists is asserting the writer
    // leaves a stale second copy of every vertex behind.
    assert!(
        dest.join("vertex/KB/chunk0.parquet").exists(),
        "KB vertex chunk"
    );
    assert!(
        dest.join("vertex/Project/chunk0.parquet").exists(),
        "Project vertex chunk"
    );
    assert!(
        !dest.join("vertex/KB.parquet").exists(),
        "the staged single-file vertex Parquet is removed once chunked"
    );
    assert!(
        dest.join("edge/KB_hasProject_Project/by_source.parquet")
            .exists(),
        "edge CSR Parquet"
    );
}

// The `.fossil` for the @conn test: data AND schema are `@conn` references — the
// data in `@data`, the ShEx in `@vocab` — proving every URI-valued argument
// resolves uniformly through the connection map (not just the positional data).
const CPI_FOSSIL_CONN: &str = r#"prefix ex: <https://ex.org/>

{ KB, Project } := io.rdf("@data/graph.ttl", schema = "@vocab/graph.shex")

KB : ex:KB from KB
    iri = .subject
    ex:label = .label
    ex:hasProject = .hasProject

Project : ex:Project from Project
    iri = .subject
    ex:title = .title
"#;

#[test]
fn run_rdf_resolves_schema_through_a_connection() {
    let bin = fossil_binary();
    // Data lives under `data/`, the ShEx under `vocab/` — referenced as `@data/…`
    // and `@vocab/…`. This is the host-boundary contract: keasy pipes a
    // `connections` map (name → base URL + secret) on stdin, fossil resolves
    // EVERY reference through it (not just the positional data URI) and reads them
    // all via `read_text`. The connection URLs are plain absolute dirs (a local
    // stand-in for `az://`/`s3://`; the resolution + `read_text` path is identical
    // for cloud).
    let workdir = workdir_with_files(
        "conn-refs",
        &[
            ("data/graph.ttl", GRAPH_TTL),
            ("vocab/graph.shex", GRAPH_SHEX),
            ("cpi.fossil", CPI_FOSSIL_CONN),
        ],
    );
    let dest = workdir.join("out");
    let dest_url = format!("file://{}", dest.display());

    let creds = serde_json::json!({
        "connections": {
            "data": { "url": workdir.join("data").to_string_lossy() },
            "vocab": { "url": workdir.join("vocab").to_string_lossy() },
        }
    })
    .to_string();

    let mut child = Command::new(bin)
        .args([
            "run",
            "cpi.fossil",
            "--dest",
            &dest_url,
            "--output-json",
            "--creds-stdin",
        ])
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fossil run");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(creds.as_bytes())
        .expect("write creds to stdin");
    let output = child.wait_with_output().expect("wait fossil run");

    assert!(
        output.status.success(),
        "fossil run (@conn refs) exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
            .expect("--output-json parseable");

    // Same typed graph as the local-files test — but every artifact came through
    // a `@conn` reference, proving the schema is no longer std::fs-bound.
    let vertices = parsed["vertices"].as_array().expect("vertices array");
    assert_eq!(
        vertices
            .iter()
            .find(|v| v["type"] == "KB")
            .and_then(|v| v["count"].as_i64()),
        Some(1),
        "one KB; got: {parsed}"
    );
    assert_eq!(
        vertices
            .iter()
            .find(|v| v["type"] == "Project")
            .and_then(|v| v["count"].as_i64()),
        Some(2),
        "two Projects; got: {parsed}"
    );
    let edge = parsed["edges"]
        .as_array()
        .expect("edges array")
        .iter()
        .find(|e| e["src_type"] == "KB" && e["dst_type"] == "Project")
        .unwrap_or_else(|| panic!("no KB→Project edge: {parsed}"));
    assert_eq!(
        edge["count"].as_i64(),
        Some(2),
        "multi-valued hasProject via @conn refs; got: {edge}"
    );
}

// ── Regression: edges to a property-less (leaf) shape ───────────────────────

// A leaf type — only an edge target, no properties of its own (`Tag` here, like
// keasy's `IfcBuilding`/`LCAPhase`). Its mapping is `iri = .subject` and nothing
// else, so it emits no triples; the codegen base relation must still project the
// real subject (not the NULL placeholder) or every edge pointing at it silently
// resolves to ZERO rows (`base_relation_sql`'s emit-less branch).
const LEAF_TTL: &str = r"@prefix ex: <https://ex.org/> .
<https://ex.org/item/1> a ex:Item ; ex:tag <https://ex.org/tag/red> .
<https://ex.org/tag/red> a ex:Tag .
";

const LEAF_SHEX: &str = r#"{ "@context": "http://www.w3.org/ns/shex.jsonld", "type": "Schema", "shapes": [
  {"type":"ShapeDecl","id":"https://ex.org/Item","shapeExpr":{"type":"Shape","expression":
     {"type":"TripleConstraint","predicate":"https://ex.org/tag","valueExpr":"https://ex.org/Tag","min":0,"max":-1}}},
  {"type":"ShapeDecl","id":"https://ex.org/Tag","shapeExpr":{"type":"Shape"}}
] }"#;

// `Tag` is a property-less member (a leaf edge target) — a subset of the schema's
// shapes is allowed; here both shapes are bound. One read-once `io.rdf`.
const LEAF_FOSSIL: &str = r#"prefix ex: <https://ex.org/>

{ Item, Tag } := io.rdf("graph.ttl", schema = "graph.shex")

Item : ex:Item from Item
    iri = .subject
    ex:tag = .tag

Tag : ex:Tag from Tag
    iri = .subject
"#;

#[test]
fn run_rdf_resolves_edges_to_a_property_less_leaf_shape() {
    let bin = fossil_binary();
    let workdir = workdir_with_files(
        "leaf-edge",
        &[
            ("graph.ttl", LEAF_TTL),
            ("graph.shex", LEAF_SHEX),
            ("cpi.fossil", LEAF_FOSSIL),
        ],
    );
    let dest = workdir.join("out");
    let dest_url = format!("file://{}", dest.display());

    let output = Command::new(bin)
        .args(["run", "cpi.fossil", "--dest", &dest_url, "--output-json"])
        .current_dir(&workdir)
        .output()
        .expect("spawn fossil run");
    assert!(
        output.status.success(),
        "fossil run (leaf edge) exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
            .expect("--output-json parseable");

    // The Tag vertex materialises (1 leaf node)...
    let tag = parsed["vertices"]
        .as_array()
        .expect("vertices")
        .iter()
        .find(|v| v["type"] == "Tag")
        .unwrap_or_else(|| panic!("no Tag vertex: {parsed}"));
    assert_eq!(tag["count"].as_i64(), Some(1), "one Tag; got: {tag}");

    // ...and the edge to it resolves to ITS row, not ZERO (the bug). Before the
    // fix, Tag's `subject` was a NULL placeholder so this inner-joined to 0.
    let edge = parsed["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .find(|e| e["src_type"] == "Item" && e["dst_type"] == "Tag")
        .unwrap_or_else(|| panic!("no Item→Tag edge: {parsed}"));
    assert_eq!(
        edge["count"].as_i64(),
        Some(1),
        "edge to a property-less leaf shape must resolve (was 0 before the base-relation fix); got: {edge}"
    );
}
