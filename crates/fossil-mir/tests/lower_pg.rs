//! E2E del lowering property-graph (paso 2, vertex-only): parse `hello.fossil`
//! → [`lower_to_mir_pg`] → `Source → EmitVertex(Person) → Sink`.
//!
//! Branch-by-abstraction: el path legacy [`fossil_mir::lower_to_mir`] (`TripleEmit`)
//! sigue intacto; este test fija el nuevo path PG-canónico para el caso de un
//! vértice con propiedades literales (sin edges — esos llegan en el próximo
//! incremento, con la clasificación descriptor-driven que ya tiene el codegen).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_hir::def_map::def_map;
use fossil_mir::{apply_output_shape, lower_to_mir_pg, Op};

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
    assert_eq!(type_name, "Person", "vertex type = subject shape local name");
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
    assert_eq!(prop_names, ["total"], "only the literal property is a vertex prop");

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

// ── Descriptor-driven refinement (ShEx → edges + cardinality) ───────────────

// An io.rdf mapping: the shape-ref property `ex:hasProject` is written as a
// plain `FieldRef` (`.hasProject`), so the agnostic lowering CANNOT tell it from
// a literal column — only the ShEx descriptor knows it's an edge to `Project`
// (and multi-valued). This is the run_rdf.rs case at the MIR level.
const KB_FOSSIL: &str = "\
prefix ex: <https://ex.org/>

kb := io.rdf(\"graph.ttl\")

KB : ex:KB from kb
    iri = .subject
    ex:label = .label
    ex:hasProject = .hasProject
";

// `KB`: a literal `label` + a multi-valued (`max:-1`) shape-ref `hasProject` →
// `Project` (an edge); `Project`: a literal `title`.
const KB_SHEX: &str = r#"{ "@context": "http://www.w3.org/ns/shex.jsonld", "type": "Schema", "shapes": [
  {"type":"ShapeDecl","id":"https://ex.org/KB","shapeExpr":{"type":"Shape","expression":{"type":"EachOf","expressions":[
     {"type":"TripleConstraint","predicate":"https://ex.org/label","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}},
     {"type":"TripleConstraint","predicate":"https://ex.org/hasProject","valueExpr":"https://ex.org/Project","min":0,"max":-1}
  ]}}},
  {"type":"ShapeDecl","id":"https://ex.org/Project","shapeExpr":{"type":"Shape","expression":{
     "type":"TripleConstraint","predicate":"https://ex.org/title","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}}}}
] }"#;

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

    // The ShEx descriptor reclassifies `hasProject` into a typed, multi-valued edge.
    let desc = OutputDescriptorKind::ShEx(
        ShExDescriptor::from_reader(KB_SHEX.as_bytes()).expect("parse ShEx"),
    );
    let refined = apply_output_shape(ops, &desc);

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
        props.iter().find(|p| p.name == "label").unwrap().single_valued,
        "label is Exact(1) → single-valued"
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
