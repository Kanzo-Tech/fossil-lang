//! `InferredDescriptor` — input schema produced by host-side runtime introspection
//! (`DuckDB` `DESCRIBE read_csv_auto` in the playground; `duckdb::Connection` in
//! the native CLI). See ADR-0037.
//!
//! Salsa-friendly: concrete struct (NOT trait object), Send + Sync + Clone +
//! Hash + Eq, serde-(de)serialisable. The host passes descriptors in via the
//! `fossil-base::System` accessor (mirrors `read_file` pattern, ADR-0020) BEFORE
//! invoking `compile()`. Rust never does network IO from the WASM-gated crates.
//!
//! Implements `InputDescriptor` so existing forward-propagation code paths in
//! `fossil-hir` can dispatch by `&dyn InputDescriptor` without specialisation
//! (passes a hot-path `&dyn`, NOT a `Box<dyn>` — CLAUDE.md hard rule).

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
/// `content_hash` is provided by the host (browser-side DuckDB-WASM; native
/// CLI). If absent (`""`), the consumer falls back to hashing the canonical
/// `(name, primitive)` column tuple list deterministically. The hash drives
/// Salsa invalidation: when the host re-registers a descriptor with the same
/// hash, downstream queries do not re-run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InferredDescriptor {
    /// Source binding name this descriptor is for (`users` in
    /// `users := io.csv("...")`).
    pub source_name: SmolStr,
    /// Ordered columns (order is significant — Phase 3 CORE-05 column-position
    /// fallback parsing depends on it).
    pub columns: Vec<InferredColumn>,
    /// Opaque content-hash provided by the host; empty means "consumer should
    /// derive". UTF-8 hex string; max 64 chars.
    pub content_hash: String,
}

impl InferredDescriptor {
    /// Build an empty descriptor for `source_name` — useful for tests.
    #[must_use]
    pub fn empty(source_name: impl Into<SmolStr>) -> Self {
        Self {
            source_name: source_name.into(),
            columns: Vec::new(),
            content_hash: String::new(),
        }
    }

    /// Compute a deterministic content-hash from the column list if the host
    /// did not supply one. Uses std `DefaultHasher` for portability (no new
    /// dep on blake3 / sha2 — WASM gate friendly). Returned as a 16-char hex
    /// string (sufficient for Salsa keying within a single workspace).
    #[must_use]
    pub fn derived_content_hash(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        self.source_name.hash(&mut h);
        for c in &self.columns {
            c.name.hash(&mut h);
            c.primitive.hash(&mut h);
        }
        format!("{:016x}", h.finish())
    }
}

impl InputDescriptor for InferredDescriptor {
    // Trait signature returns `&str` (not `&'static str`) for Phase 3+ dynamic
    // naming compatibility — see `InputDescriptor::name` doc and the mirror
    // allow on `CsvDescriptor::name` in `lib.rs`.
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "inferred"
    }

    // `parse()` is a no-op for InferredDescriptor — there is no raw blob to
    // parse; the host constructed the columns directly via DuckDB DESCRIBE.
    // We satisfy the trait by returning a freshly-built InputSchema from
    // self.columns. Plan 13-02 wires the actual lower.rs/check.rs path
    // around this trait method.
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
            source_name: "users".into(),
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
            content_hash: "abc123".into(),
        };
        let j = serde_json::to_string(&d).expect("serialise");
        let d2: InferredDescriptor = serde_json::from_str(&j).expect("deserialise");
        assert_eq!(d, d2);
    }

    #[test]
    fn derived_content_hash_is_deterministic() {
        let d1 = InferredDescriptor {
            source_name: "users".into(),
            columns: vec![InferredColumn {
                name: "id".into(),
                primitive: Primitive::Integer,
            }],
            content_hash: String::new(),
        };
        let d2 = d1.clone();
        assert_eq!(d1.derived_content_hash(), d2.derived_content_hash());
        assert_eq!(d1.derived_content_hash().len(), 16);
    }

    #[test]
    fn parse_builds_input_schema_from_columns() {
        let d = InferredDescriptor {
            source_name: "users".into(),
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
            content_hash: String::new(),
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
        let json = r#"{"source_name":"users",
                       "columns":[{"name":"id","primitive":"decimal"}],
                       "content_hash":""}"#;
        let err = serde_json::from_str::<InferredDescriptor>(json)
            .expect_err("`decimal` is not in the lattice");
        assert!(
            err.to_string().contains("decimal"),
            "the error names the value the host sent: {err}"
        );
    }

    #[test]
    fn name_is_inferred() {
        let d = InferredDescriptor::empty("users");
        assert_eq!(d.name(), "inferred");
    }
}
