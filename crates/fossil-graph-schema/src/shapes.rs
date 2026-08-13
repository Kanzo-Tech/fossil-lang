//! What a **shape document** says, format-neutral — the vocabulary the middle of
//! the compiler reads instead of a schema language.
//!
//! # Why this exists
//!
//! [`GraphSchema`] is the *output* contract: node types, edge types, columns. It
//! is what a materializer consumes. But the checker needs something else — the
//! per-shape constraint table it asks "given target shape `ex:Person`, what is
//! the expected type and cardinality of predicate `p`?" — and until now the only
//! spelling of that table was `fossil_shex::ShapeBinding`, whose fields are
//! `rudof_iri::IriS` and `shex_ast::ShapeExpr`. Reading it meant linking ShEx.
//!
//! This module is the same table with no type from any schema language in it. A
//! ShEx decoder lowers into it; a SHACL decoder lowers into it; the compiler
//! reads it and never learns which document it came from. That is the same cut
//! `0e6898d` made for `fossil-mir` — the middle of the compiler is handed a
//! value, not a parser.
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
//! Salsa memoizes an `OutputShapes` (`fossil_base::shape_documents::shape_document`),
//! and salsa 0.26 decides "did this change?" with `PartialEq` — `update_fallback`
//! in `salsa/src/update.rs` compares the old and new values and skips the
//! invalidation entirely when they are equal. So every type here derives
//! `PartialEq + Eq`, and none of them may ever grow a field that compares by
//! address.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Cardinality, EdgeType, GraphSchema, NodeType, Primitive, Property};

/// The local name of an IRI — the substring after the last `#` or `/`.
///
/// This is the *user-visible* rule, not just an internal convenience: a
/// property key in a fossil program is a bare name, and the bare name is the
/// last segment of the predicate IRI that the shape declares. The name depends
/// on the LABEL ALONE — never on the cardinality, the value type or the
/// position — and there is no plural/singular heuristic: a scheme that lets a
/// small change in the document rename a member is how a minor version bump
/// breaks a program. A collision is an error naming both IRIs, never a numeric
/// suffix, and the repair is written in the program, not in the document.
///
/// It lives here because this crate is the one both sides of that rule share:
/// the decoder that produces the IRIs and the schema that carries the names.
/// There were three independent copies of this three-line function in the tree
/// (`fossil-mir/src/lower.rs`, `fossil-shex/src/lib.rs`, `fossil-df/src/shacl.rs`);
/// this is the one they collapse onto.
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

/// One shape: a set of predicates a subject of this type may carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    /// The shape's IRI, fully resolved — the key the output model is addressed
    /// by (`NodeType::iri`).
    pub iri: String,
    /// The predicates, in the order the document lists them.
    pub properties: Vec<PropertyConstraint>,
}

// `closed` was a field here — `ShEx` `CLOSED` / SHACL `sh:closed` — and its doc
// said «the checker reads it to decide whether an unlisted predicate is an
// error». The checker never saw it: `ResolvedShape::from_shape` reads `iri` and
// `properties` and drops the rest, so the only reader in the workspace was one
// test in `fossil-shex`.
//
// It is deleted rather than wired, and the reason is that wiring it would
// change nothing. A property key is a BARE NAME resolved against this table
// (`Checker::resolve_predicate`), so a key no predicate here
// declares is already an error, with a did-you-mean over the ones that do —
// unconditionally, for every shape. Openness has no way to be expressed from
// the writing side, which is the only side a mapping has. A flag that can only
// ever select the behaviour that already happens is not a flag.

/// One predicate of a [`Shape`] — the format-neutral collapse of a `ShEx`
/// `TripleConstraint` / a SHACL property shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyConstraint {
    /// The full predicate IRI, fully resolved. [`local_name`] derives from it
    /// the bare name the program writes.
    pub predicate: String,
    /// The value's datatype, or `None` when **the document did not narrow the
    /// value type**.
    ///
    /// A decoder writes:
    /// - `Some(p)` for a literal narrowed to an XSD datatype in the lattice
    ///   (`Primitive::from_xsd_iri`);
    /// - `Some(Primitive::AnyUri)` for an IRI-valued node that is *not* a
    ///   reference to another shape (`ShEx` `nodeKind IRI`) — an opaque IRI
    ///   column, not an edge;
    /// - `None` for everything else, including an XSD datatype **outside** the
    ///   lattice.
    ///
    /// `None` becomes [`Primitive::String`] in
    /// [`to_graph_schema`](OutputShapes::to_graph_schema) — the permissive
    /// walking-skeleton column. The `Option` exists so that a consumer that
    /// wants to *diagnose* an un-narrowed property can still tell the two apart,
    /// which a bare `Primitive::String` cannot.
    ///
    /// # The checker does something else today, and it is not this
    ///
    /// `None` means "the document did not narrow the value type" — that is the
    /// contract. But the **type checker currently reads the same absence as the
    /// strictest possible expectation**:
    /// `crates/fossil-hir/src/check.rs` resolves a missing `value_ty` with
    /// `unwrap_or_else(|| Ty::new(self.db, TyKind::Iri))`, while
    /// `crates/fossil-hir/src/shapes.rs`'s own field doc says `None` means "any
    /// value". Those two cannot both be right: `Iri` is the narrowest type in
    /// the lattice, "any value" is the widest.
    ///
    /// This is recorded, not fixed — `fossil-hir` is not this crate's to
    /// change. It is written down so that whoever rewrites `fossil-hir` onto
    /// this vocabulary makes the choice **deliberately** instead of preserving
    /// a `TyKind::Iri` default by accident, or deleting it and silently
    /// widening what the checker accepts.
    pub datatype: Option<Primitive>,
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
}

/// How many values a property may carry — the rich form. [`Cardinality`] is its
/// collapse, and [`Occurs::collapse`] is the only way to get there.
///
/// The rich form is a `(min, max)` pair rather than an enum of named cases
/// because the named cases could disagree with themselves: the enum this
/// replaces had both `Exact(n)` and `Range { min: n, max: Some(n) }` for the
/// same cardinality, and its `is_single_valued` answered `true` for the first
/// and `false` for the second. A pair cannot hold two spellings of one fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
/// The one thing lost against the `ShEx`-typed original is the `OneOf` node
/// itself, which the old `SuggestionSeed` cloned so the emitter could
/// re-generate a split-into-N-mappings suggestion. [`Rejection::Disjunction`]
/// carries the predicate IRIs of each branch instead: enough to *name* the split
/// in a diagnostic, not enough to re-emit the document's own syntax. Re-emitting
/// syntax is the decoder's job, and it is the decoder that knows the syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    /// A disjunction the compiler does not support. `disjuncts` carries the
    /// predicate IRIs of each branch, in order — `disjuncts.len()` is the number
    /// of branches, and each inner `Vec` is what that branch constrains.
    Disjunction {
        shape_iri: String,
        disjuncts: Vec<Vec<String>>,
    },
    /// Only acyclic shape graphs are supported (`type-system.md` §11). `path` is
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
    /// alone: a later duplicate that overwrites an earlier slot moves the shape
    /// that positional binding is counting on. So the duplicate is dropped
    /// entirely, not merged and not appended.
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

    /// Every shape, **in declaration order**. Callers may rely on that:
    /// `type { A, B } := io.shex(…)` binds **by position**, the Nth name to the
    /// Nth declared shape, so `A` is the first shape the document declares and
    /// `B` the second — the local name selects nothing and is free to be
    /// anything. A `HashMap` lived where this `Vec` is until
    /// `crates/fossil-shex/examples/declaration_order.rs` measured what it cost
    /// — six parses of one document handed back six different orders, because
    /// Rust seeds its hasher per process. The order is the document's; it is not
    /// ours to choose.
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
    /// document checks [`Self::rejections`] first. That was already true of the
    /// `ShEx` original, which lowered the non-rejected parts of every shape.
    #[must_use]
    pub fn to_graph_schema(&self) -> GraphSchema {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for shape in self.shapes() {
            let label = local_name(&shape.iri).to_string();
            let mut properties = Vec::new();
            for c in &shape.properties {
                let name = local_name(&c.predicate).to_string();
                let cardinality = c.occurs.collapse();
                if c.targets.is_empty() {
                    properties.push(Property {
                        name,
                        // An un-narrowed value is a string column: the
                        // permissive walking-skeleton default the `ShEx`
                        // lowering already applied to every `valueExpr` it could
                        // not read.
                        datatype: c.datatype.unwrap_or(Primitive::String),
                        iri: Some(c.predicate.clone()),
                        cardinality,
                    });
                } else {
                    for t in &c.targets {
                        edges.push(EdgeType {
                            label: name.clone(),
                            iri: Some(c.predicate.clone()),
                            source: label.clone(),
                            destination: local_name(t).to_string(),
                            cardinality,
                        });
                    }
                }
            }
            nodes.push(NodeType {
                label,
                iri: Some(shape.iri.clone()),
                properties,
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
            predicate: predicate.into(),
            datatype,
            targets: Vec::new(),
            occurs,
        }
    }

    fn edge(predicate: &str, targets: &[&str], occurs: Occurs) -> PropertyConstraint {
        PropertyConstraint {
            predicate: predicate.into(),
            datatype: None,
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            occurs,
        }
    }

    fn shape(iri: &str, properties: Vec<PropertyConstraint>) -> Shape {
        Shape {
            iri: iri.into(),
            properties,
        }
    }

    // -- declaration order + first declaration wins ------------------------

    /// The order is the document's. Nothing sorts, nothing hashes on the way
    /// out — the destructuring binds by position, the Nth name to the Nth
    /// declared shape.
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

    // -- Occurs::collapse over the five shapes the old enum could take -----

    /// The enum this replaces had five variants; a later commit deletes it, and
    /// this is the only thing that will catch a drift. Each row is
    /// `(the old variant, its (min, max), what its `is_single_valued` answered)`.
    ///
    /// One deliberate divergence, and it is unreachable: the old `Exact(n)`
    /// answered `true` for **every** `n`, so `Exact(3)` claimed to be
    /// single-valued while `Range { min: 3, max: Some(3) }` — the same
    /// cardinality — answered `false`. `Occurs` cannot hold that contradiction,
    /// and it resolves to the `Range` answer. Nothing observes the difference:
    /// `Cardinality::from_shex` only ever produced `Exact(1)` (`(None, None)`),
    /// and `(Some(lo), None)` went to `Range { min: lo, max: Some(lo) }`.
    #[test]
    fn collapse_pins_every_shape_the_old_enum_could_take() {
        let cases: &[(&str, Occurs, bool)] = &[
            // Exact(1) — `from_shex(None, None)`, the ShEx default.
            (
                "Exact(1)",
                Occurs {
                    min: 1,
                    max: Some(1),
                },
                true,
            ),
            // Exact(0) — degenerate but single-valued under both rules.
            (
                "Exact(0)",
                Occurs {
                    min: 0,
                    max: Some(0),
                },
                true,
            ),
            // Exact(3) — the divergence. Old: true. New: false. Unconstructible.
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

    /// `demands_one_or_more` is `min >= 1` for every one of the five, with no
    /// divergence at all — the old `Exact(n) => n >= 1` is exactly `min >= 1`.
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
