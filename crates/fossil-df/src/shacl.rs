//! SHACL shapes-graph → canonical [`GraphSchema`] — the SHACL arm of the output
//! model. SHACL **is** RDF: a shapes graph is a set of triples. So rather than
//! pull in the `shacl_ast`/`srdf` stack (generic over an RDF backend, heavy,
//! WASM-awkward), we walk the shapes graph with the same pure-Rust `oxttl`/`oxrdf`
//! reader the `io.rdf` provider already uses (WASM-clean) and project the SHACL
//! Core structural vocabulary onto [`GraphSchema`]:
//!
//! - `sh:NodeShape` + `sh:targetClass T`  → a [`NodeType`] (`iri = T`).
//! - `sh:property [ sh:path p ; sh:datatype D ]`        → a literal [`Property`].
//! - `sh:property [ sh:path p ; sh:nodeKind sh:IRI ]`   → an `AnyUri` [`Property`].
//! - `sh:property [ sh:path p ; sh:class C | sh:node S ]`→ an [`EdgeType`] → C.
//! - `sh:property [ sh:path p ; sh:or ( [sh:class A] [sh:class B] ) ]`
//!     → one edge per alternative (the reference RDF→property-graph union model,
//!       identical to ShEx's `@<A> OR @<B>`).
//! - `sh:maxCount 1` ⇒ `Single`, otherwise `Multi`.
//!
//! Constraint features outside the graph's *shape* (`sh:minCount`, `sh:pattern`,
//! `sh:sparql`, severities, …) are validation concerns and intentionally ignored
//! here — this lowering only needs the node/edge/property structure.

use std::collections::BTreeMap;

use fossil_graph_schema::{Cardinality, DataType, EdgeType, GraphSchema, NodeType, Property};
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
const SH_MAX_COUNT: &str = "http://www.w3.org/ns/shacl#maxCount";
const SH_OR: &str = "http://www.w3.org/ns/shacl#or";

/// One object of a triple: its lexical value plus whether it is a node (IRI or
/// blank — i.e. follow-able as a subject) vs a literal.
struct Obj {
    value: String,
    is_node: bool,
}

/// A minimal in-memory triple store: `subject → predicate → objects`. Enough to
/// walk SHACL's shape/property/list structure without an RDF database.
struct Store {
    by_subject: BTreeMap<String, BTreeMap<String, Vec<Obj>>>,
}

impl Store {
    fn parse(turtle: &str) -> Result<Self, String> {
        let mut by_subject: BTreeMap<String, BTreeMap<String, Vec<Obj>>> = BTreeMap::new();
        let mut parser = TurtleParser::new().for_reader(turtle.as_bytes());
        for triple in parser.by_ref() {
            let t = triple.map_err(|e| format!("parse SHACL turtle: {e}"))?;
            let subject = subject_value(&t.subject);
            let (value, is_node) = object_value(&t.object);
            by_subject
                .entry(subject)
                .or_default()
                .entry(t.predicate.as_str().to_string())
                .or_default()
                .push(Obj { value, is_node });
        }
        Ok(Self { by_subject })
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
        self.objects(subject, predicate).next().map(|o| o.value.as_str())
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
            if let Some(first) = self.objects(&cur, RDF_FIRST).next().map(|o| o.value.clone()) {
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

/// Lower a SHACL shapes graph (Turtle) into the canonical [`GraphSchema`].
///
/// # Errors
/// Turtle parse failures.
pub fn shacl_to_graph_schema(turtle: &str) -> Result<GraphSchema, String> {
    let store = Store::parse(turtle)?;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for (subject, preds) in &store.by_subject {
        let is_node_shape = store.objects(subject, RDF_TYPE).any(|o| o.value == SH_NODE_SHAPE)
            || preds.contains_key(SH_TARGET_CLASS)
            || preds.contains_key(SH_PROPERTY);
        if !is_node_shape {
            continue;
        }
        // The node's rdf:type is its `sh:targetClass`; absent that, the shape's
        // own IRI is the type (a common self-typed convention).
        let type_iri = store
            .first(subject, SH_TARGET_CLASS)
            .unwrap_or(subject)
            .to_string();
        let label = local_name(&type_iri).to_string();

        let mut properties = Vec::new();
        for psh in store.objects(subject, SH_PROPERTY).filter(|o| o.is_node) {
            let Some(path) = store.first(&psh.value, SH_PATH) else {
                continue;
            };
            let path = path.to_string();
            let name = local_name(&path).to_string();
            let cardinality = if store.first(&psh.value, SH_MAX_COUNT) == Some("1") {
                Cardinality::Single
            } else {
                Cardinality::Multi
            };

            let targets = edge_targets(&store, &psh.value);
            if targets.is_empty() {
                properties.push(Property {
                    name,
                    datatype: datatype_of(&store, &psh.value),
                    iri: Some(path),
                    cardinality,
                });
            } else {
                for t in targets {
                    edges.push(EdgeType {
                        label: name.clone(),
                        iri: Some(path.clone()),
                        source: label.clone(),
                        destination: local_name(&t).to_string(),
                        cardinality,
                    });
                }
            }
        }

        nodes.push(NodeType {
            label,
            iri: Some(type_iri),
            properties,
        });
    }

    Ok(GraphSchema { nodes, edges })
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

/// The canonical datatype of a non-edge property shape: `sh:datatype` through the
/// XSD lattice (unknown ⇒ `String`); `sh:nodeKind sh:IRI` ⇒ opaque `AnyUri`;
/// otherwise `String`.
fn datatype_of(store: &Store, property_shape: &str) -> DataType {
    if let Some(dt) = store.first(property_shape, SH_DATATYPE) {
        return DataType::from_xsd_iri(dt).unwrap_or(DataType::String);
    }
    if store.first(property_shape, SH_NODE_KIND) == Some(SH_IRI) {
        return DataType::AnyUri;
    }
    DataType::String
}

/// The local name of an IRI — the substring after the last `#` or `/`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// The bare IRI / blank-node label of a triple subject (`as_str`, not the `<>`
/// `Display` form).
fn subject_value(subject: &NamedOrBlankNode) -> String {
    match subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNode::BlankNode(b) => b.to_string(),
    }
}

/// An object term's lexical value + whether it is a node (IRI/blank, follow-able)
/// vs a literal.
fn object_value(term: &Term) -> (String, bool) {
    match term {
        Term::NamedNode(n) => (n.as_str().to_string(), true),
        Term::BlankNode(b) => (b.to_string(), true),
        Term::Literal(l) => (l.value().to_string(), false),
        Term::Triple(_) => (term.to_string(), false),
    }
}
