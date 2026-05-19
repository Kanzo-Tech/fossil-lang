//! CSVW Metadata Vocabulary v0.1 — minimum-subset thin parser.
//!
//! Implements the documented subset of the W3C "Metadata Vocabulary for
//! Tabular Data" recommendation (<https://www.w3.org/TR/tabular-metadata/>)
//! required for Phase 3 CORE-05 forward type propagation. No Rust crate
//! implements CSVW directly (verified 2026-05-19 — see
//! `.planning/phases/03-bidirectional-type-checker-shex-target/03-RESEARCH.md`
//! §"Standard Stack"), so this module hand-rolls serde-derived structs over
//! the documented minimum subset.
//!
//! ## Why a string-name lookup, not `Primitive` directly
//!
//! `datatype_to_primitive_name` returns `Option<&'static str>` (the canonical
//! [`Primitive`] variant name as a string) rather than a `Primitive` /
//! `Ty<'db>` value. The conversion from variant name to the interned `Ty<'db>`
//! lives in `fossil-hir/src/infer.rs` (plan 03-05). This split avoids a
//! circular dependency `fossil-descriptors-input` ↔ `fossil-hir`: the
//! descriptor crate stays compiler-substrate-independent and free of Salsa
//! types, while the hir crate owns the canonical type table.
//!
//! [`Primitive`]: ../../fossil_hir/ty/enum.Primitive.html
//!
//! ## What's IN the v0.1 subset
//!
//! - Literal `@context = "http://www.w3.org/ns/csvw"`. Object/array forms
//!   rejected at parse time with [`DescriptorError::JsonLdContextNotSupported`].
//! - `url` (optional, recorded but not validated).
//! - `tableSchema.columns[]` with each column carrying:
//!     - `name` (required).
//!     - `datatype` (optional) in two forms:
//!         - String form: `"string"`, `"integer"`, ..., optionally with
//!           `xsd:` prefix (stripped at lookup time).
//!         - Object form: `{ "base": "..." }`. The `format` field is IGNORED
//!           in v0.1 (no facet checking — that's a Phase 4 concern per
//!           CORE-09 / ROADMAP §Phase 4 SC#4).
//!     - `titles` (optional display-only string/array, NOT used for resolution).
//! - Unrecognised top-level fields are tolerated (no
//!   `#[serde(deny_unknown_fields)]` — forward-compat with future CSVW
//!   extensions).
//!
//! ## What's OUT (rejected or unsupported)
//!
//! - JSON-LD `@context` resolution (no remote IRI fetch).
//! - Array-form `@context` (e.g. `["http://www.w3.org/ns/csvw", {...}]`) — rejected.
//! - Compound object-form `@context` — rejected.
//! - CSVW facet checking (length, minInclusive, maxInclusive, pattern) — deferred to Phase 4.
//! - Foreign-key / primary-key declarations.
//! - Multi-table CSVW (`tables[]` at top level).
//! - Inheritance from parent metadata documents.
//!
//! See ADR-0007 (`decisions/0007-csvw-jsonld-subset-cutoff.md`) for the full
//! rationale.
//!
//! ## Datatype catalog
//!
//! [`datatype_to_primitive_name`] maps CSVW datatype names (with optional
//! `xsd:` prefix already stripped) to one of the 9 [`Primitive`] variant
//! names per `type-system.md` §2:
//!
//! | CSVW datatype                                                                | Primitive |
//! |------------------------------------------------------------------------------|-----------|
//! | `string`                                                                     | `String`  |
//! | `integer`, `long`, `int`, `short`, `byte`, `nonNegativeInteger`,             | `Integer` |
//! | `positiveInteger`, `nonPositiveInteger`, `negativeInteger`                   |           |
//! | `decimal`, `float`, `double`, `number`                                       | `Float`   |
//! | `boolean`                                                                    | `Bool`    |
//! | `date`                                                                       | `Date`    |
//! | `dateTime`, `dateTimeStamp`                                                  | `DateTime`|
//! | `time`                                                                       | `Time`    |
//! | `gYear`                                                                      | `GYear`   |
//! | `anyURI`                                                                     | `AnyURI`  |
//! | (anything else)                                                              | `None`    |
//!
//! On `None`, the caller (plan 03-05's checker) emits a structured
//! [`DescriptorError::UnknownDatatype`] diagnostic and types the column as
//! `Ty::Error` (NOT silent String fallback).

use serde::Deserialize;

use crate::DescriptorError;

/// Parsed CSVW metadata document (minimum subset per ADR-0007).
///
/// Deserialised directly from JSON via [`CsvwDescriptor::parse`].
#[derive(Deserialize, Debug, Clone)]
pub struct CsvwMetadata {
    /// JSON-LD `@context`. v0.1 ONLY accepts the canonical string
    /// `"http://www.w3.org/ns/csvw"`. Array / object forms are rejected at
    /// validation time (see [`CsvwDescriptor::parse`]).
    #[serde(rename = "@context")]
    pub context: serde_json::Value,

    /// Optional source URL (recorded but not validated against filesystem).
    pub url: Option<String>,

    /// Optional table schema describing the columns of the tabular source.
    #[serde(rename = "tableSchema")]
    pub table_schema: Option<TableSchema>,
}

/// Table-schema fragment of a CSVW metadata document.
#[derive(Deserialize, Debug, Clone)]
pub struct TableSchema {
    /// Ordered column declarations. CSVW does not require `columns` to be
    /// present, but if it IS, each column must declare a `name`.
    pub columns: Vec<Column>,
}

/// Single column declaration inside [`TableSchema::columns`].
#[derive(Deserialize, Debug, Clone)]
pub struct Column {
    /// Column name (required). This is the identifier the Fossil mapping
    /// references via field-projection (e.g. `users.age`).
    pub name: String,

    /// Optional datatype declaration. Two forms are accepted; see
    /// [`DatatypeForm`]. Absent → typed as inference fallback (currently
    /// returns `None` from [`CsvwDescriptor::type_for_column`]; plan 03-05
    /// decides whether to treat as String fallback or emit a diagnostic).
    pub datatype: Option<DatatypeForm>,

    /// Optional display titles. CSVW accepts a string or array of strings
    /// here, plus a JSON object keyed by language tag. We accept any
    /// `serde_json::Value` and IGNORE it — titles are display-only per the
    /// W3C spec and do not participate in type resolution.
    pub titles: Option<serde_json::Value>,
}

/// Two accepted forms for the `datatype` field on a column.
///
/// `#[serde(untagged)]` deserialises whichever shape matches: a bare JSON
/// string → [`DatatypeForm::Named`]; a JSON object with a `base` key →
/// [`DatatypeForm::Detailed`].
///
/// The `format` sub-field of the detailed form is CSVW spec but UNUSED in
/// v0.1 (facet checking is deferred to Phase 4 CORE-09). Future extension
/// point: add `format: Option<String>` here and route to a `FacetSpec` in
/// `fossil-mir`.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum DatatypeForm {
    /// Bare-string form, e.g. `"datatype": "integer"` or `"xsd:date"`.
    Named(String),
    /// Object form, e.g. `"datatype": { "base": "integer" }`. The `base`
    /// field is required; other fields are ignored.
    Detailed {
        /// The CSVW datatype name (with optional `xsd:` prefix).
        base: String,
    },
}

/// Top-level descriptor wrapping a parsed [`CsvwMetadata`].
///
/// Constructed via [`CsvwDescriptor::parse`], which performs JSON-LD
/// `@context` validation in addition to the underlying `serde_json` parse.
#[derive(Debug, Clone)]
pub struct CsvwDescriptor {
    /// The parsed (and validated) metadata document.
    pub metadata: CsvwMetadata,
}

impl CsvwDescriptor {
    /// Parse a CSVW Metadata JSON document.
    ///
    /// Errors:
    /// - [`DescriptorError::MalformedJson`] if `bytes` is not valid JSON or
    ///   does not match the documented v0.1 shape.
    /// - [`DescriptorError::JsonLdContextNotSupported`] if `@context` is
    ///   anything other than the literal string `"http://www.w3.org/ns/csvw"`.
    pub fn parse(bytes: &[u8]) -> Result<Self, DescriptorError> {
        let metadata: CsvwMetadata = serde_json::from_slice(bytes)
            .map_err(|e| DescriptorError::MalformedJson(e.to_string()))?;
        Self::validate_context(&metadata.context)?;
        Ok(Self { metadata })
    }

    /// Validate that `@context` is the canonical CSVW IRI string.
    ///
    /// Array and object forms are rejected with
    /// [`DescriptorError::JsonLdContextNotSupported`] per ADR-0007.
    fn validate_context(ctx: &serde_json::Value) -> Result<(), DescriptorError> {
        match ctx {
            serde_json::Value::String(s) if s == "http://www.w3.org/ns/csvw" => Ok(()),
            serde_json::Value::String(_) => Err(DescriptorError::JsonLdContextNotSupported(
                "non-canonical @context IRI; expected exactly \"http://www.w3.org/ns/csvw\"".into(),
            )),
            _ => Err(DescriptorError::JsonLdContextNotSupported(
                "array/object-form @context not supported in v0.1; \
                 use exactly \"http://www.w3.org/ns/csvw\""
                    .into(),
            )),
        }
    }

    /// Iterate over the declared columns, if any.
    ///
    /// Returns an empty iterator if `tableSchema` is absent — callers
    /// should treat this case as "schema not declared" rather than
    /// "schema is empty".
    pub fn columns(&self) -> impl Iterator<Item = &Column> {
        self.metadata
            .table_schema
            .as_ref()
            .map(|ts| ts.columns.iter())
            .into_iter()
            .flatten()
    }

    /// Look up the [`Primitive`] variant name for the given column.
    ///
    /// Returns `None` when:
    /// - `tableSchema` is absent.
    /// - No column with that `name` exists.
    /// - The column exists but carries no `datatype` field.
    /// - The column's datatype is not in the known v0.1 catalog.
    ///
    /// The four cases are NOT distinguished by this function; plan 03-05's
    /// checker disambiguates via [`CsvwDescriptor::columns`] / [`Column::name`] /
    /// [`Column::datatype`] to emit a precise diagnostic.
    ///
    /// [`Primitive`]: ../../fossil_hir/ty/enum.Primitive.html
    pub fn type_for_column(&self, name: &str) -> Option<&'static str> {
        let c = self.columns().find(|c| c.name == name)?;
        let dt = c.datatype.as_ref()?;
        let dt_name = match dt {
            DatatypeForm::Named(s) => s.as_str(),
            DatatypeForm::Detailed { base } => base.as_str(),
        };
        // Strip optional "xsd:" prefix per W3C tabular-metadata spec (§ 5.11.1
        // Datatypes; xsd is the canonical prefix when present).
        let bare = dt_name.strip_prefix("xsd:").unwrap_or(dt_name);
        datatype_to_primitive_name(bare)
    }
}

/// Map a CSVW datatype name (with `xsd:` prefix already stripped) to the
/// canonical [`Primitive`] variant name as a `&'static str`.
///
/// See the module-level documentation for the full catalog. Returns `None`
/// for any unrecognised name; callers emit
/// [`DescriptorError::UnknownDatatype`] with the column name + datatype
/// string for a precise diagnostic.
///
/// The returned string is the EXACT variant name in `fossil-hir::ty::Primitive`
/// (verified against `crates/fossil-hir/src/ty.rs` — `String`, `Integer`,
/// `Float`, `Bool`, `Date`, `DateTime`, `Time`, `GYear`, `AnyURI`).
///
/// [`Primitive`]: ../../fossil_hir/ty/enum.Primitive.html
pub fn datatype_to_primitive_name(name: &str) -> Option<&'static str> {
    match name {
        "string" => Some("String"),
        // The xsd integer family — all collapse to Primitive::Integer for v0.1.
        // Refinement (signed vs. unsigned, width) is Phase 3+ at the earliest
        // per RESEARCH.md §Q4 and out of scope for Milestone 1.
        "integer" | "long" | "int" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" => Some("Integer"),
        // The xsd numeric family — collapse to Primitive::Float per RESEARCH.md
        // §Example 3. CSVW's `number` is an additional alias used by some
        // descriptors; it maps to Float because CSVW does not distinguish.
        "decimal" | "float" | "double" | "number" => Some("Float"),
        "boolean" => Some("Bool"),
        "date" => Some("Date"),
        // Both dateTime variants map to Primitive::DateTime; the W3C spec
        // distinguishes `dateTimeStamp` (offset-required) from `dateTime`
        // (offset-optional), but v0.1 collapses them.
        "dateTime" | "dateTimeStamp" => Some("DateTime"),
        "time" => Some("Time"),
        "gYear" => Some("GYear"),
        "anyURI" => Some("AnyURI"),
        _ => None,
    }
}

#[cfg(test)]
// Test names intentionally mirror the `Primitive` variant names (e.g.
// `Bool`, `AnyURI`) for one-to-one readability — `datatype_anyURI_maps_to_AnyURI`
// is more grep-friendly than `datatype_any_uri_maps_to_any_uri`. Justified
// non-snake-case ONLY in tests; production code follows the workspace lint.
#[allow(non_snake_case)]
mod tests {
    use super::*;

    // ---- Parsing happy-path -------------------------------------------------

    #[test]
    fn parses_minimal_csvw_with_one_string_column() {
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "url": "users.csv",
            "tableSchema": {
                "columns": [
                    {"name": "id", "datatype": "string"}
                ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).expect("happy-path CSVW should parse");
        assert_eq!(d.metadata.url.as_deref(), Some("users.csv"));
        assert_eq!(d.type_for_column("id"), Some("String"));
    }

    #[test]
    fn parses_csvw_with_object_form_datatype() {
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "tableSchema": {
                "columns": [
                    {"name": "age", "datatype": {"base": "integer"}}
                ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).unwrap();
        assert_eq!(d.type_for_column("age"), Some("Integer"));
    }

    #[test]
    fn parses_csvw_with_xsd_prefix_on_datatype() {
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "tableSchema": {
                "columns": [
                    {"name": "birthday", "datatype": "xsd:date"}
                ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).unwrap();
        assert_eq!(d.type_for_column("birthday"), Some("Date"));
    }

    // ---- datatype_to_primitive_name catalog --------------------------------

    #[test]
    fn datatype_string_maps_to_String() {
        assert_eq!(datatype_to_primitive_name("string"), Some("String"));
    }

    #[test]
    fn datatype_integer_family_collapses() {
        for name in [
            "integer",
            "long",
            "int",
            "short",
            "byte",
            "nonNegativeInteger",
            "positiveInteger",
            "nonPositiveInteger",
            "negativeInteger",
        ] {
            assert_eq!(
                datatype_to_primitive_name(name),
                Some("Integer"),
                "{name} should collapse to Integer"
            );
        }
    }

    #[test]
    fn datatype_float_family_collapses() {
        for name in ["decimal", "float", "double", "number"] {
            assert_eq!(
                datatype_to_primitive_name(name),
                Some("Float"),
                "{name} should collapse to Float"
            );
        }
    }

    #[test]
    fn datatype_boolean_maps_to_Bool() {
        assert_eq!(datatype_to_primitive_name("boolean"), Some("Bool"));
    }

    #[test]
    fn datatype_date_maps_to_Date() {
        assert_eq!(datatype_to_primitive_name("date"), Some("Date"));
    }

    #[test]
    fn datatype_dateTime_maps_to_DateTime() {
        assert_eq!(datatype_to_primitive_name("dateTime"), Some("DateTime"));
        assert_eq!(
            datatype_to_primitive_name("dateTimeStamp"),
            Some("DateTime")
        );
    }

    #[test]
    fn datatype_time_maps_to_Time() {
        assert_eq!(datatype_to_primitive_name("time"), Some("Time"));
    }

    #[test]
    fn datatype_gYear_maps_to_GYear() {
        assert_eq!(datatype_to_primitive_name("gYear"), Some("GYear"));
    }

    #[test]
    fn datatype_anyURI_maps_to_AnyURI() {
        assert_eq!(datatype_to_primitive_name("anyURI"), Some("AnyURI"));
    }

    #[test]
    fn unknown_datatype_returns_none() {
        // `duration` is in xsd but NOT in our v0.1 catalog. Callers must
        // emit a diagnostic and type the column as `Ty::Error`.
        assert_eq!(datatype_to_primitive_name("duration"), None);
        assert_eq!(datatype_to_primitive_name(""), None);
        assert_eq!(datatype_to_primitive_name("StringWithCase"), None);
    }

    // ---- @context validation ----------------------------------------------

    #[test]
    fn array_form_context_rejected() {
        let json = br#"{
            "@context": ["http://www.w3.org/ns/csvw", {}],
            "tableSchema": { "columns": [] }
        }"#;
        let err = CsvwDescriptor::parse(json).unwrap_err();
        match err {
            DescriptorError::JsonLdContextNotSupported(msg) => {
                assert!(msg.contains("array/object-form"));
            }
            other => panic!("expected JsonLdContextNotSupported, got {other:?}"),
        }
    }

    #[test]
    fn non_canonical_context_iri_rejected() {
        let json = br#"{
            "@context": "http://example.org/wrong",
            "tableSchema": { "columns": [] }
        }"#;
        let err = CsvwDescriptor::parse(json).unwrap_err();
        match err {
            DescriptorError::JsonLdContextNotSupported(msg) => {
                assert!(msg.contains("non-canonical"));
            }
            other => panic!("expected JsonLdContextNotSupported, got {other:?}"),
        }
    }

    // ---- type_for_column edge cases ---------------------------------------

    #[test]
    fn missing_table_schema_returns_metadata_with_none_schema() {
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "url": "schemaless.csv"
        }"#;
        let d = CsvwDescriptor::parse(json).unwrap();
        assert!(d.metadata.table_schema.is_none());
        assert!(d.type_for_column("anything").is_none());
    }

    #[test]
    fn unknown_column_returns_none() {
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "tableSchema": {
                "columns": [
                    {"name": "id", "datatype": "string"}
                ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).unwrap();
        assert!(d.type_for_column("nonexistent").is_none());
    }

    #[test]
    fn column_without_datatype_returns_none() {
        // Per plan 03-05's checker contract, missing-datatype is distinct
        // from unknown-datatype: the former is "no declaration" (caller
        // chooses fallback), the latter is "declared but unrecognised"
        // (caller emits UnknownDatatype diagnostic). type_for_column
        // returns None for BOTH; the disambiguation happens in the caller
        // by inspecting `Column::datatype`.
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "tableSchema": {
                "columns": [
                    {"name": "raw"}
                ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).unwrap();
        let col = d.columns().find(|c| c.name == "raw").unwrap();
        assert!(col.datatype.is_none());
        assert!(d.type_for_column("raw").is_none());
    }

    // ---- Extras: titles tolerated; unknown fields tolerated ---------------

    #[test]
    fn titles_field_present_but_ignored_for_resolution() {
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "tableSchema": {
                "columns": [
                    {"name": "id", "datatype": "string", "titles": ["Identifier", "ID"]}
                ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).unwrap();
        let col = d.columns().find(|c| c.name == "id").unwrap();
        assert!(col.titles.is_some(), "titles should deserialize");
        // `titles` is present but unused for type resolution.
        assert_eq!(d.type_for_column("id"), Some("String"));
    }

    #[test]
    fn unknown_top_level_field_tolerated_for_forward_compat() {
        // Future CSVW extensions may introduce new top-level fields; we
        // accept and ignore them rather than `deny_unknown_fields`.
        let json = br#"{
            "@context": "http://www.w3.org/ns/csvw",
            "futureField": {"experimental": true},
            "tableSchema": {
                "columns": [ {"name": "id", "datatype": "string"} ]
            }
        }"#;
        let d = CsvwDescriptor::parse(json).expect("unknown top-level field should be tolerated");
        assert_eq!(d.type_for_column("id"), Some("String"));
    }

    #[test]
    fn malformed_json_returns_malformed_error() {
        let bad = b"{not valid json";
        let err = CsvwDescriptor::parse(bad).unwrap_err();
        assert!(matches!(err, DescriptorError::MalformedJson(_)));
    }
}
