//! An `io.rdf` destructuring program through the executor — the single RDF path.
//!
//! The ONLY way to load RDF is the destructured form
//! `{ A, B, ... } := io.rdf("data.ttl", schema = io.shex("x.shex"))`. The `.ttl` is read
//! and parsed ONCE per `io.rdf` call, yielding N typed relations (one per member).
//! Subject selection is ALWAYS by `rdf:type` (a shape's rows are the subjects
//! typed with its IRI) — there are NO `ShapeMaps`, no `select=`.
//!
//! The OUTPUT descriptor is program-resident: the `io.rdf(schema = io.shex("graph.shex"))`
//! shape IS the output graph's shape, so the rich vertex/edge decomposition runs
//! and the result is a TYPED graph:
//!   - multi-shape vertices (`KB` + `Project`), each with its own columns;
//!   - a typed edge `KB --hasProject--> Project` (a `ShEx` shape-ref);
//!   - multi-valued: the two `hasProject` objects unroll (UNNEST) into two edges.

#![cfg(not(target_arch = "wasm32"))]

use fossil_sinks::manifest::{EdgeTable, Manifest, VertexTable};

#[path = "support/native.rs"]
mod native;
mod support;

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

// One destructuring `io.rdf` (read once) binding both members `KB` and
// `Project`. The document is named TWICE and neither naming is redundant:
// `io.rdf(schema = …)` types the ROWS that come out of the graph; the
// `type { … }` binding is what puts shape NAMES in scope for a mapping header.
// The vertex type is still `KB` because it comes from the shape's own IRI,
// never from the local label.
const CPI_FOSSIL: &str = r#"type { KBShape, ProjectShape } := io.shex("graph.shex")

{ KB, Project } := io.rdf("graph.ttl", schema = io.shex("graph.shex"))

KBs : KBShape from KB
    @subject = KB.subject
    label = KB.label
    hasProject = KB.hasProject

Projects : ProjectShape from Project
    @subject = Project.subject
    title = Project.title
"#;

fn vertex<'m>(m: &'m Manifest, name: &str) -> &'m VertexTable {
    m.vertex_tables
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no {name} vertex table: {m:?}"))
}

fn edge<'m>(m: &'m Manifest, source: &str, destination: &str) -> &'m EdgeTable {
    m.edge_tables
        .iter()
        .find(|e| e.source.references == source && e.destination.references == destination)
        .unwrap_or_else(|| panic!("no {source}→{destination} edge table: {m:?}"))
}

fn columns(v: &VertexTable) -> Vec<&str> {
    v.properties.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn run_rdf_writes_typed_multi_shape_graph_with_multivalued_edges() {
    let dir = native::write_dir(&[
        ("graph.ttl", GRAPH_TTL),
        ("graph.shex", GRAPH_SHEX),
        ("cpi.fossil", CPI_FOSSIL),
    ]);
    let corpus = native::run_dir(dir.path(), "cpi.fossil", &[]).expect("the RDF program runs");
    let m = corpus.manifest();

    // Two vertex types, each materialised from ITS shape (multi-shape: the `KB`
    // member did not get the `Project` shape's columns or vice-versa).
    let kb = vertex(&m, "KB");
    assert_eq!(kb.record_count, 1);
    assert_eq!(
        columns(kb),
        ["dense_id", "subject", "label"],
        "KB carries its own column after the writer's two"
    );
    let project = vertex(&m, "Project");
    assert_eq!(project.record_count, 2);
    assert_eq!(columns(project), ["dense_id", "subject", "title"]);

    // The shape-ref `hasProject` is a TYPED edge KB→Project, NOT a column on KB;
    // kb/1 references two projects → the LIST UNNESTs to two edges.
    let has_project = edge(&m, "KB", "Project");
    assert_eq!(has_project.label, "hasProject");
    assert_eq!(has_project.record_count, 2);
    // Both objects named a Project the graph carries, so nothing was discarded.
    assert_eq!(
        corpus
            .report
            .dropped
            .iter()
            .find(|d| d.table == has_project.name)
            .map(|d| d.dropped),
        Some(0)
    );
    for table in [&kb.path, &project.path, &has_project.path] {
        assert!(corpus.files.contains_key(table), "{table} is not there");
    }
}

// Data AND schema are `@conn` references — the data in `@data`, the ShEx in
// `@vocab` — proving every URI-valued argument resolves through the connection
// map, not just the positional data. The `type { … }` binding reads the same
// document by a program-relative path; what this test is FOR is the `schema =`
// argument, which is the reference a regex over the data URI cannot see.
const CPI_FOSSIL_CONN: &str = r#"type { KBShape, ProjectShape } := io.shex("vocab/graph.shex")

{ KB, Project } := io.rdf("@data/graph.ttl", schema = io.shex("@vocab/graph.shex"))

KBs : KBShape from KB
    @subject = KB.subject
    label = KB.label
    hasProject = KB.hasProject

Projects : ProjectShape from Project
    @subject = Project.subject
    title = Project.title
"#;

#[test]
fn run_rdf_resolves_schema_through_a_connection() {
    let dir = native::write_dir(&[
        ("data/graph.ttl", GRAPH_TTL),
        ("vocab/graph.shex", GRAPH_SHEX),
        ("cpi.fossil", CPI_FOSSIL_CONN),
    ]);
    let data = format!("{}data", native::AUTHORITY);
    let vocab = format!("{}vocab", native::AUTHORITY);
    let corpus = native::run_dir(
        dir.path(),
        "cpi.fossil",
        &[("data", data.as_str()), ("vocab", vocab.as_str())],
    )
    .expect("the @conn program runs");
    let m = corpus.manifest();

    assert_eq!(vertex(&m, "KB").record_count, 1);
    assert_eq!(vertex(&m, "Project").record_count, 2);
    assert_eq!(edge(&m, "KB", "Project").record_count, 2);
}

// ── Regression: edges to a property-less (leaf) shape ───────────────────────

// A leaf type — only an edge target, no properties of its own. Its mapping is
// `@subject` and nothing else; the base relation must still project the real
// subject (not a NULL placeholder) or every edge pointing at it silently
// resolves to ZERO rows.
const LEAF_TTL: &str = r"@prefix ex: <https://ex.org/> .
<https://ex.org/item/1> a ex:Item ; ex:tag <https://ex.org/tag/red> .
<https://ex.org/tag/red> a ex:Tag .
";

const LEAF_SHEX: &str = r#"{ "@context": "http://www.w3.org/ns/shex.jsonld", "type": "Schema", "shapes": [
  {"type":"ShapeDecl","id":"https://ex.org/Item","shapeExpr":{"type":"Shape","expression":
     {"type":"TripleConstraint","predicate":"https://ex.org/tag","valueExpr":"https://ex.org/Tag","min":0,"max":-1}}},
  {"type":"ShapeDecl","id":"https://ex.org/Tag","shapeExpr":{"type":"Shape"}}
] }"#;

const LEAF_FOSSIL: &str = r#"type { ItemShape, TagShape } := io.shex("graph.shex")

{ Item, Tag } := io.rdf("graph.ttl", schema = io.shex("graph.shex"))

Items : ItemShape from Item
    @subject = Item.subject
    tag = Item.tag

Tags : TagShape from Tag
    @subject = Tag.subject
"#;

#[test]
fn run_rdf_resolves_edges_to_a_property_less_leaf_shape() {
    let dir = native::write_dir(&[
        ("graph.ttl", LEAF_TTL),
        ("graph.shex", LEAF_SHEX),
        ("cpi.fossil", LEAF_FOSSIL),
    ]);
    let corpus = native::run_dir(dir.path(), "cpi.fossil", &[]).expect("the leaf program runs");
    let m = corpus.manifest();

    assert_eq!(vertex(&m, "Tag").record_count, 1, "one leaf Tag");
    assert_eq!(
        edge(&m, "Item", "Tag").record_count,
        1,
        "an edge to a property-less leaf shape resolves to its row, not to zero"
    );
}
