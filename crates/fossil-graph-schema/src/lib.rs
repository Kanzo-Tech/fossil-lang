//! The canonical **graph-schema** — the shared contract of fossil's universal
//! substrate.
//!
//! # The model (one idea)
//!
//! A property graph is **not a data structure — it is a SCHEMA over typed
//! relations** (the SQL:2023 / PGQ + DuckPGQ convergence). The data are plain
//! relations (Arrow tables); the *graph* is an interpretation that declares:
//! which relations are node types, what identifies a node (its **key** — here,
//! the subject IRI), and which predicates **reference** another node type (the
//! edges). RDF, GraphAr (`dense_id`/CSR-CSC), edge-lists, traversal, DCAT are
//! then all **views or materializers** over this single schema + the relations.
//!
//! This type is that schema, made first-class and **format-neutral**: it carries
//! no `dense_id`, no Parquet/CSR detail, no GraphAr/xsd spelling. Those belong to
//! the *materializers*, derived from this schema on demand. Keeping the contract
//! free of any storage/format detail is what lets a new output format be "just
//! another materializer" without touching the producer or the consumer — the
//! modifiability driver (D2) of the architecture.
//!
//! # Why dependency-free
//!
//! The schema is spoken by every side at once — the producer (`fossil-df`), each
//! materializer, the consumer (`fossil-graph`), and the wire/manifest. So this
//! crate depends on nothing but `serde`: anyone can deserialize and interpret a
//! graph without pulling the ShEx descriptor or Arrow. *Decoding* a ShEx
//! document (which does need that machinery) lives next to the descriptor, not
//! here — the contract stays pure. What the decoder produces is [`shapes`],
//! which is in this crate precisely because it too must be speakable without
//! ShEx.
//!
//! # Identity & references
//!
//! Every node's key is its **subject IRI** (uniform across fossil). An edge
//! `source → destination` references node types by `label`; the relational plan
//! computes the actual IRI-valued columns, and a GraphAr materializer resolves
//! those IRIs to dense ids. The schema only states the shape.
//!
//! # Two contracts, one crate
//!
//! [`GraphSchema`] is the **output** model — what gets written. [`shapes`] is
//! the **input** side of the same border: what a shape document *says*, also
//! format-neutral, so the middle of the compiler can read a document's
//! constraints without linking a schema language. [`OutputShapes::to_graph_schema`]
//! is the one function between them.

use serde::{Deserialize, Serialize};

pub mod shapes;

pub use shapes::{Occurs, OutputShapes, PropertyConstraint, Rejection, Shape, local_name};

/// A whole graph's schema: its node types and edge types. The single contract
/// shared by the producer, every materializer, and the consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSchema {
    pub nodes: Vec<NodeType>,
    pub edges: Vec<EdgeType>,
}

/// One node type — a relation interpreted as vertices keyed by their subject IRI
/// (SQL/PGQ `VERTEX TABLE … KEY (id)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeType {
    /// The type's local label, e.g. `"Person"` (the relation name).
    pub label: String,
    /// The `rdf:type` IRI of the shape (e.g. `https://example.org/Person`) — the
    /// RDF-border metadata DCAT/RDF views read. `None` for a non-RDF graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// The literal-valued properties. The key (subject IRI) is implicit and
    /// uniform — it is not listed here.
    pub properties: Vec<Property>,
}

/// One literal property column of a node type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    /// Column / predicate local name, e.g. `"name"`.
    pub name: String,
    /// The canonical (format-neutral) datatype. A materializer derives its own
    /// spelling from this (GraphAr `int64`, xsd `…#integer`, …).
    pub datatype: Primitive,
    /// The full RDF predicate IRI (e.g. `https://example.org/name`) — RDF-border
    /// metadata. `None` for a non-RDF graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// Single- vs multi-valued (drives dedup vs keep-all in a materializer).
    pub cardinality: Cardinality,
}

/// One edge type — a predicate that references one node type from another
/// (SQL/PGQ `EDGE TABLE … SOURCE … DESTINATION … REFERENCES …`). Endpoints are
/// named by node `label`; the relational plan supplies the IRI columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeType {
    /// The edge's local label, e.g. `"placedBy"`.
    pub label: String,
    /// The full RDF predicate IRI. `None` for a non-RDF graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// Source node type label (the edge originates here).
    pub source: String,
    /// Destination node type label (the edge points here).
    pub destination: String,
    /// Single- vs multi-valued (at most one edge per source vs keep-all).
    pub cardinality: Cardinality,
}

/// The canonical datatype lattice — format-neutral, and **the same enum the type
/// system checks against** (`TyKind::Primitive`). It lives in this crate because
/// the schema, the checker and the descriptors all speak it, and a lattice that
/// crosses a crate boundary as a string is a lattice with no single definition.
///
/// Materializers map it to their own vocabulary (GraphAr `string/int64/double/…`,
/// `DataFusion` scalars, `DuckDB` column types) next to the materializer; only the
/// xsd direction lives here, because xsd is the RDF border every side reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
    String,
    Integer,
    Float,
    Bool,
    Date,
    DateTime,
    Time,
    GYear,
    AnyUri,
}

/// Cardinality of a property or edge — the only distinction a materializer
/// needs. The richer `(min, max)` form a shape document states is [`Occurs`],
/// and [`Occurs::collapse`] is the only way from there to here: at most one
/// value is [`Single`](Cardinality::Single), anything else
/// [`Multi`](Cardinality::Multi).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// At most one value/edge per subject — a materializer dedups by key.
    Single,
    /// Keep all values/edges per subject (multi-valued / `UNNEST`).
    Multi,
}

impl Primitive {
    /// Parse an XSD datatype IRI into the lattice. Accepts all three spellings
    /// the tree carries: the full `http://www.w3.org/2001/XMLSchema#<name>` IRI,
    /// the `xsd:<name>` prefixed form, and the bare local name a CSVW `datatype`
    /// field holds. **This is the only xsd → [`Primitive`] table in the tree.**
    ///
    /// `None` for an XSD type outside the lattice — callers decide between a
    /// diagnostic and a `String` fallback, and the two do differ (CSVW says so,
    /// `ShEx` narrowing does not).
    #[must_use]
    pub fn from_xsd_iri(iri: &str) -> Option<Self> {
        let local = iri.rsplit(['#', '/', ':']).next().unwrap_or(iri);
        Some(match local {
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
            _ => return None,
        })
    }

    /// The canonical XSD datatype IRI for this primitive — the output spec's
    /// literal datatype, carried into the manifest for the host's governance
    /// layer (DCAT). The inverse direction of [`Self::from_xsd_iri`], and the
    /// only one: an alias like `xsd:long` parses in and comes back as
    /// `xsd:integer`.
    #[must_use]
    pub const fn to_xsd_iri(self) -> &'static str {
        match self {
            Self::String => "http://www.w3.org/2001/XMLSchema#string",
            Self::Integer => "http://www.w3.org/2001/XMLSchema#integer",
            Self::Float => "http://www.w3.org/2001/XMLSchema#double",
            Self::Bool => "http://www.w3.org/2001/XMLSchema#boolean",
            Self::Date => "http://www.w3.org/2001/XMLSchema#date",
            Self::DateTime => "http://www.w3.org/2001/XMLSchema#dateTime",
            Self::Time => "http://www.w3.org/2001/XMLSchema#time",
            Self::GYear => "http://www.w3.org/2001/XMLSchema#gYear",
            Self::AnyUri => "http://www.w3.org/2001/XMLSchema#anyURI",
        }
    }
}

impl GraphSchema {
    /// Look up a node type by label.
    #[must_use]
    pub fn node(&self, label: &str) -> Option<&NodeType> {
        self.nodes.iter().find(|n| n.label == label)
    }

    /// Look up an edge type by label.
    #[must_use]
    pub fn edge(&self, label: &str) -> Option<&EdgeType> {
        self.edges.iter().find(|e| e.label == label)
    }

    /// Look up a node type by its `rdf:type` IRI (the descriptor key). The output
    /// model is addressed by IRI — a shape's `rdf:type` — not by the local label.
    #[must_use]
    pub fn node_by_iri(&self, iri: &str) -> Option<&NodeType> {
        self.nodes.iter().find(|n| n.iri.as_deref() == Some(iri))
    }

    /// Every edge type originating at `source_label` for the predicate `iri`.
    /// Returns more than one when the predicate's range is a union of node types
    /// (`@<A> OR @<B>`, `sh:or`) — the reference RDF→property-graph model emits one
    /// edge type per destination, sharing the predicate. Object IRIs partition
    /// cleanly across the destinations at materialisation (disjoint vertex tables).
    pub fn edges_from<'a>(
        &'a self,
        source_label: &'a str,
        iri: &'a str,
    ) -> impl Iterator<Item = &'a EdgeType> + 'a {
        self.edges
            .iter()
            .filter(move |e| e.source == source_label && e.iri.as_deref() == Some(iri))
    }
}

impl NodeType {
    /// Look up a literal/IRI property by its predicate IRI (the descriptor key).
    #[must_use]
    pub fn property_by_iri(&self, iri: &str) -> Option<&Property> {
        self.properties
            .iter()
            .find(|p| p.iri.as_deref() == Some(iri))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example (`hello`/`edges`): two node types, one edge — exactly
    /// what the universal substrate says fossil produces (relations + this).
    fn worked_example() -> GraphSchema {
        GraphSchema {
            nodes: vec![
                NodeType {
                    label: "Person".into(),
                    iri: Some("https://example.org/Person".into()),
                    properties: vec![Property {
                        name: "name".into(),
                        datatype: Primitive::String,
                        iri: Some("https://example.org/name".into()),
                        cardinality: Cardinality::Single,
                    }],
                },
                NodeType {
                    label: "Order".into(),
                    iri: Some("https://example.org/Order".into()),
                    properties: vec![Property {
                        name: "total".into(),
                        datatype: Primitive::Integer,
                        iri: Some("https://example.org/total".into()),
                        cardinality: Cardinality::Single,
                    }],
                },
            ],
            edges: vec![EdgeType {
                label: "placedBy".into(),
                iri: Some("https://example.org/placedBy".into()),
                source: "Order".into(),
                destination: "Person".into(),
                cardinality: Cardinality::Single,
            }],
        }
    }

    #[test]
    fn lookups_resolve_by_label() {
        let g = worked_example();
        assert_eq!(g.node("Person").unwrap().properties[0].name, "name");
        let e = g.edge("placedBy").expect("placedBy edge");
        assert_eq!(
            (e.source.as_str(), e.destination.as_str()),
            ("Order", "Person")
        );
    }

    #[test]
    fn datatype_parses_xsd_iris() {
        assert_eq!(
            Primitive::from_xsd_iri("http://www.w3.org/2001/XMLSchema#string"),
            Some(Primitive::String)
        );
        assert_eq!(
            Primitive::from_xsd_iri("xsd:integer"),
            Some(Primitive::Integer)
        );
        assert_eq!(
            Primitive::from_xsd_iri("http://www.w3.org/2001/XMLSchema#double"),
            Some(Primitive::Float)
        );
        assert_eq!(
            Primitive::from_xsd_iri("http://www.w3.org/2001/XMLSchema#anyURI"),
            Some(Primitive::AnyUri)
        );
        assert_eq!(Primitive::from_xsd_iri("http://example.org/Custom"), None);
    }

    /// The three spellings the tree carries reach the same variant. This is the
    /// test that replaces the reconciliation the seven tables needed: a CSVW
    /// `datatype` (bare local name), a `ShEx` `valueExpr` (full IRI) and a
    /// prefixed form are one lookup, so there is no second table to disagree with.
    #[test]
    fn every_xsd_spelling_reaches_one_lattice() {
        for (iri, prefixed, bare, want) in [
            (
                "http://www.w3.org/2001/XMLSchema#integer",
                "xsd:integer",
                "integer",
                Primitive::Integer,
            ),
            (
                "http://www.w3.org/2001/XMLSchema#dateTime",
                "xsd:dateTime",
                "dateTime",
                Primitive::DateTime,
            ),
            (
                "http://www.w3.org/2001/XMLSchema#anyURI",
                "xsd:anyURI",
                "anyURI",
                Primitive::AnyUri,
            ),
            (
                "http://www.w3.org/2001/XMLSchema#gYear",
                "xsd:gYear",
                "gYear",
                Primitive::GYear,
            ),
        ] {
            assert_eq!(Primitive::from_xsd_iri(iri), Some(want), "full IRI: {iri}");
            assert_eq!(
                Primitive::from_xsd_iri(prefixed),
                Some(want),
                "prefixed: {prefixed}"
            );
            assert_eq!(Primitive::from_xsd_iri(bare), Some(want), "bare: {bare}");
        }
    }

    /// The xsd families collapse, and the collapse is the whole point of a
    /// lattice: v0.1 does not distinguish width or signedness. Moved here from
    /// the CSVW parser's own table, which no longer exists.
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
            assert_eq!(
                Primitive::from_xsd_iri(name),
                Some(Primitive::Integer),
                "{name}"
            );
        }
        for name in ["decimal", "float", "double", "number"] {
            assert_eq!(
                Primitive::from_xsd_iri(name),
                Some(Primitive::Float),
                "{name}"
            );
        }
        for name in ["string", "normalizedString", "token", "language"] {
            assert_eq!(
                Primitive::from_xsd_iri(name),
                Some(Primitive::String),
                "{name}"
            );
        }
        assert_eq!(
            Primitive::from_xsd_iri("dateTimeStamp"),
            Some(Primitive::DateTime)
        );
    }

    /// Outside the lattice is `None`, never a silent `String`. `duration` is a
    /// real xsd type we do not carry; the caller decides what to do about it.
    #[test]
    fn an_xsd_type_outside_the_lattice_is_none() {
        assert_eq!(Primitive::from_xsd_iri("duration"), None);
        assert_eq!(Primitive::from_xsd_iri("StringWithCase"), None);
        assert_eq!(Primitive::from_xsd_iri(""), None);
    }

    /// Every variant renders to an IRI that parses back to itself — the two
    /// directions are inverses, not two tables that happen to agree today.
    #[test]
    fn to_xsd_iri_round_trips_every_variant() {
        let all = [
            Primitive::String,
            Primitive::Integer,
            Primitive::Float,
            Primitive::Bool,
            Primitive::Date,
            Primitive::DateTime,
            Primitive::Time,
            Primitive::GYear,
            Primitive::AnyUri,
        ];
        assert_eq!(all.len(), 9, "the lattice is nine wide");
        for p in all {
            assert_eq!(Primitive::from_xsd_iri(p.to_xsd_iri()), Some(p), "{p:?}");
        }
    }

    #[test]
    fn round_trips_through_json() {
        let g = worked_example();
        let json = serde_json::to_string(&g).expect("serialize");
        let back: GraphSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(g, back, "the wire contract round-trips losslessly");
    }
}
