//! XSD, at the RDF border.
//!
//! [`Primitive`] is fossil's lattice and names no datatype IRI. A shape document
//! states a value's datatype as an XSD IRI, and a mapping writes one back, so
//! the two meet here and nowhere else: [`Xsd`] wraps the IRI, `TryFrom<Primitive>`
//! writes the canonical one (a `Uri` has none: it is a node), and `TryFrom<&Xsd>` reads one into the lattice.
//! The second is a `TryFrom` because the lattice is smaller than XSD:
//! `xsd:duration` has no primitive, and `xsd:decimal` folds into `Float`.

use serde::{Deserialize, Serialize};

use crate::{Lang, Precision, Primitive, RDF_LANG_STRING};

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
/// `xsd:integer`. A [`Primitive::Uri`] has none: it is written as an IRI node,
/// never a literal.
impl TryFrom<Primitive> for Xsd {
    type Error = NotALiteral;

    fn try_from(primitive: Primitive) -> Result<Self, NotALiteral> {
        Ok(Self::new(match primitive {
            Primitive::String(Lang::Untagged) => "http://www.w3.org/2001/XMLSchema#string",
            Primitive::String(Lang::Tagged) => RDF_LANG_STRING,
            Primitive::Integer => "http://www.w3.org/2001/XMLSchema#integer",
            Primitive::Float => "http://www.w3.org/2001/XMLSchema#double",
            Primitive::Bool => "http://www.w3.org/2001/XMLSchema#boolean",
            Primitive::Time(Precision::Year) => "http://www.w3.org/2001/XMLSchema#gYear",
            Primitive::Time(Precision::Date) => "http://www.w3.org/2001/XMLSchema#date",
            Primitive::Time(Precision::DateTime) => "http://www.w3.org/2001/XMLSchema#dateTime",
            Primitive::Time(Precision::Hour) => "http://www.w3.org/2001/XMLSchema#time",
            Primitive::Uri => return Err(NotALiteral),
        }))
    }
}

/// A primitive RDF writes as a node and not as a literal: [`Primitive::Uri`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the primitive is written as an IRI node, and has no datatype")]
pub struct NotALiteral;

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
            // `xsd:anyURI` is a literal, a text whose shape says so; an
            // identifier written as a node is `Term::Iri`, read as `Uri`.
            "string" | "normalizedString" | "token" | "language" | "anyURI" => Self::STRING,
            "integer" | "long" | "int" | "short" | "byte" | "nonNegativeInteger"
            | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" | "unsignedLong"
            | "unsignedInt" => Self::Integer,
            "decimal" | "float" | "double" | "number" => Self::Float,
            "boolean" => Self::Bool,
            "gYear" => Self::YEAR,
            "date" => Self::DATE,
            "dateTime" | "dateTimeStamp" => Self::DATE_TIME,
            "time" => Self::HOUR,
            // RDF's, not XSD's: the one datatype of the lattice outside the
            // `xsd:` namespace, read by its local name like the rest.
            "langString" => Self::LANG_STRING,
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
            Some(Primitive::STRING)
        );
        assert_eq!(read("xsd:integer"), Some(Primitive::Integer));
        assert_eq!(
            read("http://www.w3.org/2001/XMLSchema#double"),
            Some(Primitive::Float)
        );
        assert_eq!(
            read("http://www.w3.org/2001/XMLSchema#anyURI"),
            Some(Primitive::STRING),
            "an `xsd:anyURI` literal is a text; `Uri` is a node"
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
            ("dateTime", Primitive::DATE_TIME),
            ("anyURI", Primitive::STRING),
            ("gYear", Primitive::YEAR),
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
            assert_eq!(read(name), Some(Primitive::STRING), "{name}");
        }
        assert_eq!(read("dateTimeStamp"), Some(Primitive::DATE_TIME));
    }

    /// Outside the lattice is an error, never a silent `String`. `duration` is a
    /// real xsd type we do not carry; the caller decides what to do about it.
    #[test]
    fn an_xsd_type_outside_the_lattice_is_refused() {
        assert_eq!(read("duration"), None);
        assert_eq!(read("StringWithCase"), None);
        assert_eq!(read(""), None);
    }

    /// Every literal writes a datatype that reads back to itself — the two
    /// directions are inverses, not two tables that happen to agree today —
    /// and `Uri` alone writes none.
    #[test]
    fn every_primitive_round_trips_through_its_datatype() {
        for p in Primitive::ALL {
            match Xsd::try_from(p) {
                Ok(xsd) => assert_eq!(Primitive::try_from(&xsd), Ok(p), "{p:?}"),
                Err(NotALiteral) => assert_eq!(p, Primitive::Uri),
            }
        }
    }

    /// The wire carries the IRI as a plain string, as it did before the type.
    #[test]
    fn the_wire_is_the_bare_iri() {
        let xsd = Xsd::try_from(Primitive::DATE).expect("a literal");
        let json = serde_json::to_string(&xsd).expect("serialises");
        assert_eq!(json, "\"http://www.w3.org/2001/XMLSchema#date\"");
    }
}
