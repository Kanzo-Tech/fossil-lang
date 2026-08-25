//! E2E del lowering property-graph (paso 2, vertex-only): parse `hello.fossil`
//! → [`lower_to_mir_pg`] → `Source → EmitVertex(Person) → Sink`.
//!
//! Fija además la disciplina de fallo: un `from` que no resuelve a una fuente
//! TIÑE el grafo en vez de sustituir un valor por defecto.

#![cfg(not(target_arch = "wasm32"))]
// Every `@subject` below is an interpolated string, and `{User.id}` is fossil's
// hole, not a Rust format argument. The lint reads the Rust literal and cannot
// know that.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::test_support::db_with_document_at;
use fossil_base::{Diagnostic, FossilDb, SourceFile, System};
use fossil_graph_schema::{Cardinality, EdgeType, GraphSchema, NodeType, Primitive, Property};
use fossil_hir::def_map::def_map;
use fossil_mir::{Op, apply_output_shape, lower_to_mir_pg};

// ── the two halves a program's shape contract needs, and neither is the disk ──
//
// A property key is a bare name whose meaning is the last segment of a predicate
// IRI THE DOCUMENT DECLARES, which is why naming a shape document is MANDATORY:
// a program that names no document carries no `rdf_uri` on any property and
// `apply_output_shape` has nothing to match against. Two things are needed and
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
// dev-only feature of a crate this one already depends on: no schema language
// enters `fossil-mir`, which is the cut `0e6898d` made and this test nearly
// undid.

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
        "prop carries its predicate IRI for the manifest/DCAT"
    );
}

/// A mapping reading `from` a DERIVED binding must taint, not silently read
/// some other file.
///
/// Lowering used to substitute `examples/users.csv` for a source binding it
/// could not resolve — so a mapping over a real connection executed against the
/// walking-skeleton fixture instead, producing a full, plausible, entirely
/// wrong graph. The substitution is gone: the graph is poisoned and carries no
/// ops.
///
/// What moved is WHERE the unresolvable name sits. `x := Source.where(...)`
/// was the fixture because it parsed as a source definition carrying no `io.*`
/// constructor; `where` is a pipeline verb the lowering walks now
/// (`lower_source_chain`), so a derived binding resolves through to its base
/// and the unresolvable name is that base. The recursion is the part worth
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
    // The specific regression, and it is asserted on the DIAGNOSTIC rather
    // than on the ops. A loop over `ops` looking for `examples/users.csv` is
    // what stood here, and it ran zero times against the empty vec the
    // assertion above had just demanded — a guard that reads as a guard and
    // proves nothing. The message is the only observable that distinguishes
    // «poisoned because the source did not resolve» from «poisoned because the
    // body failed to type-check», and the two would both satisfy every
    // assertion above.
    let diags = lower_to_mir_pg::accumulated::<Diagnostic>(&db, mapping);
    let refusal = "`Rows` is not a declared source binding";
    assert!(
        diags.iter().any(|d| d.message.contains(refusal)),
        "the refusal names the binding that did not resolve, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
}

// `EDGES` + `lower_pg_classifies_edge_vs_prop` lived here, and what they proved
// is worth writing down because it has to be re-proved against the constructor.
//
// Given two mappings over two sources — `Person` keyed
// `@subject = "https://example.org/person/{User.id}"` and `Order` keyed
// `@subject = "https://example.org/order/{Purchase.order_id}"` — the `Order`
// body wrote three non-identity properties and the lowering sorted them three
// ways:
//
//   * `placedBy = "https://example.org/person/{Purchase.user_id}"` →
//     `Op::EmitEdge { edge_type: "placedBy", dst_type: "Person" }`. The
//     template's SKELETON (`https://example.org/person/{}`, every per-row hole
//     replaced by a marker) equalled the skeleton of `Person`'s subject, so the
//     value was taken to be a reference to a `Person`.
//   * `total = Purchase.amount` → a vertex prop. `EmitVertex`'s props were
//     exactly `["total"]`.
//   * `external = "https://example.org/widget/{Purchase.wid}"` → NEITHER. A
//     template whose skeleton matched no mapping's subject was dropped: no
//     edge, and not a prop either.
//
// That mechanism is deleted (see the tombstone at `fossil_mir::lower`'s
// `subject_skeletons`): an edge was GUESSED by comparing strings, and the guess
// existed only because the identity rule was written once per mapping. There is
// now exactly ONE identity per type — every mapping producing `T` declares the
// same `@subject`, and disagreeing is an error — so the comparison becomes a
// lookup, and an edge is written by naming the target type:
// `buyer = Person(User.email)` builds that type's one subject template. The
// successor test drives the constructor, and the three outcomes it has to keep
// are the three above.
//
// The dangling case is the one most likely to be lost: a reference the program
// writes to a subject nothing constructs must still be well-formed and must not
// become a property. An edge is a reference and RDF
// is open-world, so nothing checks that the target exists — which makes "it
// silently became a column" the failure to guard against.

// ── Schema-driven refinement (GraphSchema → edges + cardinality) ────────────

// An io.rdf mapping: the reference property `hasProject` is written as a plain
// qualified column reference (`Graph.hasProject`), so the agnostic lowering
// CANNOT tell it from a literal column — only the graph schema knows it's an
// edge to `Project` (and multi-valued). This is the run_rdf.rs case at the MIR
// level.
const KB_FOSSIL: &str = "\
type { KB } := io.shex(\"kb.shex\")

Graph := io.rdf(\"graph.ttl\")

Facts : KB from Graph
    @subject = Graph.subject
    label = Graph.label
    hasProject = Graph.hasProject
";

/// `KB`'s output contract. It declares the two predicates the body writes, so
/// the props reach MIR carrying `rdf_uri`; what it does NOT say is which of them
/// is an edge in the property-graph sense — that is `kb_schema()`'s answer, and
/// keeping the two apart is the point of the test.
const KB_SHAPE: &str = "\
shape https://ex.org/KB
prop https://ex.org/label - 1 1
prop https://ex.org/hasProject - 0 *
";

// `KB`: a single-valued literal `label` + a multi-valued edge `hasProject` →
// `Project`; `Project`: a literal `title`. Written as the schema itself, not as
// the ShEx document that would derive it: which schema language produced this is
// exactly what MIR must not know (the ShEx path is covered end-to-end by
// `fossil-df/tests/rdf_source.rs`).
fn kb_schema() -> GraphSchema {
    let string_prop = |name: &str, iri: &str, cardinality| Property {
        name: name.to_string(),
        datatype: Primitive::String,
        iri: Some(iri.to_string()),
        cardinality,
    };
    GraphSchema {
        nodes: vec![
            NodeType {
                label: "KB".to_string(),
                iri: Some("https://ex.org/KB".to_string()),
                properties: vec![string_prop(
                    "label",
                    "https://ex.org/label",
                    Cardinality::Single,
                )],
            },
            NodeType {
                label: "Project".to_string(),
                iri: Some("https://ex.org/Project".to_string()),
                properties: vec![string_prop(
                    "title",
                    "https://ex.org/title",
                    Cardinality::Single,
                )],
            },
        ],
        edges: vec![EdgeType {
            label: "hasProject".to_string(),
            iri: Some("https://ex.org/hasProject".to_string()),
            source: "KB".to_string(),
            destination: "Project".to_string(),
            cardinality: Cardinality::Multi,
        }],
    }
}

#[test]
fn apply_output_shape_reclassifies_shape_ref_to_edge() {
    let (db, file) = db_with_document_at("kb.fossil", KB_FOSSIL, "kb.shex", KB_SHAPE);
    let kb = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("`Facts` is the first mapping");

    // Agnostic: both `label` and `hasProject` look like literal columns.
    let agnostic = lower_to_mir_pg(&db, kb);
    let ops = agnostic.ops(&db);
    let agnostic_props: Vec<String> = ops
        .iter()
        .find_map(|o| match o {
            Op::EmitVertex { props, .. } => {
                Some(props.iter().map(|p| p.name.to_string()).collect())
            }
            _ => None,
        })
        .expect("an EmitVertex");
    assert_eq!(
        agnostic_props,
        ["label", "hasProject"],
        "agnostic lowering can't tell the shape-ref from a literal"
    );
    assert!(
        !ops.iter().any(|o| matches!(o, Op::EmitEdge { .. })),
        "agnostic lowering synthesises no edge for a FieldRef value"
    );

    // The schema reclassifies `hasProject` into a typed, multi-valued edge.
    let refined = apply_output_shape(ops, &kb_schema());

    // `label` stays a vertex prop (single-valued); `hasProject` is gone from props.
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

    // `hasProject` is now a typed edge KB→Project, multi-valued (max:-1).
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
