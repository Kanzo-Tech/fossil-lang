//! Forward propagation via a host-registered `InferredDescriptor`, keyed by the
//! source **URI** (ADR-0037, rekeyed by ADR-0050).
//!
//! What only this crate can test is the indirection: the checker holds a
//! binding name, the cache holds URIs, and the `DefMap` is what joins them. The
//! table's own behaviour — insertion, replacement, freshness — belongs to
//! `fossil-descriptors-input::cache`, and the `System` accessor to
//! `fossil-base`; neither is re-asserted here.

use fossil_base::{Db, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::Primitive;
use fossil_hir::def_map::def_map;
use fossil_hir::infer::source_row_inferred;
use fossil_hir::ty::TyKind;
use std::sync::Arc;

const PROGRAM: &str = "prefix ex: <https://example.org/>\n\
                       users := io.csv(\"data/users.csv\")\n\
                       User : ex:Person from users\n    \
                       ex:name = .name\n";

fn descriptor(uri: &str, columns: &[(&str, Primitive)]) -> InferredDescriptor {
    InferredDescriptor {
        uri: uri.into(),
        columns: columns
            .iter()
            .map(|(n, p)| InferredColumn {
                name: (*n).into(),
                primitive: *p,
            })
            .collect(),
        freshness_token: "t1".into(),
    }
}

/// A db over a `NativeSystem` holding `descriptors`, plus the program file.
fn db_with(descriptors: Vec<InferredDescriptor>) -> (FossilDb, SourceFile) {
    let system = NativeSystem::default();
    let cache = system
        .descriptors()
        .expect("the native host keeps a descriptor table");
    for d in descriptors {
        cache.insert(d);
    }
    let db = FossilDb::new(Arc::new(system) as Arc<dyn System>);
    let file = SourceFile::new(&db, PROGRAM.to_string(), "/w/mapping.fossil".to_string());
    (db, file)
}

fn field_names(db: &FossilDb, file: SourceFile) -> Vec<String> {
    let mappings = def_map(db, file).mappings(db).clone();
    let mapping = *mappings.first().expect("one mapping");
    let Some(row) = source_row_inferred(db, mapping) else {
        return Vec::new();
    };
    let TyKind::Record(rec) = row.kind(db) else {
        panic!("a source row is a Record");
    };
    rec.fields(db).iter().map(|f| f.name.to_string()).collect()
}

/// The checker reaches the descriptor through the URI the binding names, which
/// is the one thing the binding name is good for here.
#[test]
fn a_descriptor_registered_under_the_uri_reaches_the_binding_that_names_it() {
    let (db, file) = db_with(vec![descriptor(
        "data/users.csv",
        &[("id", Primitive::Integer), ("name", Primitive::String)],
    )]);
    assert_eq!(field_names(&db, file), ["id", "name"]);
}

/// The binding name is not a key. A descriptor filed under `"users"` — what the
/// pre-ADR-0050 host registered — is not found, and the checker falls through
/// to no forward propagation instead of typing against the wrong file.
#[test]
fn a_descriptor_registered_under_the_binding_name_is_not_found() {
    let (db, file) = db_with(vec![descriptor("users", &[("id", Primitive::Integer)])]);
    assert!(field_names(&db, file).is_empty());
}

/// A URI nobody registered propagates nothing, and does not panic on the way.
#[test]
fn an_unregistered_uri_propagates_nothing() {
    let (db, file) = db_with(vec![descriptor(
        "data/other.csv",
        &[("id", Primitive::Integer)],
    )]);
    assert!(field_names(&db, file).is_empty());
}

/// Re-introspection is visible to the checker: the cache replaces the entry for
/// a URI, and the next type-check reads the new columns. This is the
/// consumer-side half of the engine's re-introspection test.
#[test]
fn re_registering_a_uri_changes_what_the_checker_sees() {
    let (db, file) = db_with(vec![descriptor(
        "data/users.csv",
        &[("id", Primitive::Integer)],
    )]);
    assert_eq!(field_names(&db, file), ["id"]);

    db.system().descriptors().expect("table").insert(descriptor(
        "data/users.csv",
        &[("id", Primitive::Integer), ("email", Primitive::String)],
    ));
    assert_eq!(field_names(&db, file), ["id", "email"]);
}
