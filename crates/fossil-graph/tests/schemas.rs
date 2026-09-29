//! Snapshot the JSON Schemas for every verb's params + result.
//!
//! These snapshots ARE the wire contract every transport binding consumes —
//! the surface is published once here and no binding carries a second copy of
//! it. A hand-edit to a `Params`/`Result` struct that the
//! author didn't intend to publish surfaces as a snapshot diff and fails CI
//! until reviewed (`cargo insta review` to accept).
//!
//! Nothing reads the `.snap` files at run time. What the binding consumes is
//! the same `schemars` derive: `@fossil-lang/corpus` through
//! `examples/dump_schemas.rs` and its `scripts/gen-types.sh`. These snapshots are the review surface over that
//! derive, not a second artefact.

use fossil_graph::operations::{Operation, RawSqlAccess, Verb, schema, sql};
use schemars::schema_for;
use serde_json::json;

macro_rules! snap {
    ($name:literal, $ty:ty) => {
        insta::assert_yaml_snapshot!($name, schema_for!($ty));
    };
}

#[test]
fn dispatch_envelope() {
    // The outer Operation enum — its tagged shape IS the wire envelope
    // (`{ "verb": "...", "params": {...} }`).
    snap!("operation_envelope", Operation);
}

#[test]
fn schema_verb() {
    snap!("schema_params", schema::SchemaParams);
    snap!("schema_result", schema::SchemaResult);
}

#[test]
fn sql_verb() {
    snap!("execute_sql_params", sql::ExecuteSqlParams);
    snap!("execute_sql_result", sql::ExecuteSqlResult);
}

#[test]
fn operation_round_trip() {
    // Sanity: the wire envelope round-trips through JSON. It deserialises
    // through `from_wire` rather than `serde_json::from_str` because
    // `Operation` has no `Deserialize` impl — see `operations::raw_sql`.
    let op = Operation::Schema(schema::SchemaParams::default());
    let text = serde_json::to_string(&op).expect("serialize");
    assert!(
        text.contains(r#""verb":"schema""#),
        "envelope tag should match snake_case verb name, got: {text}",
    );
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    let back = Operation::from_wire(&value, None).expect("deserialize");
    assert_eq!(back.verb_name(), "schema");
}

/// **The permission is one argument.** `execute_sql`'s `sql` is `RawSql`,
/// which has no `Deserialize`; the only way in is [`Operation::from_wire`],
/// and its `sql` argument decides whether the hatch parses at all.
#[test]
fn the_permission_opens_and_closes_the_hatch() {
    let hatch = json!({ "verb": "execute_sql", "params": { "sql": "SELECT 1" } });
    assert!(matches!(
        Operation::from_wire(&hatch, None).expect_err("the hatch is raw SQL"),
        fossil_graph::GraphError::RawSqlWithheld {
            field: "execute_sql.sql"
        }
    ));
    assert!(Operation::from_wire(&hatch, Some(RawSqlAccess::granted())).is_ok());
}

/// The verb set a wire tag parses against is the set the envelope enumerates.
#[test]
fn the_catalogue_is_the_envelope() {
    let envelope = serde_json::to_value(schema_for!(Operation)).expect("envelope schema");
    let tags: Vec<String> = envelope["oneOf"]
        .as_array()
        .expect("the envelope is a oneOf over its variants")
        .iter()
        .map(|variant| {
            variant["properties"]["verb"]["const"]
                .as_str()
                .or_else(|| variant["properties"]["verb"]["enum"][0].as_str())
                .expect("each variant pins its tag")
                .to_string()
        })
        .collect();
    let catalogue: Vec<String> = Verb::ALL.iter().map(|v| v.name().to_string()).collect();
    assert_eq!(tags, catalogue, "the verb set and the wire envelope differ");
    for verb in Verb::ALL {
        assert_eq!(Verb::from_name(verb.name()), Some(verb));
    }
    assert!(Verb::from_name("read").is_none());
}

/// **The wire mirror cannot drift from the struct it mirrors.**
///
/// `WireExecuteSqlParams` exists because `sql` arrives as a string and leaves
/// as `RawSql`. Being a hand-written copy, it can fall behind a field added to
/// the real struct — and the failure would be silent: the field would simply
/// stop being reachable from the wire. So the two schemas are compared as
/// documents. `RawSql` is `#[schemars(transparent)]` precisely so that this
/// comparison is possible at all.
#[test]
fn the_wire_mirror_describes_the_same_document() {
    fn normalise(mut v: serde_json::Value) -> serde_json::Value {
        // The two things that are MEANT to differ, and only those: the title
        // is the Rust type name, and the struct-level doc comment argues a
        // different point. Everything below the root — field names, types,
        // defaults, the required set and the per-field prose — must agree.
        let root = v.as_object_mut().expect("a schema object");
        root.remove("title");
        root.remove("description");
        v
    }
    assert_eq!(
        normalise(serde_json::to_value(schema_for!(sql::WireExecuteSqlParams)).expect("wire")),
        normalise(serde_json::to_value(schema_for!(sql::ExecuteSqlParams)).expect("sql")),
        "execute_sql's wire mirror and its params struct describe different documents",
    );
}
