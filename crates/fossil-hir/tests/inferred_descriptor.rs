//! Forward propagation via a host-registered `InferredDescriptor`, keyed by the
//! source **URI**, never the binding name.
//!
//! What only this crate can test is the indirection: the checker holds a
//! binding name, the source registry holds locations, and the `DefMap` is what
//! joins them — and that the registry is an input, so describing a source
//! re-runs a check that already ran (`/docs/design/incremental`).

use fossil_base::sources::{InferredColumn, InferredDescriptor};
use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System, describe_columns};
use fossil_graph_schema::Primitive;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;
use fossil_hir::infer::source_row_inferred;
use fossil_hir::ty::TyKind;
use std::sync::Arc;

const PROGRAM: &str = "users := io.csv(\"data/users.csv\")\n\
                       User : Person from users\n    \
                       name = users.name\n";

fn descriptor(uri: &str, columns: &[(&str, Primitive)]) -> InferredDescriptor {
    InferredDescriptor {
        key: uri.into(),
        columns: columns
            .iter()
            .map(|(n, p)| InferredColumn {
                name: (*n).into(),
                primitive: *p,
            })
            .collect(),
        etag: "t1".into(),
    }
}

/// A db over a `NativeSystem` holding `descriptors`, plus the program file.
fn db_with(descriptors: Vec<InferredDescriptor>) -> (FossilDb, SourceFile) {
    let mut db = FossilDb::new(Arc::new(NativeSystem::default()) as Arc<dyn System>);
    for d in descriptors {
        describe_columns(&mut db, d);
    }
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

/// The binding name is not a key. A descriptor filed under `"users"` — what a
/// host registered before the cache was keyed by URI — is not found, and the
/// checker falls through
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

/// Re-describing is visible to the checker: the registry replaces the entry for
/// a URI, and the next type-check reads the new columns. This is the
/// consumer-side half of the engine's re-introspection test.
#[test]
fn re_registering_a_uri_changes_what_the_checker_sees() {
    let (mut db, file) = db_with(vec![descriptor(
        "data/users.csv",
        &[("id", Primitive::Integer)],
    )]);
    assert_eq!(field_names(&db, file), ["id"]);

    describe_columns(
        &mut db,
        descriptor(
            "data/users.csv",
            &[("id", Primitive::Integer), ("email", Primitive::String)],
        ),
    );
    assert_eq!(field_names(&db, file), ["id", "email"]);
}

/// What the registry being an input buys: a check that already ran against a
/// source's earlier columns re-runs when the source is described again. Read
/// beside Salsa, the memoised check would keep refusing a column the source
/// now has.
#[test]
fn describing_a_source_after_a_check_re_runs_the_check() {
    let (mut db, file) = db_with(vec![descriptor(
        "data/users.csv",
        &[("id", Primitive::Integer)],
    )]);
    let unknown_name = |db: &FossilDb| {
        let mapping = *def_map(db, file).mappings(db).first().expect("one mapping");
        typecheck_mapping(db, mapping)
            .diagnostics(db)
            .iter()
            .any(|d| d.problem.code() == "name/unknown-field")
    };
    assert!(unknown_name(&db), "`users.name` is not a column yet");

    describe_columns(
        &mut db,
        descriptor(
            "data/users.csv",
            &[("id", Primitive::Integer), ("name", Primitive::String)],
        ),
    );
    assert!(
        !unknown_name(&db),
        "the check re-ran against the new columns"
    );
}
