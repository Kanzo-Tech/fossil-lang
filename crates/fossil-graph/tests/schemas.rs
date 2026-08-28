//! Snapshot the JSON Schemas for every verb's params + result.
//!
//! These snapshots ARE the wire contract every transport binding consumes —
//! the surface is published once here and no binding carries a second copy of
//! it. A hand-edit to a `Params`/`Result` struct that the
//! author didn't intend to publish surfaces as a snapshot diff and fails CI
//! until reviewed (`cargo insta review` to accept).
//!
//! Downstream consumers (fossil-mcp, fossil-cli, @fossil-lang/corpus
//! codegen) treat the accepted `.snap` files as the source of truth. The
//! TS bindings, in particular, regenerate their wrapper types from these
//! schemas via openapi-typescript at the playground/keasy `pnpm openapi`
//! step.

use fossil_graph::operations::{Operation, RawSqlAccess, Verb, aggregate, discovery, schema, sql};
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
fn read_verbs() {
    snap!("read_params", discovery::ReadParams);
    snap!("read_result", discovery::ReadResult);
    snap!("expand_params", discovery::ExpandParams);
    snap!("expand_result", discovery::ExpandResult);
    snap!("path_params", discovery::PathParams);
    snap!("path_result", discovery::PathResult);
}

#[test]
fn aggregate_verb() {
    snap!("aggregate_params", aggregate::AggregateParams);
    snap!("aggregate_result", aggregate::AggregateResult);
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

/// **The permission is one argument, and it lands on both doors.**
///
/// `read`'s `where` and `execute_sql`'s `sql` are the same authority over the
/// same engine. That was a comment on `ReadParams` until the two fields became
/// `RawSql`, which has no `Deserialize`; the only way in is
/// [`Operation::from_wire`], whose `sql` argument this exercises in all four
/// combinations. There is no fifth: a binding cannot spell "hatch closed,
/// `where` open", which is what the comment was asking for and could not get.
#[test]
fn one_permission_opens_both_raw_sql_doors_and_closes_both() {
    let read_plain = json!({ "verb": "read", "params": { "vertex_type": "Person" } });
    let read_where =
        json!({ "verb": "read", "params": { "vertex_type": "Person", "where": "age > 30" } });
    let hatch = json!({ "verb": "execute_sql", "params": { "sql": "SELECT 1" } });

    // Withheld: the predicate-free read still works, the other two do not.
    assert!(Operation::from_wire(&read_plain, None).is_ok());
    let refused = Operation::from_wire(&read_where, None).expect_err("where is raw SQL");
    assert!(
        matches!(
            refused,
            fossil_graph::GraphError::RawSqlWithheld {
                field: "read.where"
            }
        ),
        "got {refused:?}",
    );
    assert!(matches!(
        Operation::from_wire(&hatch, None).expect_err("the hatch is raw SQL"),
        fossil_graph::GraphError::RawSqlWithheld {
            field: "execute_sql.sql"
        }
    ));

    // Granted: both.
    let access = Some(RawSqlAccess::granted());
    assert!(Operation::from_wire(&read_where, access).is_ok());
    assert!(Operation::from_wire(&hatch, access).is_ok());
}

/// The verb catalogue a binding lists its tools from is the same six the
/// envelope enumerates, and `reaches_raw_sql` names exactly the two the
/// permission covers.
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
    assert_eq!(
        tags, catalogue,
        "the tool list and the wire envelope differ"
    );

    let raw: Vec<&str> = Verb::ALL
        .into_iter()
        .filter(|v| v.reaches_raw_sql())
        .map(Verb::name)
        .collect();
    assert_eq!(raw, ["read", "execute_sql"]);

    // Every verb answers every question in the catalogue.
    for verb in Verb::ALL {
        assert_eq!(Verb::from_name(verb.name()), Some(verb));
        assert!(
            !verb.description().is_empty(),
            "{} has no prose",
            verb.name()
        );
        assert_eq!(
            verb.params_schema()["type"],
            "object",
            "{}'s params schema is not an object",
            verb.name()
        );
    }
    assert!(Verb::from_name("materialize_graph").is_none());
}

/// **The wire mirrors cannot drift from the structs they mirror.**
///
/// `WireReadParams` and `WireExecuteSqlParams` exist because `where` and `sql`
/// arrive as strings and leave as `RawSql`. Being hand-written copies, they can
/// fall behind a field added to the real struct — and the failure would be
/// silent: the field would simply stop being reachable from the wire. So the
/// two schemas are compared as documents. `RawSql` is `#[schemars(transparent)]`
/// precisely so that this comparison is possible at all.
#[test]
fn the_wire_mirrors_describe_the_same_document() {
    fn normalise(mut v: serde_json::Value) -> serde_json::Value {
        // The two things that are MEANT to differ, and only those: the title
        // is the Rust type name, and the struct-level doc comment argues two
        // different points. Everything below the root — field names, types,
        // defaults, the required set and the per-field prose — must agree.
        let root = v.as_object_mut().expect("a schema object");
        root.remove("title");
        root.remove("description");
        v
    }
    for (real, mirror, name) in [
        (
            Verb::Read.params_schema(),
            serde_json::to_value(schema_for!(discovery::ReadParams)).expect("read schema"),
            "read",
        ),
        (
            Verb::ExecuteSql.params_schema(),
            serde_json::to_value(schema_for!(sql::ExecuteSqlParams)).expect("sql schema"),
            "execute_sql",
        ),
    ] {
        assert_eq!(
            normalise(real),
            normalise(mirror),
            "{name}'s wire mirror and its params struct describe different documents",
        );
    }
}
