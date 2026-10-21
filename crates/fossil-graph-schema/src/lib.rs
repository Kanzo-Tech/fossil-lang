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
pub use xsd::{NotALiteral, OutsideLattice, Xsd};

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
    /// For a [`Primitive::LANG_STRING`], the basic language ranges a value's
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
    /// The term a primitive is written as: a [`Primitive::Uri`] is an IRI, and
    /// every other primitive a literal of its canonical datatype.
    #[must_use]
    pub fn of(primitive: Primitive) -> Self {
        Xsd::try_from(primitive).map_or(Self::Iri, |xsd| Self::Literal(Some(xsd)))
    }

    /// The lattice reading of the term: an IRI is [`Primitive::Uri`], a
    /// literal is its datatype read into the lattice ([`Xsd`]), and a literal
    /// with no datatype, or one outside the lattice, narrows nothing.
    #[must_use]
    pub fn primitive(&self) -> Option<Primitive> {
        match self {
            Self::Iri => Some(Primitive::Uri),
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
/// meets it: this enum names no datatype IRI, and no variant exists for RDF's
/// sake. A text with a language is a [`String`](Self::String) with
/// [`Lang::Tagged`]; every point in time is a [`Time`](Self::Time) of some
/// [`Precision`]; an identifier is a [`Uri`](Self::Uri), whichever format spells
/// it as an IRI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "WirePrimitive", from = "WirePrimitive")]
pub enum Primitive {
    /// A text, with a language or without one. The two are disjoint, as
    /// `xsd:string` and `rdf:langString` are (RDF 1.1 Concepts §3.3: a tag is
    /// never empty), so `String` is no `String<Lang>` and a text gains its tag
    /// only by a call that writes one (`str.lang`). A tagged property is
    /// always a property table with a `lang` column, whatever its cardinality
    /// ([`OutputShapes::to_graph_schema`]).
    String(Lang),
    Integer,
    Float,
    Bool,
    /// A point in time, at a precision. No precision is a subtype of
    /// another: a year is not a date.
    Time(Precision),
    /// An identifier. A format that has nodes writes it as one: RDF as an IRI
    /// node, never an `xsd:anyURI` literal, which is a [`Self::String`] whose
    /// shape says so.
    Uri,
}

/// Whether a [`Primitive::String`] carries a language tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    /// `String`.
    Untagged,
    /// `String<Lang>`: a text and its BCP 47 language tag.
    Tagged,
}

/// The precision of a [`Primitive::Time`]: what one value of it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    /// A calendar year, `Time<Year>`.
    Year,
    /// A calendar day, `Time<Date>`.
    Date,
    /// A day and a time of day, `Time<DateTime>`.
    DateTime,
    /// A time of day on no day in particular, `Time<Hour>`.
    Hour,
}

impl Precision {
    /// Every precision, coarsest first.
    pub const ALL: [Self; 4] = [Self::Year, Self::Date, Self::DateTime, Self::Hour];
}

/// The wire's spelling of a [`Primitive`]: one flat name, so a reader keys a
/// table on it (`@fossil-lang/types`' `Primitive`). It is the program's
/// spelling in `snake_case`, the parameter after the family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "Primitive")]
enum WirePrimitive {
    String,
    StringLang,
    Integer,
    Float,
    Bool,
    TimeYear,
    TimeDate,
    TimeDateTime,
    TimeHour,
    Uri,
}

/// The wire's schema is `WirePrimitive`'s, under this type's name and
/// documentation.
impl schemars::JsonSchema for Primitive {
    fn schema_name() -> String {
        "Primitive".into()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        WirePrimitive::json_schema(generator)
    }
}

impl From<Primitive> for WirePrimitive {
    fn from(p: Primitive) -> Self {
        match p {
            Primitive::String(Lang::Untagged) => Self::String,
            Primitive::String(Lang::Tagged) => Self::StringLang,
            Primitive::Integer => Self::Integer,
            Primitive::Float => Self::Float,
            Primitive::Bool => Self::Bool,
            Primitive::Time(Precision::Year) => Self::TimeYear,
            Primitive::Time(Precision::Date) => Self::TimeDate,
            Primitive::Time(Precision::DateTime) => Self::TimeDateTime,
            Primitive::Time(Precision::Hour) => Self::TimeHour,
            Primitive::Uri => Self::Uri,
        }
    }
}

impl From<WirePrimitive> for Primitive {
    fn from(w: WirePrimitive) -> Self {
        match w {
            WirePrimitive::String => Self::STRING,
            WirePrimitive::StringLang => Self::LANG_STRING,
            WirePrimitive::Integer => Self::Integer,
            WirePrimitive::Float => Self::Float,
            WirePrimitive::Bool => Self::Bool,
            WirePrimitive::TimeYear => Self::Time(Precision::Year),
            WirePrimitive::TimeDate => Self::Time(Precision::Date),
            WirePrimitive::TimeDateTime => Self::Time(Precision::DateTime),
            WirePrimitive::TimeHour => Self::Time(Precision::Hour),
            WirePrimitive::Uri => Self::Uri,
        }
    }
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

/// How a constraint is enforced — SHACL's three severities (SHACL §2.1.5,
/// `sh:Violation`, `sh:Warning`, `sh:Info`), written as the argument of
/// `@assert(…, severity = warning)` and as a field's `@severity(info)`.
///
/// A violation refuses: an error when the checker can decide it, a refused run
/// when only the data can. A warning or an info is reported and the graph is
/// still written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintSeverity {
    /// The default: a constraint nothing qualifies is a violation.
    #[default]
    Violation,
    Warning,
    Info,
}

impl ConstraintSeverity {
    /// Every severity, strongest first.
    pub const ALL: [Self; 3] = [Self::Violation, Self::Warning, Self::Info];

    /// The name a program writes — `violation`, `warning`, `info`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Violation => "violation",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    /// The severity a program writes as `name`, if it is one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The severity of the diagnostic a constraint of this severity raises
    /// when the checker can decide it.
    #[must_use]
    pub const fn diagnostic(self) -> Severity {
        match self {
            Self::Violation => Severity::Error,
            Self::Warning => Severity::Warning,
            Self::Info => Severity::Info,
        }
    }
}

/// The name a program reads: in a hover, a diagnostic, a completion's
/// signature — `Time<Year>`, `String<Lang>`, the spelling a declared field
/// writes ([`Primitive::from_name`]).
impl std::fmt::Display for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(Lang::Untagged) => f.write_str("String"),
            Self::String(Lang::Tagged) => f.write_str("String<Lang>"),
            Self::Time(p) => write!(f, "Time<{p:?}>"),
            other => std::fmt::Debug::fmt(other, f),
        }
    }
}

impl Primitive {
    /// A text with no language.
    pub const STRING: Self = Self::String(Lang::Untagged);
    /// A text and its language tag.
    pub const LANG_STRING: Self = Self::String(Lang::Tagged);
    /// A calendar year.
    pub const YEAR: Self = Self::Time(Precision::Year);
    /// A calendar day.
    pub const DATE: Self = Self::Time(Precision::Date);
    /// A day and a time of day.
    pub const DATE_TIME: Self = Self::Time(Precision::DateTime);
    /// A time of day.
    pub const HOUR: Self = Self::Time(Precision::Hour);

    /// The lattice, in declaration order — what a declared field's type is
    /// read against ([`Self::from_name`]).
    pub const ALL: [Self; 10] = [
        Self::STRING,
        Self::LANG_STRING,
        Self::Integer,
        Self::Float,
        Self::Bool,
        Self::Time(Precision::Year),
        Self::Time(Precision::Date),
        Self::Time(Precision::DateTime),
        Self::Time(Precision::Hour),
        Self::Uri,
    ];

    /// The primitive a program writes as `name` — `String`, `Time<Year>`,
    /// `Uri`: the name [`Display`](std::fmt::Display) prints, so a type is read
    /// in the words the checker prints it in.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.to_string() == name)
    }

    /// The family a program names before any parameter: `String` for both
    /// texts, `Time` for every precision.
    #[must_use]
    pub const fn family(self) -> &'static str {
        match self {
            Self::String(_) => "String",
            Self::Integer => "Integer",
            Self::Float => "Float",
            Self::Bool => "Bool",
            Self::Time(_) => "Time",
            Self::Uri => "Uri",
        }
    }

    /// Whether this is a text with a language tag.
    #[must_use]
    pub const fn is_tagged(self) -> bool {
        matches!(self, Self::String(Lang::Tagged))
    }
}

/// `rdf:langString`, the datatype of every language-tagged string (RDF 1.1
/// Concepts §3.3) — what [`Primitive::LANG_STRING`] is written as.
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
                        datatype: Primitive::STRING,
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
        assert_eq!(Primitive::from_name("Time"), None, "a family is no type");
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
