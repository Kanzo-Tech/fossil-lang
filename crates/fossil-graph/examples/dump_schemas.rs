//! Emit one combined JSON Schema document for the whole verb surface to stdout.
//!
//! This is the codegen source for the TS binding (`@fossil-lang/corpus`): its
//! `scripts/gen-types.sh` pipes this output through `json-schema-to-typescript`
//! to produce `src/generated.ts`. The same schemas are snapshot-tested in
//! `tests/schemas.rs` (the canonical wire contract) — this example
//! just re-shapes them into a single `{ definitions, properties }` doc that
//! `json2ts` can compile in one pass.
//!
//! No hand-written TS types: the Rust structs (which derive `schemars::JsonSchema`)
//! are the single source of truth. Run from the repo root:
//!
//! ```sh
//! cargo run -p fossil-graph --example dump_schemas
//! ```

use fossil_graph::operations::{Operation, aggregate, discovery, schema, sql};
use schemars::r#gen::SchemaGenerator;
use serde_json::{Map, Value, json};

fn main() {
    let mut generator = SchemaGenerator::default();

    // Register every public type. Params are reachable transitively through
    // `Operation`, but registering them explicitly keeps the property map below
    // exhaustive and self-documenting.
    generator.subschema_for::<Operation>();

    generator.subschema_for::<schema::SchemaResult>();
    generator.subschema_for::<discovery::ReadResult>();
    generator.subschema_for::<discovery::ExpandResult>();
    generator.subschema_for::<discovery::PathResult>();
    generator.subschema_for::<aggregate::AggregateResult>();
    generator.subschema_for::<sql::ExecuteSqlResult>();

    // The schemars 0.8 default puts named schemas under `#/definitions/<Ident>`.
    // Build a root object whose properties reference each registered definition
    // so `json2ts` emits an interface for every one (it declares all reachable
    // `$ref` targets).
    let definitions: Map<String, Value> = generator
        .definitions()
        .iter()
        .map(|(name, schema)| (name.clone(), serde_json::to_value(schema).unwrap()))
        .collect();

    let mut properties = Map::new();
    for name in definitions.keys() {
        properties.insert(
            name.clone(),
            json!({ "$ref": format!("#/definitions/{name}") }),
        );
    }

    let doc = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "FossilGraphSchemas",
        "type": "object",
        "properties": properties,
        "definitions": definitions,
    });

    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
