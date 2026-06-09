//! E2E del lowering property-graph (paso 2, vertex-only): parse `hello.fossil`
//! → [`lower_to_mir_pg`] → `Source → EmitVertex(Person) → Sink`.
//!
//! Branch-by-abstraction: el path legacy [`fossil_mir::lower_to_mir`] (TripleEmit)
//! sigue intacto; este test fija el nuevo path PG-canónico para el caso de un
//! vértice con propiedades literales (sin edges — esos llegan en el próximo
//! incremento, con la clasificación descriptor-driven que ya tiene el codegen).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_hir::def_map::def_map;
use fossil_mir::{lower_to_mir_pg, Op};

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
