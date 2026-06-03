//! Derive an [`InferredDescriptor`] from a `ShEx` shape — the **compile-time,
//! always** source-type path for RDF inputs (`io.rdf("data.ttl", schema:
//! "person.shex")`).
//!
//! A `ShEx` shape *is* the schema: each triple constraint declares a predicate
//! (→ column name) and a value type (→ column [`Primitive`]). Unlike CSV/JSON
//! (where types are introspected from the data at compile time), RDF types come
//! from the declared shape — never sniffed from the data. The shape resolution
//! (parse + `EachOf` flatten + `Ref` resolution + the `valueExpr` narrowing)
//! lives in the neutral [`fossil_shex`] crate, shared with the output
//! descriptor's backward checker; here we only map the resolved
//! [`fossil_shex::ConstraintValue`] onto the [`Primitive`] lattice via the same
//! `xsd:` table the CSVW path uses ([`datatype_to_primitive_name`]).
//!
//! [`Primitive`]: ../../fossil_hir/ty/enum.Primitive.html
//! [`datatype_to_primitive_name`]: crate::datatype_to_primitive_name

use fossil_shex::{ConstraintValue, ShExDescriptor};

use crate::csvw::datatype_to_primitive_name;
use crate::inferred::{InferredColumn, InferredDescriptor};

/// Failure modes of deriving a descriptor from a `ShEx` schema.
#[derive(Debug, thiserror::Error)]
pub enum ShExInputError {
    /// The `ShEx` JSON could not be parsed / resolved.
    #[error("failed to parse ShEx schema: {0}")]
    Parse(String),
    /// The requested target shape IRI is absent from the schema.
    #[error("shape `{0}` not found in the ShEx schema")]
    ShapeNotFound(String),
}

/// Build the [`InferredDescriptor`] for an RDF source from its `ShEx` schema.
///
/// Each resolved triple constraint of the target shape becomes one column: the
/// predicate's local name is the column name, and the `valueExpr` narrows to a
/// [`Primitive`] (typed literal → that primitive; IRI / object reference →
/// `AnyURI`; un-narrowable → `String`). Compile-time + deterministic: no data is
/// read, so the same `(shape, shape IRI)` always yields the same columns.
///
/// # Errors
///
/// [`ShExInputError::Parse`] if the schema is malformed; [`ShExInputError::ShapeNotFound`]
/// if `shape_iri` names no shape in the schema.
///
/// [`Primitive`]: ../../fossil_hir/ty/enum.Primitive.html
pub fn inferred_descriptor_from_shex(
    source_name: &str,
    shape_iri: &str,
    shex_json: &[u8],
) -> Result<InferredDescriptor, ShExInputError> {
    let descriptor = ShExDescriptor::from_reader(shex_json)
        .map_err(|e| ShExInputError::Parse(format!("{e:?}")))?;
    let binding = descriptor
        .lookup_shape_str(shape_iri)
        .ok_or_else(|| ShExInputError::ShapeNotFound(shape_iri.to_string()))?;

    let columns = binding
        .constraints
        .iter()
        .map(|c| InferredColumn {
            name: c.predicate_local_name().into(),
            primitive: column_primitive(&c.value()).into(),
        })
        .collect();

    Ok(InferredDescriptor {
        source_name: source_name.into(),
        columns,
        content_hash: String::new(),
    })
}

/// Map a narrowed [`ConstraintValue`] to a canonical `Primitive` variant name.
/// Datatype IRIs reuse the CSVW `xsd:` → primitive table (stripped to local
/// name); IRI / object references are `AnyURI`; anything un-narrowable falls
/// back to `String` (mirrors the CSVW unknown-datatype behaviour).
fn column_primitive(value: &ConstraintValue) -> &'static str {
    match value {
        ConstraintValue::Datatype(iri) => {
            datatype_to_primitive_name(local_name(iri)).unwrap_or("String")
        }
        ConstraintValue::Iri => "AnyURI",
        ConstraintValue::Unknown => "String",
    }
}

/// The local name of an IRI — the substring after the last `#` or `/`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ex:Person` with name (string), age (integer), and homepage (an IRI node).
    const PERSON_SCHEMA: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Person",
          "shapeExpr": {
            "type": "Shape",
            "expression": {
              "type": "EachOf",
              "expressions": [
                {
                  "type": "TripleConstraint",
                  "predicate": "http://xmlns.com/foaf/0.1/name",
                  "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#string" }
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://xmlns.com/foaf/0.1/age",
                  "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#integer" }
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://xmlns.com/foaf/0.1/homepage",
                  "valueExpr": { "type": "NodeConstraint", "nodeKind": "iri" }
                }
              ]
            }
          }
        }
      ]
    }"#;

    fn primitive_of<'a>(d: &'a InferredDescriptor, col: &str) -> &'a str {
        d.columns
            .iter()
            .find(|c| c.name == col)
            .unwrap_or_else(|| panic!("column `{col}` missing"))
            .primitive
            .as_str()
    }

    #[test]
    fn derives_columns_and_primitives_from_the_shape() {
        let d = inferred_descriptor_from_shex(
            "people",
            "http://example.org/Person",
            PERSON_SCHEMA.as_bytes(),
        )
        .expect("descriptor derives");

        assert_eq!(d.source_name, "people");
        assert_eq!(d.columns.len(), 3, "one column per triple constraint");
        // Predicate local name → column name; datatype → Primitive.
        assert_eq!(primitive_of(&d, "name"), "String");
        assert_eq!(primitive_of(&d, "age"), "Integer");
        // nodeKind IRI → an IRI-valued column (an edge target).
        assert_eq!(primitive_of(&d, "homepage"), "AnyURI");
    }

    #[test]
    fn missing_shape_is_an_error() {
        let err = inferred_descriptor_from_shex(
            "people",
            "http://example.org/Nope",
            PERSON_SCHEMA.as_bytes(),
        )
        .unwrap_err();
        assert!(matches!(err, ShExInputError::ShapeNotFound(_)));
    }

    #[test]
    fn malformed_schema_is_a_parse_error() {
        let err = inferred_descriptor_from_shex("x", "http://example.org/X", b"not json")
            .unwrap_err();
        assert!(matches!(err, ShExInputError::Parse(_)));
    }
}
