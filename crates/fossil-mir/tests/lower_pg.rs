//! E2E del lowering property-graph (paso 2, vertex-only): parse `hello.fossil`
//! → [`lower_to_mir_pg`] → `Source → EmitVertex(Person) → Sink`.
//!
//! Fija además la disciplina de fallo: un `from` que no resuelve a una fuente
//! TIÑE el grafo en vez de sustituir un valor por defecto.

// Every `@subject` below is an interpolated string, and `{User.id}` is fossil's
// hole, not a Rust format argument. The lint reads the Rust literal and cannot
// know that.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::test_support::db_with_document_at;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_hir::def_map::def_map;
use fossil_mir::{Op, lower_to_mir_pg};

// ── the two halves a program's shape contract needs, and neither is the disk ──
//
// A property key is a bare name whose meaning is the last segment of a predicate
// IRI THE DOCUMENT DECLARES, which is why naming a shape document is MANDATORY:
// a program that names no document carries no `rdf_uri` on any property and
// the graph type has nothing to match against. Two things are needed and
// both are easy to half-do:
//
//  1. a HOST WITH A TYPE-READING ROW — `NativeSystem::providers` is the trait
//     default, the four rows that read DATA, so nothing reads types and a
//     `.shex` it reads decodes to nothing, silently;
//  2. the document REGISTERED as a Salsa input — `decoded_document` resolves
//     through `fossil_base::file_at`, which reads the registry and never the
//     disk, so writing the file next to the program is invisible.
//
// `fossil_base::test_support` supplies both. It is a dev-dependency on a
// dev-only feature of a crate this one already depends on, so the test names
// no schema language.

/// The shape `hello.fossil` targets: one un-narrowed `name` predicate, whose
/// IRI is the one the vertex prop must carry.
const HELLO_SHAPE: &str = "\
shape https://example.org/Person
prop https://example.org/name - 1 1
";

const HELLO: &str = "\
type { Person } := io.shex(\"hello.shex\")

User := io.csv(\"examples/users.csv\")

People : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";

#[test]
fn lower_pg_emits_source_vertex_sink() {
    let (db, file) = db_with_document_at("hello.fossil", HELLO, "hello.shex", HELLO_SHAPE);
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("hello.fossil must contain one mapping");

    let mir = lower_to_mir_pg(&db, mapping);
    let ops = mir.ops(&db);

    // Source → EmitVertex → Sink (a literal-only vertex synthesises no edges).
    assert_eq!(ops.len(), 3, "Source → EmitVertex → Sink, got {ops:?}");
    assert!(matches!(ops[0], Op::Source { .. }), "op[0] = Source");
    assert!(matches!(ops[2], Op::Sink { .. }), "op[2] = Sink");

    let Op::EmitVertex {
        type_name,
        rdf_type,
        props,
        ..
    } = &ops[1]
    else {
        panic!("op[1] must be EmitVertex, got {:?}", ops[1]);
    };
    assert_eq!(
        type_name, "Person",
        "vertex type = subject shape local name"
    );
    assert_eq!(
        rdf_type.as_deref(),
        Some("https://example.org/Person"),
        "rdf_type = full shape IRI"
    );
    let name = props
        .iter()
        .find(|p| p.name == "name")
        .expect("the `name = User.name` column reference becomes a vertex prop");
    assert_eq!(
        name.rdf_uri.as_deref(),
        Some("https://example.org/name"),
        "prop carries its predicate IRI for the layout and the mapping"
    );
}

/// A mapping reading `from` a DERIVED binding must taint, not silently read
/// some other file.
///
/// A substitute — the walking-skeleton fixture, say — for a source binding the
/// lowering could not resolve would execute a mapping over a real connection
/// against the wrong file, producing a full, plausible, entirely wrong graph.
/// The graph is poisoned instead and carries no ops.
///
/// `where` is a pipeline verb the lowering walks (`lower_source_chain`), so a
/// derived binding resolves through to its base and the unresolvable name is
/// that base. The recursion is the part worth
/// pinning: an error under a pipeline has one more frame to be swallowed in
/// than an error at the top, and `lower_source_chain` propagates it.
#[test]
fn derived_binding_poisons_instead_of_defaulting() {
    let src = "\
filtered := Rows.where(Rows.kind == \"https://example.org/wanted\")

Thing : Thing from filtered
    @subject = \"https://example.org/thing/{Rows.id}\"
    name = Rows.name
";
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "derived.fossil".to_string());
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");

    let mir = lower_to_mir_pg(&db, mapping);

    assert!(
        mir.error(&db).is_some(),
        "an unresolvable source binding must poison the graph"
    );
    assert!(
        mir.ops(&db).is_empty(),
        "a poisoned graph carries no ops, so nothing can execute it by accident"
    );
    // Asserted on the DIAGNOSTIC rather than on the ops: the problem is the
    // only observable that distinguishes
    // «poisoned because the source did not resolve» from «poisoned because the
    // body failed to type-check», and the two would both satisfy every
    // assertion above.
    let diags = mir.diagnostics(&db);
    assert!(
        diags.iter().any(|d| matches!(
            &d.problem,
            fossil_graph_schema::Problem::UnknownSource { binding, bound_to: None } if binding == "Rows"
        )),
        "the refusal names the binding that did not resolve, got: {:?}",
        diags.iter().map(|d| &d.problem).collect::<Vec<_>>(),
    );
}

// ── Edges and cardinality from the program's graph type ────────────────────

// An io.rdf mapping: the reference property `hasProject` is written as a plain
// qualified column reference (`Graph.hasProject`), so the body alone CANNOT tell
// it from a literal column — only the graph type knows it's an edge to `Project`
// (and multi-valued). This is the run_rdf.rs case at the MIR level.
const KB_FOSSIL: &str = "\
type { KB, Project } := io.shex(\"kb.shex\")

Graph := io.rdf(\"graph.ttl\")

Facts : KB from Graph
    @subject = Graph.subject
    label = Graph.label
    hasProject = Graph.hasProject
";

/// `KB`: a single-valued literal `label` and a multi-valued `hasProject` whose
/// range is `Project`, which is what makes it an edge.
const KB_SHAPE: &str = "\
shape https://ex.org/KB
prop https://ex.org/label - 1 1
prop https://ex.org/hasProject @https://ex.org/Project 0 *
shape https://ex.org/Project
prop https://ex.org/title - 1 1
";

#[test]
fn a_property_whose_range_is_a_type_is_lowered_as_an_edge() {
    let (db, file) = db_with_document_at("kb.fossil", KB_FOSSIL, "kb.shex", KB_SHAPE);
    let kb = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("`Facts` is the first mapping");

    let lowered = lower_to_mir_pg(&db, kb);
    let refined = lowered.ops(&db);

    // `label` stays a vertex prop (single-valued); `hasProject` leaves the props.
    let Op::EmitVertex { props, .. } = refined
        .iter()
        .find(|o| matches!(o, Op::EmitVertex { .. }))
        .expect("an EmitVertex")
    else {
        unreachable!()
    };
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["label"], "the shape-ref left the vertex props");
    assert!(
        props
            .iter()
            .find(|p| p.name == "label")
            .unwrap()
            .single_valued,
        "label is Cardinality::Single → single-valued"
    );

    // `hasProject` is a typed edge KB→Project, multi-valued (max:-1).
    let edge = refined
        .iter()
        .find_map(|o| match o {
            Op::EmitEdge {
                edge_type,
                src_type,
                dst_type,
                single_valued,
                ..
            } => Some((
                edge_type.to_string(),
                src_type.to_string(),
                dst_type.to_string(),
                *single_valued,
            )),
            _ => None,
        })
        .expect("hasProject became an EmitEdge");
    assert_eq!(
        edge,
        ("hasProject".into(), "KB".into(), "Project".into(), false),
        "shape-ref → typed edge KB→Project, multi-valued (single_valued=false)"
    );

    // The Sink still consumes the last op (re-pointed past the new edge).
    let Some(Op::Sink { input, .. }) = refined.last() else {
        panic!("last op must be the Sink");
    };
    assert_eq!(*input, refined.len() - 2, "Sink consumes the op before it");
}
