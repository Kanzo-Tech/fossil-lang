//! What the graph's tables mean as RDF: [`describe`] builds the RML-Core
//! mapping that `mapping.ttl` is, with rudof's `rml` AST, and `rml::rdf`
//! writes it.
//!
//! **The reference is RML-Core** (Draft Community Group Report, 16 March 2026)
//! with RML-IO for the logical source, as `rml` reads them; the shape of the
//! document is `/docs/format/reading/rdf`'s and the argument for it
//! `/docs/design/graphar`'s. The graph says nothing of a class, a predicate or
//! a datatype (`GraphAr` has no word for them), so the mapping is where the
//! program's IRIs are.
//!
//! **It is a pure function of the compiled schema and the layout**: it reads no
//! data and no engine. One triples map per vertex type, over the table of that
//! name (`rml:SQL2008Table`); one per relation, over a query that joins the
//! adjacency list to the two end tables on the index and reads the two
//! subjects; one per multi-valued or language-tagged property, over a query
//! that unnests the list once so each row is one element. The tables are SQL
//! names, which the host resolves.
//!
//! **No term map reads an index, a row position or a blank node**, which is
//! what makes the image layout independent (R1, `/docs/format/round-trip`): the
//! index columns appear in the relation's query and in no term map, and the
//! mapping defines no blank node (A6) because the AST has no way to say one
//! that is not a fresh node per iteration, which `describe` never builds.
//!
//! **A2 is held here**, because it is a property of the mapping: two types do
//! not share a class, a class does not give two columns one predicate, and no
//! relation's predicate is a column predicate of its source class. A2 is
//! checked before a chunk is written (`write` calls this beside the layout).
//! A column or a type with no IRI is not mapped: a graph of a program that
//! names no shape has tables and no RDF meaning.

// `pub(crate)`: this module exports nothing, as `layout` does not.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;

use fossil_graph_schema::{
    Annotation as Declared, AnnotationObject, EdgeType, GraphSchema, NodeType, Primitive, Property,
    Term,
};
use graphar_info::{Cardinality as Several, DataType as Stored, GraphInfo, VertexInfo};
use graphar_parquet::{DST_INDEX_COL, SRC_INDEX_COL, VERTEX_INDEX_COL};
use rml::{
    Annotation, Constant, Iterable, LanguageMap, Literal, LiteralKind, LogicalSource, Mapping,
    Node, ObjectMap, PredicateObjectMap, SubjectMap, TermMap, TermType, TriplesMap,
};
use rudof_iri::IriS;

use crate::PAYLOAD_IDENTITY;
use crate::layout::LANG_STRING;
use crate::write::WriteError;

/// The XSD namespace, for the datatype a column keeps that `GraphAr` has no
/// type for.
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// What a run says about the tables it writes, beside the tables. Only the
/// mapping today; the shapes and the lineage come with a reader
/// (`/docs/design/graphar`, D1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Description {
    pub mapping: Mapping,
}

/// The datatype of a column that was stored as text, by the XSD local name,
/// keyed by `(vertex type, column)`.
pub(crate) type Natural = HashMap<(String, String), &'static str>;

/// The mapping of the graph `info` describes, from the program's `schema`.
///
/// `natural` holds the columns the layout stored as text that are not text.
///
/// # Errors
///
/// [`WriteError::AmbiguousIri`] when the graph is not admissible by A2, and
/// [`WriteError::Mapping`] when an IRI is not one (which no compiled schema
/// holds).
pub(crate) fn describe(
    schema: &GraphSchema,
    info: &GraphInfo,
    natural: &Natural,
) -> Result<Description, WriteError> {
    admissible(schema)?;

    let mut mapping = Mapping::default();
    let mut notes = Vec::new();
    for vertex in &info.vertices {
        let Some(node) = schema.node(&vertex.r#type) else {
            continue;
        };
        vertex_maps(node, vertex, natural, &mut mapping, &mut notes)?;
    }
    for edge in &info.edges {
        let declared = schema.edges.iter().find(|e| {
            (&e.source, &e.label, &e.destination)
                == (&edge.src_type, &edge.edge_type, &edge.dst_type)
        });
        if let Some(declared) = declared {
            relation_map(declared, &edge.triple(), &mut mapping, &mut notes)?;
        }
    }
    notes.sort();
    notes.dedup();
    mapping.annotations = notes;
    mapping.validate().map_err(WriteError::Mapping)?;
    Ok(Description { mapping })
}

// ───────────────────────────────────────────────────────────────── A2

/// A2 (`/docs/format/round-trip`): distinct types have distinct class IRIs,
/// within a class distinct columns have distinct predicate IRIs, and no
/// relation's predicate is a column predicate of its source class. Each of
/// them is two things the RDF image would make one.
fn admissible(schema: &GraphSchema) -> Result<(), WriteError> {
    let ambiguous = |iri: &str, first: String, second: String| WriteError::AmbiguousIri {
        iri: iri.to_string(),
        first,
        second,
    };
    let mut classes: HashMap<&str, &str> = HashMap::new();
    for node in &schema.nodes {
        if let Some(class) = node.iri.as_deref()
            && let Some(other) = classes.insert(class, &node.label)
        {
            return Err(ambiguous(class, other.to_string(), node.label.clone()));
        }
        let mut predicates: HashMap<&str, String> = HashMap::new();
        for property in &node.properties {
            let Some(predicate) = property.iri.as_deref() else {
                continue;
            };
            let at = format!("{}.{}", node.label, property.name);
            if let Some(other) = predicates.insert(predicate, at.clone()) {
                return Err(ambiguous(predicate, other, at));
            }
        }
        for edge in schema.edges.iter().filter(|e| e.source == node.label) {
            if let Some(predicate) = edge.iri.as_deref()
                && let Some(other) = predicates.get(predicate)
            {
                return Err(ambiguous(
                    predicate,
                    other.clone(),
                    format!("{}_{}_{}", edge.source, edge.label, edge.destination),
                ));
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────── vertices

fn vertex_maps(
    node: &NodeType,
    vertex: &VertexInfo,
    natural: &Natural,
    mapping: &mut Mapping,
    notes: &mut Vec<Annotation>,
) -> Result<(), WriteError> {
    let label = node.label.as_str();
    let class = node.iri.as_deref().map(iri).transpose()?;
    if let Some(class) = &class {
        annotate(class, &node.annotations, notes)?;
    }

    // The properties the program mapped to a predicate, in the information
    // file's order: the scalars are the table's own columns, and the others are
    // lists or structs a reference cannot read, so each has a map over a query.
    let mut scalars = Vec::new();
    let mut nested = Vec::new();
    for group in &vertex.property_groups {
        for stored in &group.properties {
            if stored.name == PAYLOAD_IDENTITY {
                continue;
            }
            let Some(declared) = node.properties.iter().find(|p| p.name == stored.name) else {
                continue;
            };
            let Some(predicate) = declared.iri.as_deref() else {
                continue;
            };
            let predicate = iri(predicate)?;
            annotate(&predicate, &declared.annotations, notes)?;
            let tagged = stored.data_type == Stored::UserDefined(LANG_STRING.to_string());
            let text = natural
                .get(&(label.to_string(), stored.name.clone()))
                .copied();
            let object = if tagged {
                TermMap::reference("value").with_language(LanguageMap::Reference("lang".into()))
            } else {
                object(
                    if stored.cardinality == Several::Set {
                        "value".to_string()
                    } else {
                        reference(&stored.name)
                    },
                    declared,
                    text,
                )?
            };
            let pom = PredicateObjectMap {
                predicates: vec![TermMap::iri(predicate)],
                objects: vec![ObjectMap::Term(object)],
            };
            if tagged || stored.cardinality == Several::Set {
                nested.push((stored.name.as_str(), tagged, stored.cardinality, pom));
            } else {
                scalars.push(pom);
            }
        }
    }

    // The type itself: its class, and the scalars. A type that says neither
    // has nothing to say as RDF, and its table is still there.
    if class.is_some() || !scalars.is_empty() {
        mapping.triples_maps.push(TriplesMap {
            id: id(label),
            base_iri: None,
            logical_source: source(Iterable::Sql2008Table(label.to_string())),
            subject_map: subject(class.into_iter().collect()),
            predicate_object_maps: scalars,
        });
    }
    for (name, tagged, cardinality, pom) in nested {
        let column = quoted(name);
        let table = quoted(label);
        let query = match (cardinality, tagged) {
            (Several::Set, true) => format!(
                "SELECT subject, n.value AS value, n.lang AS lang \
                 FROM (SELECT subject, unnest({column}) AS n FROM {table})"
            ),
            (Several::Set, false) => {
                format!("SELECT subject, unnest({column}) AS value FROM {table}")
            }
            // A single-valued tagged property is a bare struct.
            _ => format!(
                "SELECT subject, {column}.value AS value, {column}.lang AS lang FROM {table}"
            ),
        };
        mapping.triples_maps.push(TriplesMap {
            id: id(&format!("{label}_{name}")),
            base_iri: None,
            logical_source: source(Iterable::Sql2008Query(query)),
            subject_map: subject(Vec::new()),
            predicate_object_maps: vec![pom],
        });
    }
    Ok(())
}

/// The object map of a scalar (or one element of a set) read from `column`:
/// the term the shape declares, and failing that the datatype the column has
/// lost by being stored as it is.
fn object(
    column: String,
    declared: &Property,
    text: Option<&'static str>,
) -> Result<TermMap, WriteError> {
    let map = TermMap::reference(column);
    Ok(match &declared.term {
        // The shape's term, verbatim: `xsd:gYear` over an `int32` stays
        // `xsd:gYear`.
        Some(Term::Iri) => map.with_term_type(TermType::Iri),
        Some(Term::Literal(Some(datatype))) => map.with_datatype(iri(datatype)?),
        Some(Term::Literal(None)) | None => {
            // None declared: the literal is the column's natural one, except
            // where the writer stored a text for a type `GraphAr` lacks.
            let xsd = match declared.datatype {
                Primitive::GYear => Some("gYear"),
                Primitive::Time => Some("time"),
                _ => text,
            };
            match xsd {
                Some(local) => map.with_datatype(iri(&format!("{XSD}{local}"))?),
                None => map,
            }
        }
    })
}

// ───────────────────────────────────────────────────────────────── relations

/// A relation: the subject and the object are the subjects of the two ends, so
/// the logical source joins the adjacency list to the end tables on the index.
fn relation_map(
    edge: &EdgeType,
    table: &str,
    mapping: &mut Mapping,
    notes: &mut Vec<Annotation>,
) -> Result<(), WriteError> {
    let Some(predicate) = edge.iri.as_deref() else {
        return Ok(());
    };
    let predicate = iri(predicate)?;
    annotate(&predicate, &edge.annotations, notes)?;
    let query = format!(
        "SELECT s.subject AS source, d.subject AS target FROM {edge} AS e \
         JOIN {src} AS s ON e.{SRC_INDEX_COL} = s.{VERTEX_INDEX_COL} \
         JOIN {dst} AS d ON e.{DST_INDEX_COL} = d.{VERTEX_INDEX_COL}",
        edge = quoted(table),
        src = quoted(&edge.source),
        dst = quoted(&edge.destination),
    );
    mapping.triples_maps.push(TriplesMap {
        id: id(table),
        base_iri: None,
        logical_source: source(Iterable::Sql2008Query(query)),
        subject_map: SubjectMap {
            term: TermMap::reference("source").with_term_type(TermType::Iri),
            classes: Vec::new(),
        },
        predicate_object_maps: vec![PredicateObjectMap {
            predicates: vec![TermMap::iri(predicate)],
            objects: vec![ObjectMap::Term(
                TermMap::reference("target").with_term_type(TermType::Iri),
            )],
        }],
    });
    Ok(())
}

// ─────────────────────────────────────────────────────────────────── pieces

/// The source of a logical source is the database, which the host reaches: an
/// anonymous `rml:Source` is all the mapping can honestly say
/// (`/docs/format/reading/rdf`).
const fn source(iterable: Iterable) -> LogicalSource {
    LogicalSource {
        source: None,
        iterable,
    }
}

fn subject(classes: Vec<IriS>) -> SubjectMap {
    SubjectMap {
        term: TermMap::reference(PAYLOAD_IDENTITY).with_term_type(TermType::Iri),
        classes,
    }
}

/// A triples map's name: a fragment of the document, never an output.
fn id(name: &str) -> Node {
    Node::Iri(IriS::new_unchecked(&format!("#{name}")))
}

fn iri(text: &str) -> Result<IriS, WriteError> {
    oxrdf::NamedNode::new(text)
        .map(|_| IriS::new_unchecked(text))
        .map_err(|e| WriteError::Internal(format!("`{text}` is not an IRI: {e}")))
}

/// A column as a reference: its name, and a SQL delimited identifier where the
/// name is not a plain one (RML-IO: a reference to a relational source is a
/// column name).
fn reference(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_string()
    } else {
        quoted(name)
    }
}

/// A name as a SQL delimited identifier, so a table or a column called `Order`
/// or `birth-year` is read as the name it is.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The annotations of a type, a column or a relation, as plain triples about
/// the IRI the mapping names for it (RML-Core §3.2).
fn annotate(
    about: &IriS,
    annotations: &[Declared],
    notes: &mut Vec<Annotation>,
) -> Result<(), WriteError> {
    for note in annotations {
        let object = match &note.object {
            AnnotationObject::Iri(object) => Constant::Iri(iri(object)?),
            AnnotationObject::Literal(literal) => Constant::Literal(Literal {
                lexical_form: literal.value.clone(),
                kind: match (&literal.language, &literal.datatype) {
                    (Some(tag), _) => LiteralKind::Language(tag.to_ascii_lowercase()),
                    (None, Some(datatype)) => LiteralKind::Datatype(iri(datatype)?),
                    (None, None) => return push_string(about, note, literal.value.clone(), notes),
                },
            }),
        };
        notes.push(Annotation {
            subject: about.clone(),
            predicate: iri(&note.predicate)?,
            object,
        });
    }
    Ok(())
}

/// A plain string, `xsd:string`.
fn push_string(
    about: &IriS,
    note: &Declared,
    value: String,
    notes: &mut Vec<Annotation>,
) -> Result<(), WriteError> {
    notes.push(Annotation {
        subject: about.clone(),
        predicate: iri(&note.predicate)?,
        object: Constant::Literal(Literal::string(value)),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use fossil_graph_schema::Cardinality;

    use super::*;

    const X: &str = "https://example.org/X";

    fn property(name: &str, iri: Option<&str>) -> Property {
        Property {
            name: name.to_string(),
            datatype: Primitive::String,
            iri: iri.map(str::to_string),
            term: None,
            cardinality: Cardinality::Single,
            annotations: Vec::new(),
            languages: Vec::new(),
        }
    }

    fn node(label: &str, iri: Option<&str>, properties: Vec<Property>) -> NodeType {
        NodeType {
            label: label.to_string(),
            iri: iri.map(str::to_string),
            properties,
            annotations: Vec::new(),
        }
    }

    fn edge(label: &str, iri: &str, source: &str) -> EdgeType {
        EdgeType {
            label: label.to_string(),
            iri: Some(iri.to_string()),
            source: source.to_string(),
            destination: source.to_string(),
            cardinality: Cardinality::Multi,
            annotations: Vec::new(),
        }
    }

    fn refusal(nodes: Vec<NodeType>, edges: Vec<EdgeType>) -> (String, String, String) {
        match admissible(&GraphSchema { nodes, edges }).expect_err("A2 refuses it") {
            WriteError::AmbiguousIri { iri, first, second } => (iri, first, second),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_types_do_not_share_a_class() {
        let (iri, first, second) = refusal(
            vec![node("A", Some(X), vec![]), node("B", Some(X), vec![])],
            vec![],
        );
        assert_eq!(
            (iri.as_str(), first.as_str(), second.as_str()),
            (X, "A", "B")
        );
    }

    #[test]
    fn a_class_does_not_give_two_columns_one_predicate() {
        let (_, first, second) = refusal(
            vec![node(
                "A",
                Some("https://example.org/A"),
                vec![property("a", Some(X)), property("b", Some(X))],
            )],
            vec![],
        );
        assert_eq!((first.as_str(), second.as_str()), ("A.a", "A.b"));
    }

    #[test]
    fn a_relation_is_not_a_column_of_its_source() {
        let (_, first, second) = refusal(
            vec![node("A", None, vec![property("a", Some(X))])],
            vec![edge("knows", X, "A")],
        );
        assert_eq!((first.as_str(), second.as_str()), ("A.a", "A_knows_A"));
    }

    #[test]
    fn what_has_no_iri_is_not_compared_and_distinct_iris_are_admitted() {
        admissible(&GraphSchema {
            nodes: vec![
                node("A", None, vec![property("a", None), property("b", None)]),
                node("B", None, vec![]),
                node(
                    "C",
                    Some(X),
                    vec![property("c", Some("https://example.org/c"))],
                ),
            ],
            edges: vec![edge("knows", "https://example.org/knows", "C")],
        })
        .expect("nothing is named twice");
    }
}
