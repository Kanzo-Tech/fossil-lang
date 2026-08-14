//! SHACL shapes graph → [`OutputShapes`] — the SHACL row's decode.
//!
//! SHACL **is** RDF: a shapes graph is a set of triples. So rather than pull in
//! the `shacl_ast`/`srdf` stack (generic over an RDF backend, heavy,
//! WASM-awkward), the graph is walked with the same pure-Rust `oxttl`/`oxrdf`
//! reader the `io.rdf` provider already uses (WASM-clean) and the SHACL Core
//! structural vocabulary is projected onto the neutral vocabulary:
//!
//! - `sh:NodeShape` + `sh:targetClass T`   → a [`Shape`] whose `iri` is `T`.
//! - `sh:property [ sh:path p ; sh:datatype D ]`         → a literal constraint.
//! - `sh:property [ sh:path p ; sh:nodeKind sh:IRI ]`    → an `AnyUri` constraint.
//! - `sh:property [ sh:path p ; sh:class C | sh:node S ]`→ a constraint with a target.
//! - `sh:property [ sh:path p ; sh:or ( [sh:class A] [sh:class B] ) ]`
//!   → one constraint with TWO targets (the reference RDF→property-graph union
//!   model, identical to `ShEx`'s `@<A> OR @<B>`).
//! - `sh:minCount` / `sh:maxCount` → [`Occurs`]; absent means SHACL's own
//!   defaults, `0` and unbounded.
//!
//! Constraint features outside the graph's *shape* (`sh:pattern`, `sh:sparql`,
//! severities, …) are validation concerns and intentionally ignored here — this
//! lowering only needs the node/edge/property structure. **What a language
//! expresses and the neutral vocabulary does not carry is the ROW's to
//! diagnose**, and today this row carries no [`Rejection`] but the parse
//! failure: everything above is either lowered or is a validation constraint the
//! output model has no place for.
//!
//! # Where this used to live, and why it moved
//!
//! `crates/fossil-df/src/shacl.rs`, producing a `GraphSchema` directly. That was
//! the OUTPUT model, one step past the vocabulary the checker reads, so SHACL
//! could reach the executor and never the checker — which is exactly why
//! `catalogue.fossil` had no output contract and could not write a property.
//! Producing [`OutputShapes`] puts SHACL on the same seam as `ShEx`, and
//! `OutputShapes::to_graph_schema` gives the executor byte-identical output to
//! what the old walk produced.
//!
//! # Declaration order is load-bearing
//!
//! `type { A, B } := io.shacl("shapes.ttl")` binds **by position** — the Nth
//! name to the Nth declared shape, not by matching names — so the order shapes
//! come out in is the order the document declares them: first appearance as a
//! subject in the Turtle. The store this
//! replaces used a `BTreeMap`, whose iteration is alphabetical by subject IRI;
//! with one shape that is invisible, and with two it silently swaps what a
//! program bound. `crates/fossil-shex/examples/declaration_order.rs` measured
//! the same class of bug once already.

use std::collections::BTreeMap;

use fossil_graph_schema::{Occurs, OutputShapes, Primitive, PropertyConstraint, Rejection, Shape};
use oxrdf::{NamedOrBlankNode, Term};
use oxttl::TurtleParser;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const SH_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";
const SH_TARGET_CLASS: &str = "http://www.w3.org/ns/shacl#targetClass";
const SH_PROPERTY: &str = "http://www.w3.org/ns/shacl#property";
const SH_PATH: &str = "http://www.w3.org/ns/shacl#path";
const SH_CLASS: &str = "http://www.w3.org/ns/shacl#class";
const SH_NODE: &str = "http://www.w3.org/ns/shacl#node";
const SH_DATATYPE: &str = "http://www.w3.org/ns/shacl#datatype";
const SH_NODE_KIND: &str = "http://www.w3.org/ns/shacl#nodeKind";
const SH_IRI: &str = "http://www.w3.org/ns/shacl#IRI";
const SH_MIN_COUNT: &str = "http://www.w3.org/ns/shacl#minCount";
const SH_MAX_COUNT: &str = "http://www.w3.org/ns/shacl#maxCount";
const SH_OR: &str = "http://www.w3.org/ns/shacl#or";

/// One object of a triple: its lexical value plus whether it is a node (IRI or
/// blank — i.e. follow-able as a subject) vs a literal.
struct Obj {
    value: String,
    is_node: bool,
}

/// A minimal in-memory triple store: `subject → predicate → objects`, plus the
/// order subjects first appeared. Enough to walk SHACL's shape/property/list
/// structure without an RDF database.
struct Store {
    by_subject: BTreeMap<String, BTreeMap<String, Vec<Obj>>>,
    /// Subjects in first-appearance order — see the module docs on why the map's
    /// own order is not usable.
    order: Vec<String>,
}

impl Store {
    fn parse(turtle: &str) -> Result<Self, String> {
        let mut by_subject: BTreeMap<String, BTreeMap<String, Vec<Obj>>> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        let mut parser = TurtleParser::new().for_reader(turtle.as_bytes());
        for triple in parser.by_ref() {
            let t = triple.map_err(|e| format!("parse SHACL turtle: {e}"))?;
            let subject = subject_value(&t.subject);
            if !by_subject.contains_key(&subject) {
                order.push(subject.clone());
            }
            by_subject
                .entry(subject)
                .or_default()
                .entry(t.predicate.as_str().to_string())
                .or_default()
                .push(Obj {
                    value: value_of(&t.object),
                    is_node: is_node(&t.object),
                });
        }
        Ok(Self { by_subject, order })
    }

    /// All object values of `(subject, predicate)`.
    fn objects<'a>(&'a self, subject: &str, predicate: &str) -> impl Iterator<Item = &'a Obj> {
        self.by_subject
            .get(subject)
            .and_then(|p| p.get(predicate))
            .into_iter()
            .flatten()
    }

    /// The first object value of `(subject, predicate)`, if any.
    fn first(&self, subject: &str, predicate: &str) -> Option<&str> {
        self.objects(subject, predicate)
            .next()
            .map(|o| o.value.as_str())
    }

    /// Follow an RDF list (`rdf:first`/`rdf:rest` … `rdf:nil`) from `head`,
    /// returning each member's node value. Bounded by the store size to avoid a
    /// cyclic-list infinite loop in malformed input.
    fn rdf_list(&self, head: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = head.to_string();
        let mut guard = 0;
        while cur != RDF_NIL && guard <= self.by_subject.len() {
            guard += 1;
            // Clone out of each borrow before reassigning `cur`, so the iterator
            // temporary (which borrows `cur`) is dropped first (E0506).
            if let Some(first) = self
                .objects(&cur, RDF_FIRST)
                .next()
                .map(|o| o.value.clone())
            {
                out.push(first);
            }
            let Some(rest) = self.objects(&cur, RDF_REST).next().map(|o| o.value.clone()) else {
                break;
            };
            cur = rest;
        }
        out
    }
}

/// Decode a SHACL shapes graph — the SHACL row's `decode`.
///
/// The `uri` is unread: SHACL's relative-reference base would change which IRIs
/// a document resolves to, and choosing one is a decision rather than a
/// tidy-up — the same call [`crate::decode_shex`] makes and for the same reason.
///
/// # Errors
///
/// [`Rejection::Malformed`] when the Turtle does not parse. A document that
/// parses and declares no node shape is **not** an error: it decodes to zero
/// shapes, and the binding that named it reports its own arity failure.
pub fn decode_shacl(_uri: &str, turtle: &str) -> Result<OutputShapes, Rejection> {
    let store = Store::parse(turtle).map_err(Rejection::Malformed)?;
    let mut shapes: Vec<Shape> = Vec::new();

    for subject in &store.order {
        let Some(preds) = store.by_subject.get(subject) else {
            continue;
        };
        let is_node_shape = store
            .objects(subject, RDF_TYPE)
            .any(|o| o.value == SH_NODE_SHAPE)
            || preds.contains_key(SH_TARGET_CLASS)
            || preds.contains_key(SH_PROPERTY);
        if !is_node_shape {
            continue;
        }
        // The subject's rdf:type is its `sh:targetClass`; absent that, the
        // shape's own IRI is the type (a common self-typed convention). That
        // choice is what makes `shop:ProductShape` with `sh:targetClass
        // shop:Product` produce a `Product` node type and not a `ProductShape`
        // one.
        let iri = store
            .first(subject, SH_TARGET_CLASS)
            .unwrap_or(subject)
            .to_string();

        let mut properties = Vec::new();
        for psh in store.objects(subject, SH_PROPERTY).filter(|o| o.is_node) {
            let Some(path) = store.first(&psh.value, SH_PATH) else {
                continue;
            };
            properties.push(PropertyConstraint {
                predicate: path.to_string(),
                datatype: datatype_of(&store, &psh.value),
                targets: edge_targets(&store, &psh.value),
                occurs: occurs_of(&store, &psh.value),
            });
        }
        shapes.push(Shape { iri, properties });
    }

    Ok(OutputShapes::new(shapes, Vec::new()))
}

/// The destination type IRIs a property shape points at: `sh:class`, the target
/// class of any `sh:node` shape, and every alternative inside an `sh:or` list.
/// Empty ⇒ the property is a literal / opaque-IRI value, not an edge.
fn edge_targets(store: &Store, property_shape: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for c in store.objects(property_shape, SH_CLASS) {
        targets.push(c.value.clone());
    }
    for n in store.objects(property_shape, SH_NODE).filter(|o| o.is_node) {
        targets.push(node_target(store, &n.value));
    }
    for or_head in store.objects(property_shape, SH_OR).filter(|o| o.is_node) {
        for member in store.rdf_list(&or_head.value) {
            for c in store.objects(&member, SH_CLASS) {
                targets.push(c.value.clone());
            }
            for n in store.objects(&member, SH_NODE).filter(|o| o.is_node) {
                targets.push(node_target(store, &n.value));
            }
        }
    }
    targets
}

/// A `sh:node`-referenced shape's destination type: its `sh:targetClass`, or the
/// referenced shape's own IRI when it declares none.
fn node_target(store: &Store, node_shape: &str) -> String {
    store
        .first(node_shape, SH_TARGET_CLASS)
        .unwrap_or(node_shape)
        .to_string()
}

/// How many values the property may carry. SHACL's own defaults: no `sh:minCount`
/// is `0` (optional) and no `sh:maxCount` is unbounded.
fn occurs_of(store: &Store, property_shape: &str) -> Occurs {
    let count = |p: &str| {
        store
            .first(property_shape, p)
            .and_then(|v| v.parse::<u32>().ok())
    };
    Occurs {
        min: count(SH_MIN_COUNT).unwrap_or(0),
        max: count(SH_MAX_COUNT),
    }
}

/// The value type a property shape narrows to, or `None` when it narrows
/// nothing — including an XSD datatype outside the lattice, which is the
/// contract [`PropertyConstraint::datatype`] states.
///
/// `sh:nodeKind sh:IRI` with no `sh:class` is an **opaque IRI column**, not an
/// edge: `Primitive::AnyUri`.
fn datatype_of(store: &Store, property_shape: &str) -> Option<Primitive> {
    if let Some(dt) = store.first(property_shape, SH_DATATYPE) {
        return Primitive::from_xsd_iri(dt);
    }
    if store.first(property_shape, SH_NODE_KIND) == Some(SH_IRI) {
        return Some(Primitive::AnyUri);
    }
    None
}

/// The bare IRI / blank-node label of a triple subject (`as_str`, not the `<>`
/// `Display` form).
fn subject_value(subject: &NamedOrBlankNode) -> String {
    match subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNode::BlankNode(b) => b.to_string(),
    }
}

/// An object term's lexical value.
fn value_of(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::BlankNode(b) => b.to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::Triple(_) => term.to_string(),
    }
}

/// Is this object a node (IRI/blank, follow-able as a subject) rather than a
/// literal?
const fn is_node(term: &Term) -> bool {
    matches!(term, Term::NamedNode(_) | Term::BlankNode(_))
}

#[cfg(test)]
mod tests {
    use fossil_graph_schema::{Cardinality, EdgeType};

    use super::*;

    // One node shape exercising every canonical mapping: a typed literal
    // (→ a narrowed constraint, single-valued via sh:maxCount 1), an opaque IRI
    // (sh:nodeKind sh:IRI → AnyUri), a plain edge (sh:class), and an OR edge
    // (sh:or → two targets on one constraint, one EdgeType each).
    const SHAPES: &str = r"
@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <https://ex.org/> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name     ; sh:datatype xsd:string ; sh:maxCount 1 ] ;
    sh:property [ sh:path ex:homepage ; sh:nodeKind sh:IRI ] ;
    sh:property [ sh:path ex:knows    ; sh:class ex:Person ] ;
    sh:property [ sh:path ex:contact  ; sh:or ( [ sh:class ex:Person ] [ sh:class ex:Org ] ) ] .
";

    #[test]
    fn shacl_lowers_literals_iris_and_edges_into_the_neutral_vocabulary() {
        let doc = decode_shacl("shapes.ttl", SHAPES).expect("turtle parses");
        assert_eq!(doc.shapes().count(), 1);
        let person = doc.lookup("https://ex.org/Person").expect("targetClass");

        let by = |p: &str| {
            person
                .properties
                .iter()
                .find(|c| c.predicate == format!("https://ex.org/{p}"))
                .unwrap_or_else(|| panic!("{p}"))
        };
        assert_eq!(by("name").datatype, Some(Primitive::String));
        assert_eq!(
            by("name").occurs,
            Occurs {
                min: 0,
                max: Some(1)
            }
        );
        assert_eq!(
            by("homepage").datatype,
            Some(Primitive::AnyUri),
            "`sh:nodeKind sh:IRI` with no class is an opaque IRI column"
        );
        assert!(by("homepage").targets.is_empty(), "and not an edge");
        assert_eq!(by("knows").targets, ["https://ex.org/Person"]);
        assert_eq!(
            by("contact").targets,
            ["https://ex.org/Person", "https://ex.org/Org"],
            "`sh:or` is one constraint with two destinations"
        );
    }

    /// The executor consumes `GraphSchema`, and it must get exactly what the
    /// `GraphSchema`-producing walk this replaces produced.
    #[test]
    fn the_graph_schema_is_what_the_direct_walk_used_to_produce() {
        let gs = decode_shacl("shapes.ttl", SHAPES)
            .expect("parses")
            .to_graph_schema();
        assert_eq!(gs.nodes.len(), 1);
        assert_eq!(gs.nodes[0].label, "Person");
        assert_eq!(gs.nodes[0].iri.as_deref(), Some("https://ex.org/Person"));

        let names: Vec<&str> = gs.nodes[0]
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["name", "homepage"], "edges are not properties");
        assert_eq!(gs.nodes[0].properties[0].cardinality, Cardinality::Single);
        assert_eq!(gs.nodes[0].properties[1].cardinality, Cardinality::Multi);

        let mut contact: Vec<&EdgeType> =
            gs.edges.iter().filter(|e| e.label == "contact").collect();
        contact.sort_by(|a, b| a.destination.cmp(&b.destination));
        assert_eq!(contact.len(), 2, "one edge per alternative");
        assert_eq!(contact[0].destination, "Org");
        assert_eq!(contact[1].destination, "Person");
        assert!(contact.iter().all(|e| e.source == "Person"));
    }

    /// Positional binding reads shapes in DECLARATION order. Alphabetically
    /// `ex:A…` sorts before `ex:Z…`; the document declares Z first, and the
    /// `BTreeMap` this walk used to iterate would have swapped them.
    #[test]
    fn shapes_come_out_in_declaration_order_not_alphabetical() {
        const TWO: &str = r"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://ex.org/> .

ex:ZetaShape  a sh:NodeShape ; sh:targetClass ex:Zeta .
ex:AlphaShape a sh:NodeShape ; sh:targetClass ex:Alpha .
";
        let doc = decode_shacl("two.ttl", TWO).expect("parses");
        let order: Vec<&str> = doc.shapes().map(|s| s.iri.as_str()).collect();
        assert_eq!(order, ["https://ex.org/Zeta", "https://ex.org/Alpha"]);
    }

    /// A shape with no `sh:targetClass` is typed by its own IRI.
    #[test]
    fn a_shape_without_a_target_class_is_typed_by_itself() {
        const SELF_TYPED: &str = r"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://ex.org/> .

ex:Thing a sh:NodeShape ; sh:property [ sh:path ex:label ] .
";
        let doc = decode_shacl("t.ttl", SELF_TYPED).expect("parses");
        assert!(doc.lookup("https://ex.org/Thing").is_some());
    }

    /// Text that is not Turtle is a rejection the caller can put in front of a
    /// user, not a panic and not a silent empty document.
    #[test]
    fn text_that_is_not_turtle_is_malformed() {
        let err = decode_shacl("broken.ttl", "{ this is not turtle").expect_err("must fail");
        assert!(matches!(err, Rejection::Malformed(_)), "got {err:?}");
    }
}
