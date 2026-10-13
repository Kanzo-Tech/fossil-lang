//! What a **shape document** says, format-neutral — the vocabulary the middle of
//! the compiler reads instead of a schema language.
//!
//! # Why this exists
//!
//! [`GraphSchema`] is the *output* contract: node types, edge types, columns. It
//! is what a materializer consumes. But the checker needs something else — the
//! per-shape constraint table it asks "given target shape `ex:Person`, what is
//! the expected type and cardinality of predicate `p`?" — and a table whose
//! fields were `rudof_iri::IriS` and `shex_ast::ShapeExpr` would make reading
//! it mean linking `ShEx`.
//!
//! This module is that table with no type from any schema language in it. A
//! `ShEx` decoder lowers into it; a SHACL decoder lowers into it; the compiler
//! reads it and never learns which document it came from. The middle of the
//! compiler is handed a value, not a parser.
//!
//! # The two contracts, and which is which
//!
//! - [`OutputShapes`] is the **decoded document**: shapes in declaration order,
//!   each with its predicates, plus what the decoder could not lower.
//! - [`GraphSchema`] is the **output model**: what gets written.
//!
//! [`OutputShapes::to_graph_schema`] is the one function between them, and it is
//! deliberately the only one — every other consumer reads one side or the other.
//!
//! # Structural equality is load-bearing
//!
//! Salsa memoizes an `OutputShapes` (`fossil_hir::shape_documents::shape_document`),
//! and salsa decides "did this change?" with `PartialEq` — `update_fallback`
//! in `salsa/src/update.rs` compares the old and new values and skips the
//! invalidation entirely when they are equal. So every type here derives
//! `PartialEq + Eq`, and none of them may ever grow a field that compares by
//! address.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    Annotation, Cardinality, EdgeType, GraphSchema, NodeType, Primitive, Property, Span, Term,
};

/// The local name of an IRI — the substring after the last `#` or `/`.
///
/// This is the *user-visible* rule, not just an internal convenience: a
/// property key in a fossil program is a bare name, and the bare name is the
/// last segment of the predicate IRI that the shape declares. The name depends
/// on the LABEL ALONE — never on the cardinality, the value type or the
/// position — and there is no plural/singular heuristic: a scheme that lets a
/// small change in the document rename a member is how a minor version bump
/// breaks a program. A collision is an error naming both IRIs, never a numeric
/// suffix, and the repair is to declare the type in the program, with the IRIs
/// it needs, rather than to edit the document.
///
/// It lives here because this crate is the one both sides of that rule share:
/// the decoder that produces the IRIs and the schema that carries the names.
/// Every reader that names something after an IRI — the decoders, the checker,
/// the type display — calls this one, so no two of them can disagree about
/// where a name starts.
#[must_use]
pub fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// What a shape document says — format-neutral. `ShEx` and SHACL both lower into
/// this.
///
/// Construct with [`OutputShapes::new`]; the index is derived, never supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "OutputShapesRepr", into = "OutputShapesRepr")]
pub struct OutputShapes {
    /// In the order the document declares them. See [`OutputShapes::shapes`].
    shapes: Vec<Shape>,
    /// Shape IRI → index into `shapes`. Lookup only; never iterated, so its
    /// hash order can never reach an output.
    index: HashMap<String, usize>,
    rejections: Vec<Rejection>,
}

/// The wire form of [`OutputShapes`]: the index is derived, so it is not
/// serialized — deserializing runs [`OutputShapes::new`] again, which means a
/// round-trip re-applies the first-declaration-wins rule rather than trusting a
/// possibly-stale index someone else wrote.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutputShapesRepr {
    shapes: Vec<Shape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rejections: Vec<Rejection>,
}

/// By what it says — the shapes and the rejections. The index is derived from
/// the shapes, so two values equal by [`PartialEq`] hash alike.
impl std::hash::Hash for OutputShapes {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.shapes.hash(state);
        self.rejections.hash(state);
    }
}

impl From<OutputShapesRepr> for OutputShapes {
    fn from(repr: OutputShapesRepr) -> Self {
        Self::new(repr.shapes, repr.rejections)
    }
}

impl From<OutputShapes> for OutputShapesRepr {
    fn from(shapes: OutputShapes) -> Self {
        Self {
            shapes: shapes.shapes,
            rejections: shapes.rejections,
        }
    }
}

/// The column a pivoted RDF row carries its entity IRI in — the row a
/// shape types and the RDF reader writes.
pub const SUBJECT_COLUMN: &str = "subject";

/// One shape: a set of predicates a subject of this type may carry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Shape {
    /// The shape's IRI, fully resolved — the key the output model is addressed
    /// by (`NodeType::iri`).
    pub iri: String,
    /// The predicates, in the order the document lists them.
    pub properties: Vec<PropertyConstraint>,
    /// Where the DOCUMENT declares this shape — its label — as a byte range
    /// into that document's text. `None` on the same terms as
    /// [`PropertyConstraint::span`].
    pub span: Option<Span>,
    /// The shape's annotations — ShEx's on the shape, SHACL's non-validating
    /// triples on the node shape, a declaration's `@annotation` above `type`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

// There is no `closed` here — `ShEx` `CLOSED` / SHACL `sh:closed` — because
// reading it would change nothing (`/docs/design/discarded`). A property key is
// a BARE NAME resolved against this table
// (`Checker::resolve_predicate`), so a key no predicate here
// declares is already an error, with a did-you-mean over the ones that do —
// unconditionally, for every shape. Openness has no way to be expressed from
// the writing side, which is the only side a mapping has. A flag that can only
// ever select the behaviour that already happens is not a flag.

/// One predicate of a [`Shape`] — the format-neutral collapse of a `ShEx`
/// `TripleConstraint` / a SHACL property shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PropertyConstraint {
    /// The bare name a program writes for this predicate — a body's property
    /// key, and the column the corpus names.
    ///
    /// A decoder writes [`local_name`] of [`Self::predicate`]: a document says
    /// nothing else a name could come from. A declared type writes the field's
    /// own name, which is the whole point of declaring one — two predicates
    /// whose last segments coincide are two fields with two names
    /// (`/docs/design/declared`). It is carried rather than derived because
    /// only the second source knows it.
    pub name: String,
    /// The full predicate IRI, fully resolved.
    pub predicate: String,
    /// The RDF term the document declares for the value, verbatim, or `None`
    /// when it declares none. A decoder writes [`Term::Iri`] for an IRI-valued
    /// node that is *not* a reference to another shape (`ShEx` `IRI`,
    /// `sh:nodeKind sh:IRI`) — an opaque IRI column, not an edge — and
    /// [`Term::Literal`] for a literal, with the datatype IRI as written.
    /// [`Self::datatype`] is what the checker reads of it.
    pub term: Option<Term>,
    /// Destination shape IRIs. Empty means a literal (or opaque-IRI) property
    /// that stays a column on the node type.
    ///
    /// **More than one is legal**: a value disjunction (`@<A> OR @<B>`,
    /// `sh:or`) emits **one edge type per destination**, sharing the predicate
    /// label — the reference RDF→property-graph model. Object IRIs partition
    /// cleanly across the destinations at materialisation.
    pub targets: Vec<String>,
    /// How many values the property may carry.
    pub occurs: Occurs,
    /// Where the DOCUMENT declares this predicate, as a byte range into that
    /// document's text.
    ///
    /// It is what lets a type error underline both halves of what it is
    /// saying: the program's line, and `shop:total xsd:float` in `shape.shex`.
    /// A checker knows the constraint was violated and, without this, has no
    /// way to point at the sentence it was violating — so the shape's answer
    /// travelled as prose in the message, or not at all.
    ///
    /// `None` is the honest default and there are three ways to get it: a
    /// `ShExJ` document (offsets in JSON are not what a reader is looking at),
    /// a SHACL one (a separate decoder over Turtle, with no lookup of its own
    /// yet), and a compact document where the lookup did not find the
    /// predicate. A consumer drops one label; nothing else changes. See
    /// `fossil_hir::shex::spans`, which is a lookup over text and explains at length
    /// why that is not a second parser.
    pub span: Option<Span>,
    /// The predicate's annotations in this shape. A label is one predicate
    /// whichever document gave it: a decoder writes SHACL's `sh:name` and
    /// `sh:description` as [`crate::RDFS_LABEL`] and [`crate::RDFS_COMMENT`]
    /// (`/docs/design/labels`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
    /// The basic language ranges a language-tagged value's tag must match —
    /// SHACL's `sh:languageIn`, ShEx's language value set. Empty is any tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
}

/// Does `tag` match one of `ranges` by RFC 4647's basic filtering (§3.3.1)?
/// An empty list admits every tag, and `*` matches any. Tags compare without
/// case (RFC 5646 §2.1.1). It is the one reading of `sh:languageIn` and of a
/// ShEx language stem, which are the same thing.
#[must_use]
pub fn language_matches(tag: &str, ranges: &[String]) -> bool {
    ranges.is_empty()
        || ranges.iter().any(|range| {
            let (tag, range) = (tag.to_ascii_lowercase(), range.to_ascii_lowercase());
            range == "*"
                || tag == range
                || tag
                    .strip_prefix(range.as_str())
                    .is_some_and(|rest| rest.starts_with('-'))
        })
}

impl PropertyConstraint {
    /// The value's datatype, or `None` when **the document did not narrow the
    /// value type**.
    ///
    /// It is the [`term`](Self::term)'s [`Term::primitive`]: `Some(p)` for a
    /// literal narrowed to an XSD datatype in the lattice,
    /// `Some(Primitive::AnyUri)` for an IRI, and `None` for everything else,
    /// including an XSD datatype **outside** the lattice.
    ///
    /// `None` becomes [`Primitive::String`] in
    /// [`to_graph_schema`](OutputShapes::to_graph_schema) — the permissive
    /// walking-skeleton column. The `Option` exists so that a consumer that
    /// wants to *diagnose* an un-narrowed property can still tell the two apart,
    /// which a bare `Primitive::String` cannot.
    ///
    /// # The checker agrees
    ///
    /// `None` means "the document did not narrow the value type" — that is the
    /// contract, and the type checker reads it that way: `check.rs`'s
    /// `compatible` short-circuits on
    /// `expected.is_none_or(|e| subtypes(db, actual, e))`, so an absent
    /// expectation accepts anything.
    ///
    /// Keep the two in step. `Some` narrows and `None` does not. A default on
    /// the `fossil-hir` side — `Iri`, say, the NARROWEST type in the lattice —
    /// would make a constraint the document declined to narrow reject a String,
    /// and silently make this method mean the opposite of what it says.
    #[must_use]
    pub fn datatype(&self) -> Option<Primitive> {
        self.term.as_ref().and_then(Term::primitive)
    }
}

/// How many values a property may carry — the rich form. [`Cardinality`] is its
/// collapse, and [`Occurs::collapse`] is the only way to get there.
///
/// The rich form is a `(min, max)` pair rather than an enum of named cases
/// because named cases can disagree with themselves: an enum with both
/// `Exact(n)` and `Range { min: n, max: Some(n) }` has two spellings of one
/// cardinality, and nothing stops its `is_single_valued` answering differently
/// for each. A pair cannot hold two spellings of one fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Occurs {
    /// Minimum number of values. `0` = optional.
    pub min: u32,
    /// Maximum number of values; `None` = unbounded.
    pub max: Option<u32>,
}

impl Occurs {
    /// Exactly one value — the default a document that says nothing means.
    pub const ONE: Self = Self {
        min: 1,
        max: Some(1),
    };

    /// `true` iff at most one value is allowed.
    ///
    /// Single-valued constraints collapse duplicate subjects; multi-valued ones
    /// keep every value (and, for the RDF provider, become a `LIST` column).
    /// The single source of this truth, shared by the input pivot and the output
    /// decomposition.
    #[must_use]
    pub const fn is_single_valued(self) -> bool {
        matches!(self.max, Some(m) if m <= 1)
    }

    /// Does this constraint require at least one value? Drives the
    /// `Optional<τ>` vs `τ` decision in the checker.
    #[must_use]
    pub const fn demands_one_or_more(self) -> bool {
        self.min >= 1
    }

    /// Collapse to the only distinction a materializer needs.
    #[must_use]
    pub const fn collapse(self) -> Cardinality {
        if self.is_single_valued() {
            Cardinality::Single
        } else {
            Cardinality::Multi
        }
    }
}

/// What the decoder could not lower. Format-neutral: no `ShEx` AST node survives
/// here, which is the point — a diagnostic renders from strings, and the middle
/// of the compiler never has to name the language the document was written in.
///
/// [`Rejection::Disjunction`] carries the predicate IRIs of each branch, not
/// the document's `OneOf` node: enough to *name* the split
/// in a diagnostic, not enough to re-emit the document's own syntax. Re-emitting
/// syntax is the decoder's job, and it is the decoder that knows the syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Rejection {
    /// A disjunction the compiler does not support. `disjuncts` carries the
    /// predicate IRIs of each branch, in order — `disjuncts.len()` is the number
    /// of branches, and each inner `Vec` is what that branch constrains.
    Disjunction {
        shape_iri: String,
        disjuncts: Vec<Vec<String>>,
    },
    /// Only acyclic shape graphs are supported. `path` is
    /// the chain of labels visited along the cycle.
    CyclicRef { path: Vec<String> },
    /// A reference to a label the document never declares.
    UnresolvedRef { label: String, in_shape: String },
    /// The document did not parse, or its top-level structure is unsupported.
    Malformed(String),
}

impl OutputShapes {
    /// Build from shapes in **declaration order** plus whatever the decoder
    /// rejected, deriving the lookup index.
    ///
    /// **A repeated IRI keeps its FIRST declaration and its first slot.** The
    /// alternative — last one wins — is what a `HashMap::insert` does for free,
    /// and either rule is arbitrary, but only one of them leaves the order
    /// alone, and the order is what a diagnostic lists the shapes in. So the
    /// duplicate is dropped entirely, not merged and not appended.
    #[must_use]
    pub fn new(shapes: Vec<Shape>, rejections: Vec<Rejection>) -> Self {
        let mut kept: Vec<Shape> = Vec::with_capacity(shapes.len());
        let mut index: HashMap<String, usize> = HashMap::with_capacity(shapes.len());
        for shape in shapes {
            if index.contains_key(&shape.iri) {
                continue;
            }
            index.insert(shape.iri.clone(), kept.len());
            kept.push(shape);
        }
        Self {
            shapes: kept,
            index,
            rejections,
        }
    }

    /// A document that produced nothing but a failure — the shape a decoder
    /// returns when the text did not parse at all.
    #[must_use]
    pub fn rejected(rejection: Rejection) -> Self {
        Self::new(Vec::new(), vec![rejection])
    }

    /// Every shape, **in declaration order**. An import binds by local name and
    /// not by position, but the order still reaches a person: an import naming
    /// no shape lists the ones the document declares, and two shapes sharing a
    /// local name are named first-then-second. A `HashMap` lived where this
    /// `Vec` is until `crates/fossil-hir/examples/shex_declaration_order.rs`
    /// measured what it cost — six parses of one document handed back six
    /// different orders, because Rust seeds its hasher per process. The order
    /// is the document's; it is not ours to choose.
    pub fn shapes(&self) -> impl Iterator<Item = &Shape> {
        self.shapes.iter()
    }

    /// Look up a shape by its IRI.
    #[must_use]
    pub fn lookup(&self, iri: &str) -> Option<&Shape> {
        self.index.get(iri).map(|&i| &self.shapes[i])
    }

    /// What the decoder could not lower. The checker turns these into
    /// diagnostics; an empty slice means the document lowered whole.
    #[must_use]
    pub fn rejections(&self) -> &[Rejection] {
        &self.rejections
    }

    /// Lower into the canonical, format-neutral [`GraphSchema`] — the single
    /// output model the MIR and the executor consume.
    ///
    /// Each shape becomes a [`NodeType`] keyed by its IRI; each property becomes
    /// either an [`EdgeType`] (it has targets) or a literal/IRI [`Property`] (it
    /// does not). A value disjunction emits **one edge per destination**,
    /// sharing the predicate label.
    ///
    /// [`Rejection`]s are not consulted: this is the *lowering* of what was
    /// understood, and a caller that wants to refuse a partially-understood
    /// document checks [`Self::rejections`] first.
    ///
    /// A node's label is [`local_name`] of its IRI; a property's or an edge's is
    /// [`PropertyConstraint::name`], the key a program writes, and two that
    /// coincide are a collision the checker refuses rather than a name this
    /// function chooses.
    #[must_use]
    pub fn to_graph_schema(&self) -> GraphSchema {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for shape in self.shapes() {
            let label = local_name(&shape.iri).to_string();
            let mut properties = Vec::new();
            for c in &shape.properties {
                let name = c.name.clone();
                let cardinality = c.occurs.collapse();
                if c.targets.is_empty() {
                    // An un-narrowed value is a string column: the permissive
                    // walking-skeleton default the `ShEx` lowering already
                    // applied to every `valueExpr` it could not read.
                    let datatype = c.datatype().unwrap_or(Primitive::String);
                    properties.push(Property {
                        name,
                        datatype,
                        iri: Some(c.predicate.clone()),
                        term: c.term.clone(),
                        // A language-tagged value is a property table whatever
                        // its count: that is where its tag has a column with a
                        // fixed name (`/docs/format`).
                        cardinality: if datatype == Primitive::LangString {
                            Cardinality::Multi
                        } else {
                            cardinality
                        },
                        annotations: c.annotations.clone(),
                        languages: c.languages.clone(),
                    });
                } else {
                    for t in &c.targets {
                        edges.push(EdgeType {
                            label: name.clone(),
                            iri: Some(c.predicate.clone()),
                            source: label.clone(),
                            destination: local_name(t).to_string(),
                            cardinality,
                            annotations: c.annotations.clone(),
                        });
                    }
                }
            }
            nodes.push(NodeType {
                label,
                iri: Some(shape.iri.clone()),
                properties,
                annotations: shape.annotations.clone(),
            });
        }
        GraphSchema { nodes, edges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(predicate: &str, datatype: Option<Primitive>, occurs: Occurs) -> PropertyConstraint {
        PropertyConstraint {
            name: local_name(predicate).into(),
            predicate: predicate.into(),
            term: datatype.map(|p| Term::Literal(Some(p.to_xsd_iri().into()))),
            targets: Vec::new(),
            occurs,
            span: None,
            annotations: Vec::new(),
            languages: Vec::new(),
        }
    }

    fn edge(predicate: &str, targets: &[&str], occurs: Occurs) -> PropertyConstraint {
        PropertyConstraint {
            name: local_name(predicate).into(),
            predicate: predicate.into(),
            term: None,
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            occurs,
            span: None,
            annotations: Vec::new(),
            languages: Vec::new(),
        }
    }

    fn shape(iri: &str, properties: Vec<PropertyConstraint>) -> Shape {
        Shape {
            iri: iri.into(),
            properties,
            span: None,
            annotations: Vec::new(),
        }
    }

    // -- declaration order + first declaration wins ------------------------

    /// The order is the document's. Nothing sorts, nothing hashes on the way
    /// out.
    #[test]
    fn shapes_iterate_in_declaration_order() {
        let doc = OutputShapes::new(
            vec![
                shape("https://example.org/Zeta", vec![]),
                shape("https://example.org/Alpha", vec![]),
                shape("https://example.org/Mu", vec![]),
            ],
            vec![],
        );
        let order: Vec<&str> = doc.shapes().map(|s| s.iri.as_str()).collect();
        assert_eq!(
            order,
            [
                "https://example.org/Zeta",
                "https://example.org/Alpha",
                "https://example.org/Mu"
            ],
            "declaration order, not alphabetical and not hash order"
        );
    }

    /// A repeated IRI keeps its FIRST declaration and its first slot. Both
    /// halves matter: the surviving shape is the first one's *content*, and it
    /// is still at index 0 — a last-one-wins rule would have moved `Beta` up.
    #[test]
    fn a_repeated_iri_keeps_its_first_declaration_and_its_first_slot() {
        let doc = OutputShapes::new(
            vec![
                shape(
                    "https://example.org/Person",
                    vec![prop(
                        "https://example.org/first",
                        Some(Primitive::String),
                        Occurs::ONE,
                    )],
                ),
                shape("https://example.org/Beta", vec![]),
                shape(
                    "https://example.org/Person",
                    vec![prop(
                        "https://example.org/second",
                        Some(Primitive::String),
                        Occurs::ONE,
                    )],
                ),
            ],
            vec![],
        );

        let order: Vec<&str> = doc.shapes().map(|s| s.iri.as_str()).collect();
        assert_eq!(
            order,
            ["https://example.org/Person", "https://example.org/Beta"],
            "the duplicate is dropped, not appended and not merged"
        );
        let person = doc.lookup("https://example.org/Person").expect("Person");
        assert_eq!(
            person.properties[0].predicate, "https://example.org/first",
            "the FIRST declaration survives"
        );
    }

    #[test]
    fn lookup_misses_return_none() {
        let doc = OutputShapes::new(vec![shape("https://example.org/A", vec![])], vec![]);
        assert!(doc.lookup("https://example.org/A").is_some());
        assert!(doc.lookup("A").is_none(), "the key is the full IRI");
        assert!(doc.rejections().is_empty());
    }

    #[test]
    fn a_rejected_document_carries_its_failure_and_no_shapes() {
        let doc = OutputShapes::rejected(Rejection::Malformed("expected `{`".into()));
        assert_eq!(doc.shapes().count(), 0);
        assert_eq!(
            doc.rejections(),
            [Rejection::Malformed("expected `{`".into())]
        );
    }

    // -- Occurs::collapse over every cardinality a document can state -------

    /// Each row is a cardinality by the name a reader knows it under — exactly
    /// `n`, `?`, `+`, `*` and a range — its `(min, max)`, and whether it is
    /// single-valued. Exactly `n` for `n > 1` is NOT single-valued: three
    /// values per subject are many values, whatever the spelling.
    #[test]
    fn collapse_pins_every_cardinality_a_document_can_state() {
        let cases: &[(&str, Occurs, bool)] = &[
            // Exact(1) — the ShEx default when nothing is written.
            (
                "Exact(1)",
                Occurs {
                    min: 1,
                    max: Some(1),
                },
                true,
            ),
            // Exact(0) — degenerate, and single-valued.
            (
                "Exact(0)",
                Occurs {
                    min: 0,
                    max: Some(0),
                },
                true,
            ),
            // Exact(3) — many values, so not single-valued.
            (
                "Exact(3)",
                Occurs {
                    min: 3,
                    max: Some(3),
                },
                false,
            ),
            (
                "ZeroOrOne",
                Occurs {
                    min: 0,
                    max: Some(1),
                },
                true,
            ),
            ("OneOrMore", Occurs { min: 1, max: None }, false),
            ("ZeroOrMore", Occurs { min: 0, max: None }, false),
            (
                "Range{0,Some(1)}",
                Occurs {
                    min: 0,
                    max: Some(1),
                },
                true,
            ),
            (
                "Range{1,Some(1)}",
                Occurs {
                    min: 1,
                    max: Some(1),
                },
                true,
            ),
            (
                "Range{2,Some(5)}",
                Occurs {
                    min: 2,
                    max: Some(5),
                },
                false,
            ),
            ("Range{3,None}", Occurs { min: 3, max: None }, false),
        ];
        for (name, occurs, single) in cases {
            assert_eq!(
                occurs.is_single_valued(),
                *single,
                "is_single_valued {name}"
            );
            assert_eq!(
                occurs.collapse(),
                if *single {
                    Cardinality::Single
                } else {
                    Cardinality::Multi
                },
                "collapse {name}"
            );
        }
    }

    /// `demands_one_or_more` is `min >= 1` for every cardinality.
    #[test]
    fn demands_one_or_more_is_min_at_least_one() {
        let cases: &[(&str, Occurs, bool)] = &[
            (
                "Exact(1)",
                Occurs {
                    min: 1,
                    max: Some(1),
                },
                true,
            ),
            (
                "Exact(0)",
                Occurs {
                    min: 0,
                    max: Some(0),
                },
                false,
            ),
            (
                "Exact(3)",
                Occurs {
                    min: 3,
                    max: Some(3),
                },
                true,
            ),
            (
                "ZeroOrOne",
                Occurs {
                    min: 0,
                    max: Some(1),
                },
                false,
            ),
            ("OneOrMore", Occurs { min: 1, max: None }, true),
            ("ZeroOrMore", Occurs { min: 0, max: None }, false),
            (
                "Range{2,Some(5)}",
                Occurs {
                    min: 2,
                    max: Some(5),
                },
                true,
            ),
        ];
        for (name, occurs, want) in cases {
            assert_eq!(occurs.demands_one_or_more(), *want, "{name}");
        }
    }

    #[test]
    fn occurs_one_is_exactly_one() {
        assert_eq!(
            Occurs::ONE,
            Occurs {
                min: 1,
                max: Some(1)
            }
        );
        assert!(Occurs::ONE.is_single_valued());
        assert!(Occurs::ONE.demands_one_or_more());
    }

    // -- to_graph_schema ---------------------------------------------------

    /// The four cases the lowering distinguishes, in one document: a narrowed
    /// literal, an un-narrowed literal, a single-target edge and a two-target
    /// edge. The two-target one is the case that emits two `EdgeType`s from one
    /// property.
    #[test]
    fn to_graph_schema_lowers_literals_and_edges() {
        let doc = OutputShapes::new(
            vec![
                shape(
                    "https://example.org/Order",
                    vec![
                        prop(
                            "https://example.org/total",
                            Some(Primitive::Integer),
                            Occurs::ONE,
                        ),
                        prop("https://example.org/note", None, Occurs::ONE),
                        edge(
                            "https://example.org/placedBy",
                            &["https://example.org/Person"],
                            Occurs::ONE,
                        ),
                        edge(
                            "https://example.org/paidWith",
                            &["https://example.org/Card", "https://example.org/Cash"],
                            Occurs { min: 0, max: None },
                        ),
                    ],
                ),
                shape("https://example.org/Person", vec![]),
            ],
            vec![],
        );

        let g = doc.to_graph_schema();

        assert_eq!(
            g.nodes.len(),
            2,
            "one node type per shape, in declaration order"
        );
        assert_eq!(g.nodes[0].label, "Order");
        assert_eq!(
            g.nodes[0].iri.as_deref(),
            Some("https://example.org/Order"),
            "the node key is the shape IRI"
        );

        // Only the two literal properties stay on the node type.
        let props = &g.nodes[0].properties;
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].name, "total");
        assert_eq!(props[0].datatype, Primitive::Integer);
        assert_eq!(props[0].cardinality, Cardinality::Single);
        assert_eq!(props[0].iri.as_deref(), Some("https://example.org/total"));
        assert_eq!(props[1].name, "note");
        assert_eq!(
            props[1].datatype,
            Primitive::String,
            "an un-narrowed value is a string column"
        );

        // Three edges: one for `placedBy`, two for the disjunction.
        assert_eq!(g.edges.len(), 3);
        assert_eq!(
            (
                g.edges[0].label.as_str(),
                g.edges[0].source.as_str(),
                g.edges[0].destination.as_str()
            ),
            ("placedBy", "Order", "Person")
        );
        assert_eq!(g.edges[0].cardinality, Cardinality::Single);

        let disjunction: Vec<&str> = g.edges[1..]
            .iter()
            .map(|e| e.destination.as_str())
            .collect();
        assert_eq!(
            disjunction,
            ["Card", "Cash"],
            "`@<A> OR @<B>` emits one edge per destination, in order"
        );
        for e in &g.edges[1..] {
            assert_eq!(e.label, "paidWith", "the destinations share the predicate");
            assert_eq!(e.iri.as_deref(), Some("https://example.org/paidWith"));
            assert_eq!(e.cardinality, Cardinality::Multi);
        }

        // The named lookups on the output model still resolve.
        assert_eq!(
            g.edges_from("Order", "https://example.org/paidWith")
                .count(),
            2
        );
        assert!(g.node_by_iri("https://example.org/Person").is_some());
    }

    /// An `AnyUri` datatype is an opaque IRI **column**, not an edge — the
    /// distinction `nodeKind IRI` vs `@<Shape>` makes, carried by `targets`
    /// being empty rather than by the datatype.
    #[test]
    fn an_opaque_iri_property_is_a_column_not_an_edge() {
        let doc = OutputShapes::new(
            vec![shape(
                "https://example.org/Page",
                vec![prop(
                    "https://example.org/seeAlso",
                    Some(Primitive::AnyUri),
                    Occurs::ONE,
                )],
            )],
            vec![],
        );
        let g = doc.to_graph_schema();
        assert!(g.edges.is_empty());
        assert_eq!(g.nodes[0].properties[0].datatype, Primitive::AnyUri);
    }

    #[test]
    fn local_name_splits_on_the_last_hash_or_slash() {
        assert_eq!(local_name("http://xmlns.com/foaf/0.1/name"), "name");
        assert_eq!(
            local_name("http://www.w3.org/2001/XMLSchema#integer"),
            "integer"
        );
        assert_eq!(local_name("https://example.org/a/b#c"), "c");
        assert_eq!(local_name("bare"), "bare");
        assert_eq!(local_name(""), "");
        assert_eq!(local_name("trailing/"), "", "the last segment is empty");
    }

    // -- wire --------------------------------------------------------------

    /// The index is derived, so it is not on the wire — and a round-trip
    /// rebuilds it rather than trusting one. Equality holds because
    /// `OutputShapes::new` is idempotent over an already-deduplicated list.
    #[test]
    fn round_trips_through_json_without_serializing_the_index() {
        let doc = OutputShapes::new(
            vec![shape(
                "https://example.org/Person",
                vec![edge(
                    "https://example.org/knows",
                    &["https://example.org/Person"],
                    Occurs { min: 0, max: None },
                )],
            )],
            vec![Rejection::UnresolvedRef {
                label: "_:missing".into(),
                in_shape: "https://example.org/Person".into(),
            }],
        );
        let json = serde_json::to_string(&doc).expect("serialize");
        assert!(
            !json.contains("index"),
            "the derived index is not part of the contract: {json}"
        );
        let back: OutputShapes = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(doc, back);
        assert!(back.lookup("https://example.org/Person").is_some());
    }
}
