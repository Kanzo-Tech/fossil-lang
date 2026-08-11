//! E2E del lowering property-graph (paso 2, vertex-only): parse `hello.fossil`
//! → [`lower_to_mir_pg`] → `Source → EmitVertex(Person) → Sink`.
//!
//! Fija además la disciplina de fallo: un `from` que no resuelve a una fuente
//! TIÑE el grafo en vez de sustituir un valor por defecto.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_graph_schema::{Cardinality, EdgeType, GraphSchema, NodeType, Primitive, Property};
use fossil_hir::def_map::def_map;
use fossil_mir::{Op, apply_output_shape, lower_to_mir_pg};

const HELLO: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

#[test]
fn lower_pg_emits_source_vertex_sink() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
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
        .expect("the `ex:name = .name` literal becomes a vertex prop");
    assert_eq!(
        name.rdf_uri.as_deref(),
        Some("https://example.org/name"),
        "prop carries its predicate IRI for the manifest/DCAT"
    );
}

/// A mapping reading `from` a DERIVED binding must taint, not silently read
/// some other file.
///
/// `x := Source |> seq.filter(...)` parses as a source definition (the parser
/// classifies every top-level `IDENT :=` that way) but carries no `io.*`
/// constructor and no URI. Lowering used to substitute `examples/users.csv` —
/// so a mapping over `@upv/aemet.csv` executed against the walking-skeleton
/// fixture instead, producing a full, plausible, entirely wrong graph. The
/// substitution is gone: the graph is poisoned and carries no ops.
#[test]
#[allow(clippy::literal_string_with_formatting_args)] // `${ex:}` is template syntax, not a Rust format arg
fn derived_binding_poisons_instead_of_defaulting() {
    let src = "\
prefix ex: <https://example.org/>

Rows := io.csv(\"@conn/real.csv\")

filtered := Rows |> seq.filter(.kind == \"https://example.org/wanted\")

Thing : ex:Thing from filtered
    iri = `${ex:}thing/${.id}`
    ex:name = .name
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
    // The specific regression: never reach for the fixture path.
    for op in mir.ops(&db) {
        if let Op::Source { uri, .. } = op {
            assert_ne!(
                uri.as_str(),
                "examples/users.csv",
                "the Phase-1 default is gone"
            );
        }
    }
}

const EDGES: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")
orders := io.csv(\"orders.csv\")

Person : ex:Person from users
    iri = `${ex:}person/${.id}`
    ex:name = .name

Order : ex:Order from orders
    iri = `${ex:}order/${.order_id}`
    ex:placedBy = `${ex:}person/${.user_id}`
    ex:total = .amount
    ex:external = `${ex:}widget/${.wid}`
";

/// The `Order` mapping's `ex:placedBy` template resolves (skeleton-match) to the
/// `Person` subject → `EmitEdge`; the literal `ex:total` → a vertex prop; the
/// dangling `ex:external` (no matching subject) → neither. Same classification
/// the codegen decomposition makes — now shared via `fossil_mir::skeleton`.
#[test]
fn lower_pg_classifies_edge_vs_prop() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, EDGES.to_string(), "edges.fossil".to_string());
    let order = *def_map(&db, file)
        .mappings(&db)
        .get(1)
        .expect("Order is the 2nd mapping");

    let graph = lower_to_mir_pg(&db, order);
    let ops = graph.ops(&db);

    // EmitVertex(Order): `total` is the only literal prop.
    let (vtype, prop_names) = ops
        .iter()
        .find_map(|o| match o {
            Op::EmitVertex {
                type_name, props, ..
            } => Some((
                type_name.to_string(),
                props.iter().map(|p| p.name.to_string()).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .expect("an EmitVertex");
    assert_eq!(vtype, "Order");
    assert_eq!(
        prop_names,
        ["total"],
        "only the literal property is a vertex prop"
    );

    // placedBy → Person edge; external is dangling → no edge.
    let edges: Vec<(String, String)> = ops
        .iter()
        .filter_map(|o| match o {
            Op::EmitEdge {
                edge_type,
                dst_type,
                ..
            } => Some((edge_type.to_string(), dst_type.to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(
        edges,
        vec![("placedBy".to_string(), "Person".to_string())],
        "placedBy → Person only (external is dangling)"
    );
}

// ── Schema-driven refinement (GraphSchema → edges + cardinality) ────────────

// An io.rdf mapping: the reference property `ex:hasProject` is written as a
// plain `FieldRef` (`.hasProject`), so the agnostic lowering CANNOT tell it from
// a literal column — only the graph schema knows it's an edge to `Project` (and
// multi-valued). This is the run_rdf.rs case at the MIR level.
const KB_FOSSIL: &str = "\
prefix ex: <https://ex.org/>

kb := io.rdf(\"graph.ttl\")

KB : ex:KB from kb
    iri = .subject
    ex:label = .label
    ex:hasProject = .hasProject
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
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, KB_FOSSIL.to_string(), "kb.fossil".to_string());
    let kb = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("KB is the first mapping");

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
