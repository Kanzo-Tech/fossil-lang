//! The JSON Schema of the LSP types a wire row carries.
//!
//! `lsp-types` derives none, so these state what its serde writes, and a row's
//! field names one with `#[schemars(with = …)]` or `schema_with`.

use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::{InstanceType, Schema, SchemaObject};

/// Zero-based line and UTF-16 column — LSP's `Position`.
#[derive(Debug, JsonSchema)]
#[schemars(rename = "Position")]
pub struct PositionSchema {
    pub line: u32,
    pub character: u32,
}

/// Inclusive `start`, exclusive `end` — LSP's `Range`.
#[derive(Debug, JsonSchema)]
#[schemars(rename = "Range")]
pub struct RangeSchema {
    pub start: PositionSchema,
    pub end: PositionSchema,
}

/// LSP's `DiagnosticSeverity`: 1 error, 2 warning, 3 information, 4 hint.
#[must_use]
pub fn severity(_: &mut SchemaGenerator) -> Schema {
    SchemaObject {
        instance_type: Some(InstanceType::Integer.into()),
        enum_values: Some((1..=4).map(Into::into).collect()),
        ..SchemaObject::default()
    }
    .into()
}
