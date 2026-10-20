//! The canonical **graph-schema** — the shared contract of fossil's universal
//! substrate.
//!
//! # The model (one idea)
//!
//! A property graph is **not a data structure — it is a SCHEMA over typed
//! relations** (the SQL:2023 / PGQ + `DuckPGQ` convergence). The data are plain
//! relations (Arrow tables); the *graph* is an interpretation that declares:
//! which relations are node types, what identifies a node (its **key** — here,
//! the subject IRI), and which predicates **reference** another node type (the
//! edges). RDF, the `GraphAr` graph (`_graphArVertexIndex`, `_graphArSrcIndex`/`_graphArDstIndex`), edge-lists,
//! traversal, DCAT are
//! then all **views or materializers** over this single schema + the relations.
//!
//! This type is that schema, made first-class and **format-neutral**: it carries
//! no `dense_id`, no Parquet detail, no manifest or xsd spelling. Those belong to
//! the *materializers*, derived from this schema on demand. Keeping the contract
//! free of any storage/format detail is what lets a new output format be "just
//! another materializer" without touching the producer or the consumer — the
//! modifiability driver (D2) of the architecture.
//!
//! # Why dependency-free
//!
//! The schema is spoken by every side at once — the producer (`fossil-exec`), each
//! materializer, and the wire/manifest. So this
//! crate depends on `serde` and, for [`problem`], the three error crates
//! (`thiserror`, `schemars`, `miette`'s trait) — nothing that parses or stores
//! anything: anyone can deserialize and interpret a
//! graph without pulling the `ShEx` descriptor or Arrow. *Decoding* a `ShEx`
//! document (which does need that machinery) lives next to the descriptor, not
//! here — the contract stays pure. What the decoder produces is [`shapes`],
//! which is in this crate precisely because it too must be speakable without
//! `ShEx`.
//!
//! # Identity & references
//!
//! Every node's key is its **subject IRI** (uniform across fossil). An edge
//! `source → destination` references node types by `label`; the relational plan
//! computes the actual IRI-valued columns, and the graph writer resolves
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

#[cfg(feature = "js")]
pub mod js;
pub mod problem;
pub mod shapes;
pub mod span;
pub mod xsd;

pub use problem::{CODES, Failure, Foreign, Problem, Related, Severity, TEMPLATES};
pub use shapes::{
    Occurs, OutputShapes, PropertyConstraint, Rejection, SUBJECT_COLUMN, Shape, language_matches,
    local_name,
};
pub use span::Span;
pub use xsd::{OutsideLattice, Xsd};

/// A whole graph's schema: its node types and edge types. The single contract
/// shared by the producer, every materializer, and the consumer.
///
/// The empty schema types nothing, so every predicate stays a vertex property:
/// what a program that names no output shape is run against.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The type's annotations, as its shape or its declaration carries them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

/// One literal property column of a node type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    /// Column / predicate local name, e.g. `"name"`.
    pub name: String,
    /// The canonical (format-neutral) datatype. A materializer derives its own
    /// spelling from this (`GraphAr` `int64`, xsd `…#integer`, …).
    pub datatype: Primitive,
    /// The full RDF predicate IRI (e.g. `https://example.org/name`) — RDF-border
    /// metadata. `None` for a non-RDF graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// The RDF term the shape declares for the values, verbatim — what a
    /// mapping states as `rr:termType` / `rr:datatype`. `None` where no shape
    /// declared one, including every property of an inferred schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<Term>,
    /// Single- vs multi-valued (drives dedup vs keep-all in a materializer).
    pub cardinality: Cardinality,
    /// The field's annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
    /// For a [`Primitive::LangString`], the basic language ranges a value's
    /// tag must match (RFC 4647 §3.3.1) — SHACL's `sh:languageIn`, a `ShEx`
    /// language value set. Empty is any tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
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
    /// The relation's annotations — those of the `Ref` field that declares it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

/// The RDF term a shape document declares for a property's values, **as the
/// document states it** — `ShEx` `IRI` / `LITERAL` / a datatype, SHACL
/// `sh:nodeKind` / `sh:datatype`. It is the RDF border's fact, and [`Primitive`]
/// is the type checker's: [`Term::primitive`] is the one reading of the first
/// as the second, and nothing reads back the other way, so `xsd:decimal` stays
/// `xsd:decimal` in a mapping however the lattice folds it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Term {
    /// An IRI.
    Iri,
    /// A literal, of the datatype exactly as declared; `None` for a literal
    /// the document did not give a datatype.
    Literal(Option<Xsd>),
}

impl Term {
    /// The lattice reading of the term: an IRI is [`Primitive::AnyUri`], a
    /// literal is its datatype read into the lattice ([`Xsd`]), and a literal
    /// with no datatype, or one outside the lattice, narrows nothing.
    #[must_use]
    pub fn primitive(&self) -> Option<Primitive> {
        match self {
            Self::Iri => Some(Primitive::AnyUri),
            Self::Literal(datatype) => datatype.as_ref().and_then(|d| Primitive::try_from(d).ok()),
        }
    }
}

/// The canonical datatype lattice — format-neutral, and **the same enum the type
/// system checks against** (`TyKind::Primitive`). It lives in this crate because
/// the schema, the checker and the descriptors all speak it, and a lattice that
/// crosses a crate boundary as a string is a lattice with no single definition.
///
/// Materializers map it to their own vocabulary (`GraphAr` `string/int64/double/…`,
/// `DataFusion` scalars, `DuckDB` column types) next to the materializer. XSD is
/// one of those vocabularies, the RDF border's, and [`Xsd`] is where the lattice
/// meets it: this enum names no datatype IRI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// `rdf:langString` — a string and its BCP 47 language tag, RDF 1.1
    /// Concepts §3.3. Named for the datatype's local name, as [`Self::AnyUri`]
    /// and [`Self::GYear`] are for theirs (`/docs/design/labels`). A property of
    /// this type is always a property table with a `lang` column, whatever its
    /// cardinality ([`OutputShapes::to_graph_schema`]).
    LangString,
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

/// The name a program reads: in a hover, a diagnostic, a completion's
/// signature. The variant's own name — `AnyUri`, not the `snake_case` the wire
/// carries.
impl std::fmt::Display for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl Primitive {
    /// The lattice, in declaration order — what a declared field's type is
    /// read against ([`Self::from_name`]).
    pub const ALL: [Self; 10] = [
        Self::String,
        Self::Integer,
        Self::Float,
        Self::Bool,
        Self::Date,
        Self::DateTime,
        Self::Time,
        Self::GYear,
        Self::AnyUri,
        Self::LangString,
    ];

    /// The primitive a program writes as `name` — `String`, `GYear`, `AnyUri`:
    /// the name [`Display`](std::fmt::Display) prints, so a type is read in the
    /// words the checker prints it in.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.to_string() == name)
    }
}

/// `rdf:langString`, the datatype of every language-tagged string (RDF 1.1
/// Concepts §3.3) — what [`Primitive::LangString`] is written as.
pub const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// `rdfs:label` — the one predicate a label has in a graph, whichever document
/// gave it (`/docs/design/labels`).
pub const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

/// `rdfs:comment` — the one predicate a description has in a graph.
pub const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";

/// A `ShEx` 2.1 `Annotation` (§5.9): a predicate and an RDF object, about a
/// type, a field or a relation — never about the data. `ShExJ`'s shape, with one
/// word changed: a literal's datatype IRI is `datatype`, the word a graph
/// column already uses for it, where `ShExJ` writes `type`
/// (`/docs/design/labels`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Annotation {
    /// The predicate, a whole IRI.
    pub predicate: String,
    /// `ShExJ`'s `objectValue`: an IRI as a bare string, or a literal.
    pub object: AnnotationObject,
}

/// An annotation's object — `ShExJ`'s `objectValue`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum AnnotationObject {
    /// An IRI, written as a bare string.
    Iri(String),
    /// A literal: `ShExJ`'s `ObjectLiteral`.
    Literal(ObjectLiteral),
}

/// A literal object: its lexical form, and a language tag or a datatype IRI.
/// A plain string carries neither.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ObjectLiteral {
    /// The lexical form.
    pub value: String,
    /// The BCP 47 tag of a language-tagged string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The datatype IRI of a typed literal; `ShExJ`'s `type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datatype: Option<String>,
}

impl Annotation {
    /// A string-valued annotation, language-tagged when `language` is given —
    /// the two forms a program's `@annotation` writes.
    #[must_use]
    pub fn literal(predicate: &str, value: &str, language: Option<&str>) -> Self {
        Self {
            predicate: predicate.to_string(),
            object: AnnotationObject::Literal(ObjectLiteral {
                value: value.to_string(),
                language: language.map(str::to_string),
                datatype: None,
            }),
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
                        term: None,
                        cardinality: Cardinality::Single,
                        annotations: Vec::new(),
                        languages: Vec::new(),
                    }],
                    annotations: Vec::new(),
                },
                NodeType {
                    label: "Order".into(),
                    iri: Some("https://example.org/Order".into()),
                    properties: vec![Property {
                        name: "total".into(),
                        datatype: Primitive::Integer,
                        iri: Some("https://example.org/total".into()),
                        term: None,
                        cardinality: Cardinality::Single,
                        annotations: Vec::new(),
                        languages: Vec::new(),
                    }],
                    annotations: Vec::new(),
                },
            ],
            edges: vec![EdgeType {
                label: "placedBy".into(),
                iri: Some("https://example.org/placedBy".into()),
                source: "Order".into(),
                destination: "Person".into(),
                cardinality: Cardinality::Single,
                annotations: Vec::new(),
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

    /// A program names a primitive as the checker prints it, and every variant
    /// reads back from that name — `ALL` is the whole lattice, not a list that
    /// happens to agree with the enum today.
    #[test]
    fn from_name_reads_back_every_printed_name() {
        for p in Primitive::ALL {
            assert_eq!(Primitive::from_name(&p.to_string()), Some(p), "{p:?}");
        }
        assert_eq!(
            Primitive::from_name("string"),
            None,
            "the name is case-sensitive"
        );
        assert_eq!(Primitive::from_name("Option"), None);
    }

    #[test]
    fn round_trips_through_json() {
        let g = worked_example();
        let json = serde_json::to_string(&g).expect("serialize");
        let back: GraphSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(g, back, "the wire contract round-trips losslessly");
    }
}
