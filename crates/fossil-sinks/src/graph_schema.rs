//! Derive the canonical [`GraphSchema`] from a `ShEx` output descriptor — the
//! universal substrate's contract (`fossil-graph-schema`), built from the same
//! descriptor `decomp` reads but stripped of every storage/SQL detail.
//!
//! A property graph is a schema over relations: each `ShEx` shape is a node type
//! (keyed by its subject IRI); each constraint is either a literal **property**
//! (a `NodeConstraint` datatype) or an **edge** (a `Ref` to another shape). The
//! cardinality (`Exact`/`ZeroOrOne` → single, `*OrMore` → multi) rides through.
//!
//! This is the format-neutral replacement for [`crate::decomp`]'s `SinkPlan`
//! (which is GraphAr-shaped: SQL relations, `dense_id` column names, chunk
//! sizes). It will outlive `decomp`, so it lives in its own module; when the
//! legacy SQL path is deleted it moves to its permanent home (the lowering
//! pipeline). The contract stays pure — only this derivation touches `ShEx`.

use fossil_descriptors_output::OutputDescriptorKind;
use fossil_graph_schema::{Cardinality, DataType, EdgeType, GraphSchema, NodeType, Property};
use shex_ast::{NodeKind, ShapeExpr};

use crate::decomp::{local_name, shape_label_iri};

/// Build the [`GraphSchema`] for an output descriptor. `AcceptAll` (no shape
/// target) yields the empty graph — there is no schema to interpret the
/// relations as a graph (the flat-triple passthrough is just relations).
#[must_use]
pub fn graph_schema(kind: &OutputDescriptorKind) -> GraphSchema {
    let OutputDescriptorKind::ShEx(desc) = kind else {
        return GraphSchema {
            nodes: Vec::new(),
            edges: Vec::new(),
        };
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Deterministic order: shapes by IRI, constraints by predicate.
    let mut shapes: Vec<_> = desc.shapes().collect();
    shapes.sort_by(|a, b| a.iri.to_string().cmp(&b.iri.to_string()));

    for shape in shapes {
        let shape_iri = shape.iri.to_string();
        let label = local_name(&shape_iri);
        let mut properties = Vec::new();

        let mut constraints: Vec<_> = shape.constraints.iter().collect();
        constraints.sort_by(|a, b| a.predicate.to_string().cmp(&b.predicate.to_string()));

        for c in constraints {
            let pred_iri = c.predicate.to_string();
            let pred_local = local_name(&pred_iri);
            let cardinality = if c.cardinality.is_single_valued() {
                Cardinality::Single
            } else {
                Cardinality::Multi
            };

            match classify(c) {
                Object::Property(datatype) => properties.push(Property {
                    name: pred_local,
                    datatype,
                    iri: Some(pred_iri),
                    cardinality,
                }),
                Object::Edge(dst_iri) => edges.push(EdgeType {
                    label: pred_local,
                    iri: Some(pred_iri),
                    source: label.clone(),
                    destination: local_name(&dst_iri),
                    cardinality,
                }),
                Object::Skip => {}
            }
        }

        nodes.push(NodeType {
            label,
            iri: Some(shape_iri),
            properties,
        });
    }

    GraphSchema { nodes, edges }
}

/// The graph-native classification of a constraint's object (vs `decomp`'s
/// arrow-typed `ObjectKind`): a literal property's canonical datatype, an edge
/// to another shape, or skip.
enum Object {
    Property(DataType),
    Edge(String),
    Skip,
}

/// Classify a constraint by its `value_expr`: a `Ref` is an edge to that shape;
/// a `NodeConstraint` with a datatype is a typed property; an IRI-kind node
/// constraint or a bare constraint is an opaque-IRI/string property; anything
/// else (`ShapeOr`/`ShapeAnd`/…) is skipped in v0.1.
fn classify(c: &fossil_descriptors_output::ResolvedConstraint) -> Object {
    match &c.value_expr {
        Some(ShapeExpr::Ref(label)) => Object::Edge(shape_label_iri(label)),
        Some(ShapeExpr::NodeConstraint(nc)) => match nc.datatype() {
            Some(dt) => Object::Property(
                DataType::from_xsd_iri(&dt.to_string()).unwrap_or(DataType::String),
            ),
            None if matches!(nc.node_kind(), Some(NodeKind::Iri)) => {
                Object::Property(DataType::String) // opaque IRI literal
            }
            None => Object::Skip,
        },
        None => Object::Property(DataType::String), // bare `.` constraint → string
        Some(_) => Object::Skip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_descriptors_output::ShExDescriptor;

    // Person { name: xsd:string ; knows: @Person (min=1,max=-1 ⇒ multi) } ;
    // Company { legalName: xsd:string }.  `knows` → an edge; the rest → props.
    const TWO_SHAPE: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        { "type": "ShapeDecl", "id": "http://example.org/Person",
          "shapeExpr": { "type": "Shape", "expression": { "type": "EachOf", "expressions": [
            { "type": "TripleConstraint", "predicate": "http://example.org/name",
              "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#string" } },
            { "type": "TripleConstraint", "predicate": "http://example.org/knows",
              "valueExpr": "http://example.org/Person", "min": 1, "max": -1 } ] } } },
        { "type": "ShapeDecl", "id": "http://example.org/Company",
          "shapeExpr": { "type": "Shape", "expression": {
            "type": "TripleConstraint", "predicate": "http://example.org/legalName",
            "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#string" } } } }
      ]
    }"#;

    #[test]
    fn derives_nodes_and_edges_from_shex() {
        let desc = ShExDescriptor::from_reader(TWO_SHAPE.as_bytes()).expect("parses");
        let schema = graph_schema(&OutputDescriptorKind::ShEx(desc));

        // Nodes sorted by IRI: Company, Person.
        let labels: Vec<&str> = schema.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, ["Company", "Person"]);

        let person = schema.node("Person").expect("Person");
        assert_eq!(person.iri.as_deref(), Some("http://example.org/Person"));
        // `knows` is an edge — only `name` is a property.
        assert_eq!(person.properties.len(), 1);
        let name = &person.properties[0];
        assert_eq!(name.name, "name");
        assert_eq!(name.datatype, DataType::String);
        assert_eq!(name.iri.as_deref(), Some("http://example.org/name"));
        assert_eq!(name.cardinality, Cardinality::Single);

        // One edge: knows, Person → Person, multi-valued (min=1 max=-1).
        assert_eq!(schema.edges.len(), 1);
        let knows = schema.edge("knows").expect("knows edge");
        assert_eq!(knows.iri.as_deref(), Some("http://example.org/knows"));
        assert_eq!((knows.source.as_str(), knows.destination.as_str()), ("Person", "Person"));
        assert_eq!(knows.cardinality, Cardinality::Multi);
    }

    #[test]
    fn accept_all_is_the_empty_graph() {
        use fossil_descriptors_output::AcceptAllDescriptor;
        let schema = graph_schema(&OutputDescriptorKind::AcceptAll(AcceptAllDescriptor));
        assert!(schema.nodes.is_empty() && schema.edges.is_empty());
    }
}
