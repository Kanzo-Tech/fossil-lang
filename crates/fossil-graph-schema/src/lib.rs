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
//! graph without pulling the ShEx descriptor or Arrow. The *derivation* from a
//! ShEx descriptor (which does need that machinery) lives next to the descriptor,
//! not here — the contract stays pure.
//!
//! # Identity & references
//!
//! Every node's key is its **subject IRI** (uniform across fossil). An edge
//! `source → destination` references node types by `label`; the relational plan
//! computes the actual IRI-valued columns, and a GraphAr materializer resolves
//! those IRIs to dense ids. The schema only states the shape.

use serde::{Deserialize, Serialize};

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
    pub datatype: DataType,
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

/// The canonical datatype lattice — format-neutral, 1:1 with fossil's type
/// `Primitive`. Materializers map it to their own vocabulary (GraphAr
/// `string/int64/double/…`, xsd `…#string/#integer/…`); the contract stays
/// free of any format spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
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

/// Cardinality of a property or edge — the only distinction a materializer needs
/// (the richer ShEx `Exact(n)`/`ZeroOrOne`/`OneOrMore`/`ZeroOrMore` collapses to
/// this: the first two are [`Single`](Cardinality::Single), the rest
/// [`Multi`](Cardinality::Multi)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// At most one value/edge per subject — a materializer dedups by key.
    Single,
    /// Keep all values/edges per subject (multi-valued / `UNNEST`).
    Multi,
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
                        datatype: DataType::String,
                        iri: Some("https://example.org/name".into()),
                        cardinality: Cardinality::Single,
                    }],
                },
                NodeType {
                    label: "Order".into(),
                    iri: Some("https://example.org/Order".into()),
                    properties: vec![Property {
                        name: "total".into(),
                        datatype: DataType::Integer,
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
        assert_eq!((e.source.as_str(), e.destination.as_str()), ("Order", "Person"));
    }

    #[test]
    fn round_trips_through_json() {
        let g = worked_example();
        let json = serde_json::to_string(&g).expect("serialize");
        let back: GraphSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(g, back, "the wire contract round-trips losslessly");
    }
}
