//! `fossil-descriptors-input` — input-side schema descriptors.
//!
//! An [`InputDescriptor`] tells the type-checker what fields a source has and
//! what their static types are (forward type propagation: source → mapping
//! body → triple terms).
//!
//! The crate ships the trait plus a [`CsvDescriptor`] stub returning a
//! hardcoded `{id: String, name: String}` schema. That stub exists for the
//! `hello.fossil` walking-skeleton demo and for nothing else.
//!
//! JSON Schema / XSD / Parquet implementations would attach at the same trait.
//!
//! ## v0.2: drop user-facing CSVW; introduce `InferredDescriptor`
//!
//! A host does not ship a schema sidecar. It runs `DuckDB` `DESCRIBE
//! read_csv_auto(...)` (browser-side `DuckDB-WASM`, or the native `duckdb`
//! crate in `fossil-cli`) and passes the introspected column list as an
//! [`InferredDescriptor`] (see [`inferred`]) ahead of `compile()`.
//!
//! # `CsvwDescriptor` was here, and it is gone
//!
//! A CSVW JSON-LD sidecar named by `schema = "<path>"`, kept as
//! "deprecated-but-functional internal IR" behind a `D-CSVW-DEPRECATED`
//! diagnostic whose own text read «types will be inferred from the file
//! directly. Remove the `schema = "..."` argument». The deprecation was right
//! and it has been carried out: the sidecar has no reader, no error variants
//! and no module.
//!
//! What settled it was the provider registry. `schema =` names a provider now
//! (`schema = io.shex("…")`), so CSVW would have needed a ROW — and adding one
//! is resurrecting a deprecated feature so the new model can express it. A
//! model that leaves a deprecated form nowhere to sit is agreeing with the
//! deprecation, not exposing a hole in itself.
//!
//! ## Trait stability
//!
//! The trait surface (`name`, `parse`, `type_for_field`) is the public-API
//! commitment. Additive growth (e.g. `parse_async` for streaming descriptors)
//! is allowed; removing a method is a decision, and gets written down on the
//! reference page that states the rule before it lands.

pub mod cache;
pub mod inferred;
pub mod shex;

pub use cache::DescriptorCache;
pub use inferred::{InferredColumn, InferredDescriptor};
pub use shex::{ShExInputError, inferred_descriptor_from_shex};

/// Input-side schema descriptor.
///
/// Implementations parse a raw descriptor blob (CSVW JSON-LD, JSON Schema,
/// XSD, etc.) into an [`InputSchema`] used by the type-checker for forward
/// type propagation.
pub trait InputDescriptor: Send + Sync + std::fmt::Debug {
    /// Stable, lowercase, namespace-free identifier (e.g. `"csv"`, `"json"`).
    ///
    /// Trait signature returns `&str` (not `&'static str`) so
    /// implementations can return dynamically-computed names.
    fn name(&self) -> &str;

    /// Parse a raw descriptor blob into an [`InputSchema`].
    ///
    /// The [`CsvDescriptor`] stub ignores `raw` and returns a hardcoded schema.
    fn parse(&self, raw: &[u8]) -> Result<InputSchema, DescriptorError>;

    /// Lookup the static type of a field by name.
    /// Returns `None` if the field is not declared in the schema.
    fn type_for_field(&self, schema: &InputSchema, field: &str) -> Option<FieldType>;
}

/// Parsed input schema: ordered map of field name → static type.
///
/// The ordering matters for column-position fallback parsing in CSV and for
/// stable diagnostic output.
#[derive(Debug, Clone)]
pub struct InputSchema {
    /// Ordered field declarations.
    pub fields: indexmap::IndexMap<smol_str::SmolStr, FieldType>,
}

/// The minimal type lattice for input fields.
///
/// It widens to `Float`, `Boolean`, `Date`, `DateTime` and the rest of the
/// xsd-derived datatype catalog when a source needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    String,
    Integer,
}

/// Error produced by descriptor parsing.
///
/// `Invalid` is the original catch-all; the structured variants below are what
/// new code should return. XSD / JSON Schema descriptors would extend it
/// further.
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    /// Catch-all variant retained for the
    /// [`CsvDescriptor`] stub and any external callers that pattern-matched
    /// on it. New code should prefer the structured variants below.
    #[error("invalid descriptor: {0}")]
    Invalid(String),

    /// The descriptor source is not valid JSON, or does not match the
    /// expected v0.1 shape. Carries the underlying `serde_json` error
    /// message as a string (we deliberately do NOT keep the
    /// `serde_json::Error` value to keep `DescriptorError` `Send`/`Sync` and
    /// trivially cloneable in future).
    #[error("malformed JSON: {0}")]
    MalformedJson(String),

    /// `@context` was anything other than the canonical literal IRI
    // `JsonLdContextNotSupported` and `UnknownDatatype` lived here. Both were
    // about a CSVW sidecar; there is no sidecar.

    /// The descriptor parsed successfully but did not declare a
    /// `tableSchema`, and the type-checker requires one for forward
    /// type propagation.
    #[error("CSVW descriptor lacks tableSchema; cannot drive forward type propagation")]
    MissingTableSchema,
}

/// Stub: hardcoded CSV inference returning `{id: String, name: String}`.
///
/// The real path is [`InferredDescriptor`], built from the host's own
/// `DESCRIBE read_csv_auto(…)`.
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
