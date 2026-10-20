//! XSD, at the RDF border.
//!
//! [`Primitive`] is fossil's lattice and names no datatype IRI. A shape document
//! states a value's datatype as an XSD IRI, and a mapping writes one back, so
//! the two meet here and nowhere else: [`Xsd`] wraps the IRI, `From<Primitive>`
//! writes the canonical one, and `TryFrom<&Xsd>` reads one into the lattice.
//! The second is a `TryFrom` because the lattice is smaller than XSD:
//! `xsd:duration` has no primitive, and `xsd:decimal` folds into `Float`.

use serde::{Deserialize, Serialize};

use crate::{Primitive, RDF_LANG_STRING};

/// An XSD datatype IRI, exactly as written: the full
/// `http://www.w3.org/2001/XMLSchema#<name>` IRI, the `xsd:<name>` prefixed
/// form, or the bare local name. It is kept as written, not normalised, so
/// `xsd:decimal` stays `xsd:decimal` in a mapping however the lattice folds it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Xsd(String);

impl Xsd {
    /// The datatype `iri` names.
    #[must_use]
    pub fn new(iri: impl Into<String>) -> Self {
        Self(iri.into())
    }

    /// The IRI as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Xsd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The canonical datatype of each primitive, the one a mapping writes. An alias
/// like `xsd:long` reads in as [`Primitive::Integer`] and comes back as
/// `xsd:integer`.
impl From<Primitive> for Xsd {
    fn from(primitive: Primitive) -> Self {
        Self::new(match primitive {
            Primitive::String => "http://www.w3.org/2001/XMLSchema#string",
            Primitive::Integer => "http://www.w3.org/2001/XMLSchema#integer",
            Primitive::Float => "http://www.w3.org/2001/XMLSchema#double",
            Primitive::Bool => "http://www.w3.org/2001/XMLSchema#boolean",
            Primitive::Date => "http://www.w3.org/2001/XMLSchema#date",
            Primitive::DateTime => "http://www.w3.org/2001/XMLSchema#dateTime",
            Primitive::Time => "http://www.w3.org/2001/XMLSchema#time",
            Primitive::GYear => "http://www.w3.org/2001/XMLSchema#gYear",
            Primitive::AnyUri => "http://www.w3.org/2001/XMLSchema#anyURI",
            Primitive::LangString => RDF_LANG_STRING,
        })
    }
}

/// A datatype the lattice has no primitive for. Callers decide between a
/// diagnostic and a `String` fallback, and they do differ: `ShEx` narrowing
/// takes the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the datatype has no primitive in the lattice")]
pub struct OutsideLattice;

/// The only XSD → [`Primitive`] table in the tree. It reads the datatype by its
/// local name, so the three spellings [`Xsd`] admits reach one variant.
impl TryFrom<&Xsd> for Primitive {
    type Error = OutsideLattice;

    fn try_from(xsd: &Xsd) -> Result<Self, OutsideLattice> {
        let local = xsd.0.rsplit(['#', '/', ':']).next().unwrap_or(&xsd.0);
        Ok(match local {
            "string" | "normalizedString" | "token" | "language" => Self::String,
            "integer" | "long" | "int" | "short" | "byte" | "nonNegativeInteger"
            | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" | "unsignedLong"
            | "unsignedInt" => Self::Integer,
            "decimal" | "float" | "double" | "number" => Self::Float,
            "boolean" => Self::Bool,
            "date" => Self::Date,
            "dateTime" | "dateTimeStamp" => Self::DateTime,
            "time" => Self::Time,
            "gYear" => Self::GYear,
            "anyURI" => Self::AnyUri,
            // RDF's, not XSD's: the one datatype of the lattice outside the
            // `xsd:` namespace, read by its local name like the rest.
            "langString" => Self::LangString,
            _ => return Err(OutsideLattice),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(iri: &str) -> Option<Primitive> {
        Primitive::try_from(&Xsd::new(iri)).ok()
    }

    #[test]
    fn datatype_iris_read_into_the_lattice() {
        assert_eq!(
            read("http://www.w3.org/2001/XMLSchema#string"),
            Some(Primitive::String)
        );
        assert_eq!(read("xsd:integer"), Some(Primitive::Integer));
        assert_eq!(
            read("http://www.w3.org/2001/XMLSchema#double"),
            Some(Primitive::Float)
        );
        assert_eq!(
            read("http://www.w3.org/2001/XMLSchema#anyURI"),
            Some(Primitive::AnyUri)
        );
        assert_eq!(read("http://example.org/Custom"), None);
    }

    /// The three spellings the tree carries reach the same variant: a bare
    /// local name, a `ShEx` `valueExpr` (full IRI) and a prefixed form are one
    /// lookup, so there is no second table to disagree with.
    #[test]
    fn every_xsd_spelling_reaches_one_lattice() {
        for (name, want) in [
            ("integer", Primitive::Integer),
            ("dateTime", Primitive::DateTime),
            ("anyURI", Primitive::AnyUri),
            ("gYear", Primitive::GYear),
        ] {
            let iri = format!("http://www.w3.org/2001/XMLSchema#{name}");
            let prefixed = format!("xsd:{name}");
            assert_eq!(read(&iri), Some(want), "full IRI: {iri}");
            assert_eq!(read(&prefixed), Some(want), "prefixed: {prefixed}");
            assert_eq!(read(name), Some(want), "bare: {name}");
        }
    }

    /// The xsd families collapse, and the collapse is the whole point of a
    /// lattice: v0.1 does not distinguish width or signedness.
    #[test]
    fn the_xsd_families_collapse() {
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
            "unsignedLong",
            "unsignedInt",
        ] {
            assert_eq!(read(name), Some(Primitive::Integer), "{name}");
        }
        for name in ["decimal", "float", "double", "number"] {
            assert_eq!(read(name), Some(Primitive::Float), "{name}");
        }
        for name in ["string", "normalizedString", "token", "language"] {
            assert_eq!(read(name), Some(Primitive::String), "{name}");
        }
        assert_eq!(read("dateTimeStamp"), Some(Primitive::DateTime));
    }

    /// Outside the lattice is an error, never a silent `String`. `duration` is a
    /// real xsd type we do not carry; the caller decides what to do about it.
    #[test]
    fn an_xsd_type_outside_the_lattice_is_refused() {
        assert_eq!(read("duration"), None);
        assert_eq!(read("StringWithCase"), None);
        assert_eq!(read(""), None);
    }

    /// Every variant writes a datatype that reads back to itself — the two
    /// directions are inverses, not two tables that happen to agree today.
    #[test]
    fn every_primitive_round_trips_through_its_datatype() {
        for p in Primitive::ALL {
            assert_eq!(Primitive::try_from(&Xsd::from(p)), Ok(p), "{p:?}");
        }
    }

    /// The wire carries the IRI as a plain string, as it did before the type.
    #[test]
    fn the_wire_is_the_bare_iri() {
        let xsd = Xsd::from(Primitive::Date);
        let json = serde_json::to_string(&xsd).expect("serialises");
        assert_eq!(json, "\"http://www.w3.org/2001/XMLSchema#date\"");
    }
}
