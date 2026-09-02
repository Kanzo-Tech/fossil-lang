//! `InferredDescriptor` — input schema produced by host-side runtime introspection
//! (`DuckDB` `DESCRIBE read_csv_auto` in the playground; `duckdb::Connection` in
//! the native CLI). A host ships no schema sidecar: it introspects the source
//! and hands over the column list.
//!
//! Salsa-friendly: concrete struct (NOT trait object), Send + Sync + Clone +
//! Hash + Eq, serde-(de)serialisable. The host passes descriptors in through an
//! accessor on the ambient `fossil-base::System`, reached the same way
//! `read_file` is — never a query key, never interned — BEFORE invoking
//! `compile()`. Rust never does network IO from the WASM-gated crates.
//!
//! It implements `InputDescriptor`, and nothing dispatches through that trait:
//! `fossil-hir` reads the struct's `columns` directly (`lookup_inferred`), so
//! the impl below is the trait's only one that is not a stub.

use crate::{DescriptorError, FieldType, InputDescriptor, InputSchema};
use fossil_graph_schema::Primitive;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// One column from a host-introspected source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InferredColumn {
    /// Column name as introspected (`DuckDB` `column_name`).
    pub name: SmolStr,
    /// The column's place in the lattice — the same enum the checker types
    /// against, so a host sends `"integer"`, not a name the consumer has to
    /// look up. A value outside the lattice fails here, at deserialisation,
    /// naming itself; it does not become a `String` column and a diagnostic
    /// three crates away.
    pub primitive: Primitive,
}

/// A descriptor inferred from a runtime file introspection.
///
/// Identified by the source **URI**, which is what the descriptor is about: a
/// binding name is a name for the *program*'s convenience, and two of them can
/// point at one file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InferredDescriptor {
    /// The source URI exactly as written in the program — the string inside
    /// `io.csv("examples/users.csv")`. NOT the resolved locator: the host
    /// resolves (`@conn` aliases, a program-relative path, a signed URL) in
    /// order to *read* the source, but the checker only ever sees what the
    /// program says, so that is the only string both ends can agree on.
    pub uri: SmolStr,
    /// Ordered columns; the order is the source's own and is significant.
    pub columns: Vec<InferredColumn>,
    /// Opaque token identifying the state of the source this was read from.
    /// The cache compares it; nothing interprets it. The native host writes
    /// `mtime` + size, a host that has a strong `ETag` or a content digest
    /// writes that instead, and a host that cannot cheaply tell writes `""` —
    /// which [`crate::DescriptorCache::is_fresh`] reads as "never fresh", so
    /// that source is re-introspected every time. It is NOT a hash of the bytes,
    /// and deliberately: hashing means reading the whole source to decide
    /// whether the source needs reading, which makes the cache cost more than
    /// the `DESCRIBE` it saves. `mtime` + size is two fields of one `stat`. Its
    /// one dangerous failure — saying "unchanged" when it changed — needs a file
    /// restored with the same `mtime` AND the same size, which is what pairing
    /// the two narrows.
    pub freshness_token: String,
}

impl InferredDescriptor {
    /// Build an empty descriptor for `uri` — useful for tests.
    #[must_use]
    pub fn empty(uri: impl Into<SmolStr>) -> Self {
        Self {
            uri: uri.into(),
            columns: Vec::new(),
            freshness_token: String::new(),
        }
    }
}

impl InputDescriptor for InferredDescriptor {
    // `&str` and not `&'static str`: the trait's signature, so an impl may
    // compute its name.
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "inferred"
    }

    // There is no raw blob to parse — the host built the columns from a
    // `DESCRIBE` — so this projects `self.columns` into an `InputSchema`.
    fn parse(&self, _raw: &[u8]) -> Result<InputSchema, DescriptorError> {
        let mut fields = indexmap::IndexMap::new();
        for c in &self.columns {
            let ft = match c.primitive {
                Primitive::Integer => FieldType::Integer,
                _ => FieldType::String,
            };
            fields.insert(c.name.clone(), ft);
        }
        Ok(InputSchema { fields })
    }

    fn type_for_field(&self, schema: &InputSchema, field: &str) -> Option<FieldType> {
        schema.fields.get(field).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferred_descriptor_round_trips_through_serde() {
        let d = InferredDescriptor {
            uri: "examples/users.csv".into(),
            columns: vec![
                InferredColumn {
                    name: "id".into(),
                    primitive: Primitive::Integer,
                },
                InferredColumn {
                    name: "name".into(),
                    primitive: Primitive::String,
                },
            ],
            freshness_token: "abc123".into(),
        };
        let j = serde_json::to_string(&d).expect("serialise");
        let d2: InferredDescriptor = serde_json::from_str(&j).expect("deserialise");
        assert_eq!(d, d2);
    }

    #[test]
    fn parse_builds_input_schema_from_columns() {
        let d = InferredDescriptor {
            uri: "examples/users.csv".into(),
            columns: vec![
                InferredColumn {
                    name: "id".into(),
                    primitive: Primitive::Integer,
                },
                InferredColumn {
                    name: "name".into(),
                    primitive: Primitive::String,
                },
            ],
            freshness_token: String::new(),
        };
        let schema = d.parse(b"unused").unwrap();
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.fields.get("id").copied(), Some(FieldType::Integer));
        assert_eq!(schema.fields.get("name").copied(), Some(FieldType::String));
    }

    /// The host's wire format is the lattice itself. A name outside it is
    /// rejected here — where the JSON arrives and the offending value is still
    /// in hand — instead of being coerced to `String` and reported by the
    /// checker as a diagnostic about a column.
    #[test]
    fn a_primitive_outside_the_lattice_is_a_deserialisation_error() {
        let json = r#"{"uri":"examples/users.csv",
                       "columns":[{"name":"id","primitive":"decimal"}],
                       "freshness_token":""}"#;
        let err = serde_json::from_str::<InferredDescriptor>(json)
            .expect_err("`decimal` is not in the lattice");
        assert!(
            err.to_string().contains("decimal"),
            "the error names the value the host sent: {err}"
        );
    }

    #[test]
    fn name_is_inferred() {
        let d = InferredDescriptor::empty("examples/users.csv");
        assert_eq!(d.name(), "inferred");
    }
}
