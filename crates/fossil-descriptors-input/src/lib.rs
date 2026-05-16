//! `fossil-descriptors-input` — input-side schema descriptors.
//!
//! An [`InputDescriptor`] tells the type-checker what fields a source has and
//! what their static types are (forward type propagation: source → mapping
//! body → triple terms).
//!
//! Phase 1 ships the trait + a [`CsvDescriptor`] stub returning a hardcoded
//! `{id: String, name: String}` schema (sufficient for the `hello.fossil`
//! walking-skeleton demo). Phase 3 CORE-05 replaces the stub with a real
//! CSVW thin parser (~500 LOC per RESEARCH.md §"Standard Stack").
//!
//! Phase 5 STDL-06 may add JSON Schema / XSD / Parquet implementations.
//!
//! ## Trait stability
//!
//! The Phase 1 trait surface (`name`, `parse`, `type_for_field`) is the
//! public-API commitment to Phase 3-9. Additive growth (e.g. `parse_async`
//! for streaming descriptors) is allowed; method removal requires an ADR.

/// Input-side schema descriptor.
///
/// Implementations parse a raw descriptor blob (CSVW JSON-LD, JSON Schema,
/// XSD, etc.) into an [`InputSchema`] used by the type-checker for forward
/// type propagation.
pub trait InputDescriptor: Send + Sync + std::fmt::Debug {
    /// Stable, lowercase, namespace-free identifier (e.g. `"csv"`, `"json"`).
    ///
    /// Trait signature returns `&str` (not `&'static str`) so Phase 3+
    /// implementations can return dynamically-computed names.
    fn name(&self) -> &str;

    /// Parse a raw descriptor blob into an [`InputSchema`].
    ///
    /// Phase 1 stubs ignore `raw` and return a hardcoded schema.
    fn parse(&self, raw: &[u8]) -> Result<InputSchema, DescriptorError>;

    /// Lookup the static type of a field by name.
    /// Returns `None` if the field is not declared in the schema.
    fn type_for_field(&self, schema: &InputSchema, field: &str) -> Option<FieldType>;
}

/// Parsed input schema: ordered map of field name → static type.
///
/// The ordering matters for column-position fallback parsing in CSV (Phase 3
/// CORE-05) and for stable diagnostic output.
#[derive(Debug, Clone)]
pub struct InputSchema {
    /// Ordered field declarations.
    pub fields: indexmap::IndexMap<smol_str::SmolStr, FieldType>,
}

/// Phase 1 minimal type lattice for input fields.
///
/// Phase 3 CORE-05 expands with `Float`, `Boolean`, `Date`, `DateTime`, etc.,
/// matching CSVW's xsd-derived datatype catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    String,
    Integer,
}

/// Error produced by descriptor parsing.
///
/// Phase 1 ships only `Invalid`; Phase 3 CORE-05 expands with parser-specific
/// variants (`MalformedJson`, `UnknownDatatype`, `MissingField`, etc.).
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("invalid descriptor: {0}")]
    Invalid(String),
}

/// Phase 1 stub: hardcoded CSV inference returning `{id: String, name: String}`.
///
/// Phase 3 CORE-05 replaces this with a real CSVW thin parser (~500 LOC).
/// `parse()` ignores its input — any byte slice yields the same hardcoded schema.
#[derive(Debug, Default)]
pub struct CsvDescriptor;

impl InputDescriptor for CsvDescriptor {
    // Phase 1 returns a literal; the trait signature stays `&str` for
    // Phase 3+ dynamic naming (see InputDescriptor::name() doc).
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "csv"
    }

    fn parse(&self, _raw: &[u8]) -> Result<InputSchema, DescriptorError> {
        let mut fields = indexmap::IndexMap::new();
        fields.insert("id".into(), FieldType::String);
        fields.insert("name".into(), FieldType::String);
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
    fn csv_descriptor_phase1_stub_returns_hardcoded_schema() {
        let d = CsvDescriptor;
        assert_eq!(d.name(), "csv");
        let schema = d.parse(b"unused").unwrap();
        assert_eq!(schema.fields.len(), 2);
        assert!(matches!(
            d.type_for_field(&schema, "id"),
            Some(FieldType::String)
        ));
        assert!(matches!(
            d.type_for_field(&schema, "name"),
            Some(FieldType::String)
        ));
        assert!(d.type_for_field(&schema, "missing").is_none());
    }

    #[test]
    fn csv_descriptor_ignores_raw_input() {
        // Phase 1 stub: any bytes → same hardcoded schema.
        let d = CsvDescriptor;
        let s1 = d.parse(b"").unwrap();
        let s2 = d.parse(b"completely different content").unwrap();
        assert_eq!(s1.fields.len(), s2.fields.len());
        assert_eq!(
            s1.fields.keys().collect::<Vec<_>>(),
            s2.fields.keys().collect::<Vec<_>>()
        );
    }
}
