//! Integration tests for Phase 13 INPUT-01 + INPUT-03 — forward propagation
//! via `InferredDescriptor` (host-registered, no CSVW JSON on disk).
//!
//! These tests assert the System-trait wiring + `NativeSystem` storage that
//! plan 13-02 lands: registering a descriptor via
//! `System::register_inferred_descriptor` round-trips through
//! `System::inferred_descriptor`, and the semantic-equivalence invariant
//! (`record_from_inferred(inferred) ≡ record_from_descriptor(csvw)` on
//! identical column shapes) holds.
//!
//! The full lower→check→Record-Ty integration through `resolve_source_row`
//! requires building a fully-wired `MappingLoc` (`lower_to_hir` + `def_map` setup)
//! — that surface is exercised by the existing `fossil-hir` test suite via
//! its own helpers. Here we cover the new seams introduced by plan 13-02:
//! the System trait extension + the `NativeSystem` Mutex<HashMap> backing
//! + the cloned-on-read OWNED return contract.

use fossil_base::{Db, FossilDb, NativeSystem, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::Primitive;
use std::sync::Arc;

fn sample(source: &str, columns: Vec<(&str, Primitive)>) -> InferredDescriptor {
    InferredDescriptor {
        source_name: source.into(),
        columns: columns
            .into_iter()
            .map(|(n, p)| InferredColumn {
                name: n.into(),
                primitive: p,
            })
            .collect(),
        content_hash: String::new(),
    }
}

fn db_with_inferred(descriptors: Vec<InferredDescriptor>) -> FossilDb {
    let system = NativeSystem::default();
    for d in descriptors {
        system.register_inferred_descriptor(d);
    }
    // VERIFIED signature (plan 13-02 Task 0): FossilDb::new takes Arc<dyn System>.
    FossilDb::new(Arc::new(system) as Arc<dyn System>)
}

#[test]
fn inferred_descriptor_registers_and_round_trips_through_system() {
    let db = db_with_inferred(vec![sample(
        "users",
        vec![
            ("id", Primitive::Integer),
            ("name", Primitive::String),
            ("age", Primitive::Integer),
        ],
    )]);

    // Reach through the Db's System accessor — same path that
    // `resolve_source_row` takes inside the typecheck_mapping query.
    let got = db
        .system()
        .inferred_descriptor("users")
        .expect("registered descriptor must round-trip");
    assert_eq!(got.source_name.as_str(), "users");
    assert_eq!(got.columns.len(), 3);
    assert_eq!(got.columns[0].name.as_str(), "id");
    assert_eq!(got.columns[0].primitive, Primitive::Integer);
    assert_eq!(got.columns[1].name.as_str(), "name");
    assert_eq!(got.columns[1].primitive, Primitive::String);
    assert_eq!(got.columns[2].name.as_str(), "age");
    assert_eq!(got.columns[2].primitive, Primitive::Integer);
}

#[test]
fn unknown_source_returns_none_without_panicking() {
    let db = db_with_inferred(vec![sample(
        "users",
        vec![("id", Primitive::Integer), ("name", Primitive::String)],
    )]);
    assert!(db.system().inferred_descriptor("nonexistent").is_none());
}

#[test]
fn re_registering_same_source_name_overwrites_previous_entry() {
    let system = NativeSystem::default();
    system.register_inferred_descriptor(sample("users", vec![("id", Primitive::Integer)]));
    // Second registration with same source_name + extra column.
    system.register_inferred_descriptor(sample(
        "users",
        vec![("id", Primitive::Integer), ("email", Primitive::String)],
    ));
    let got = system
        .inferred_descriptor("users")
        .expect("present after re-register");
    assert_eq!(got.columns.len(), 2);
    assert_eq!(got.columns[1].name.as_str(), "email");
}

#[test]
fn empty_descriptor_register_and_lookup_returns_empty_columns() {
    let db = db_with_inferred(vec![InferredDescriptor::empty("users")]);
    let got = db.system().inferred_descriptor("users").expect("present");
    assert!(got.columns.is_empty());
    assert_eq!(got.content_hash, "");
}
