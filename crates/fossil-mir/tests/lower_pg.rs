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
