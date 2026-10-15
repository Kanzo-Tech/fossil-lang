//! What the graph's tables mean as RDF: [`describe`] builds the RML-Core
//! mapping that `mapping.ttl` is, with rudof's `rml` AST, and [`turtle`]
//! writes it.
//!
//! **The reference is RML-Core** (Draft Community Group Report, 16 March 2026)
//! with RML-IO for the logical source, as `rml` reads them; the shape of the
//! document is `/docs/format/reading/rdf`'s and the argument for it
//! `/docs/design/graphar`'s. The graph says nothing of a class, a predicate or
//! a datatype (`GraphAr` has no word for them), so the mapping is where the
//! program's IRIs are.
//!
//! **It is a pure function of the types**, the [`GraphSchema`] a program
//! declares or imports (`fossil_hir::shapes::program_shapes`) or a run
//! produced: it reads no data and no engine, so a host asks it before any run
//! (`/docs/design/graphar`, D6). One triples map per vertex type, over the
//! table of that name (`rml:SQL2008Table`); one per relation, over a query that
//! joins the adjacency list to the two end tables on the index and reads the
//! two subjects; one per multi-valued or language-tagged property, over a query
//! that unnests the list once so each row is one element. The tables are SQL
//! names, which the host resolves, and their names and columns are the layout's
//! (`fossil-df`'s `layout`): a vertex type's table is its label, a relation's is
//! `<Src>_<label>_<Dst>`, and the columns are the properties' names beside the
//! `subject` and the three index columns.
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
//!
//! **What only a run knows** is [`Natural`]: the columns the engine produced as
//! a decimal or as bytes, which the layout stores as text and the mapping must
//! then datatype. A host that describes before a run passes none, and the
//! mapping then differs from the run's in exactly those columns.

use std::collections::HashMap;

use fossil_graph_schema::{
    Annotation as Declared, AnnotationObject, Cardinality, EdgeType, Failure, GraphSchema,
    NodeType, Primitive, Problem, Property, Term,
};
use rml::{
    Annotation, Constant, Iterable, LanguageMap, Literal, LiteralKind, LogicalSource, Mapping,
    Node, ObjectMap, PredicateObjectMap, SubjectMap, TermMap, TermType, TriplesMap,
};
use rudof_iri::IriS;

/// What a vertex is: the column of its IRI, the first of every vertex table.
pub const PAYLOAD_IDENTITY: &str = "subject";

/// `GraphAr`'s name for the column of a vertex's index within its type
/// (`graphar_parquet::VERTEX_INDEX_COL`, which `fossil-df` holds this to).
pub const VERTEX_INDEX_COL: &str = "_graphArVertexIndex";
/// `GraphAr`'s name for the column of a relation's source index
/// (`graphar_parquet::SRC_INDEX_COL`).
pub const SRC_INDEX_COL: &str = "_graphArSrcIndex";
/// `GraphAr`'s name for the column of a relation's destination index
/// (`graphar_parquet::DST_INDEX_COL`).
pub const DST_INDEX_COL: &str = "_graphArDstIndex";

/// The XSD namespace, for the datatype a column keeps that `GraphAr` has no
/// type for.
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The datatype of a column that was stored as text, by the XSD local name,
/// keyed by `(vertex type, column)`.
pub type Natural = HashMap<(String, String), &'static str>;

/// Why a mapping could not be built.
#[derive(Debug, thiserror::Error)]
pub enum DescribeError {
    /// One IRI that names two things the mapping needs it to name one of (A2).
    #[error("`{iri}` names both `{first}` and `{second}`")]
    AmbiguousIri {
        iri: String,
        first: String,
        second: String,
    },
    /// An IRI that is not one: no compiled schema holds it, so fossil's fault.
    #[error("`{0}` is not an IRI: {1}")]
    NotAnIri(String, String),
    /// The mapping is not one `rml` accepts or writes: fossil's fault.
    #[error("the mapping: {0}")]
    Mapping(rml::Error),
}

/// Two IRIs that are one, the only way a refusal reaches a user; the rest is
/// `internal/bug`, with the error as cause.
impl From<DescribeError> for Failure {
    fn from(e: DescribeError) -> Self {
        match e {
            DescribeError::AmbiguousIri { iri, first, second } => {
                Self::new(Problem::WriteAmbiguousIri { iri, first, second })
            }
            e @ DescribeError::NotAnIri(..) => Self::new(Problem::Bug {
                what: "an IRI of the schema is not one".to_string(),
            })
            .caused_by(e),
            DescribeError::Mapping(e) => Self::new(Problem::Bug {
                what: "the mapping is not valid".to_string(),
            })
            .caused_by(e),
        }
    }
}

/// The mapping of the graph `schema` describes.
///
/// `natural` holds the columns the layout stored as text that are not text; a
/// host that has not run passes `&Natural::new()`.
///
/// # Errors
///
/// [`DescribeError::AmbiguousIri`] when the graph is not admissible by A2, and
/// [`DescribeError::NotAnIri`] or [`DescribeError::Mapping`] when an IRI is not
/// one or the mapping is not valid (which no compiled schema holds).
pub fn describe(schema: &GraphSchema, natural: &Natural) -> Result<Mapping, DescribeError> {
    admissible(schema)?;

    let mut mapping = Mapping::default();
    let mut notes = Vec::new();
    for node in &schema.nodes {
        vertex_maps(node, natural, &mut mapping, &mut notes)?;
    }
    for edge in &schema.edges {
        let table = format!("{}_{}_{}", edge.source, edge.label, edge.destination);
        relation_map(edge, &table, &mut mapping, &mut notes)?;
    }
    notes.sort();
    notes.dedup();
    mapping.annotations = notes;
    mapping.validate().map_err(DescribeError::Mapping)?;
    Ok(mapping)
}

/// `mapping.ttl`: the mapping, as the Turtle document a reader parses.
///
/// # Errors
///
/// [`DescribeError::Mapping`] when `rml` cannot write it.
pub fn turtle(mapping: &Mapping) -> Result<String, DescribeError> {
    rml::rdf::write_turtle(mapping).map_err(DescribeError::Mapping)
}

// ───────────────────────────────────────────────────────────────── A2

/// A2 (`/docs/format/round-trip`): distinct types have distinct class IRIs,
/// within a class distinct columns have distinct predicate IRIs, and no
/// relation's predicate is a column predicate of its source class. Each of
/// them is two things the RDF image would make one.
fn admissible(schema: &GraphSchema) -> Result<(), DescribeError> {
    let ambiguous = |iri: &str, first: String, second: String| DescribeError::AmbiguousIri {
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
    natural: &Natural,
    mapping: &mut Mapping,
    notes: &mut Vec<Annotation>,
) -> Result<(), DescribeError> {
    let label = node.label.as_str();
    let class = node.iri.as_deref().map(iri).transpose()?;
    if let Some(class) = &class {
        annotate(class, &node.annotations, notes)?;
    }

    // The properties the program mapped to a predicate, in the table's column
    // order — the single-valued ones, then the multi-valued, as the layout
    // stores them. The scalars are the table's own columns, and the others are
    // lists or structs a reference cannot read, so each has a map over a query.
    let mut scalars = Vec::new();
    let mut nested = Vec::new();
    let single = |p: &&Property| p.cardinality == Cardinality::Single;
    let columns = node
        .properties
        .iter()
        .filter(single)
        .chain(node.properties.iter().filter(|p| !single(p)));
    for declared in columns {
        let Some(predicate) = declared.iri.as_deref() else {
            continue;
        };
        let name = declared.name.as_str();
        let predicate = iri(predicate)?;
        annotate(&predicate, &declared.annotations, notes)?;
        // A language-tagged value is a struct of the form and its tag, whatever
        // its cardinality; a set is a list of values.
        let tagged = declared.datatype == Primitive::LangString;
        let several = declared.cardinality == Cardinality::Multi;
        let text = natural.get(&(label.to_string(), name.to_string())).copied();
        let object = if tagged {
            TermMap::reference("value").with_language(LanguageMap::Reference("lang".into()))
        } else {
            object(
                if several {
                    "value".to_string()
                } else {
                    reference(name)
                },
                declared,
                text,
            )?
        };
        let pom = PredicateObjectMap {
            predicates: vec![TermMap::iri(predicate)],
            objects: vec![ObjectMap::Term(object)],
        };
        if tagged || several {
            nested.push((name, tagged, several, pom));
        } else {
            scalars.push(pom);
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
    for (name, tagged, several, pom) in nested {
        let column = quoted(name);
        let table = quoted(label);
        let query = match (several, tagged) {
            (true, true) => format!(
                "SELECT subject, n.value AS value, n.lang AS lang \
                 FROM (SELECT subject, unnest({column}) AS n FROM {table})"
            ),
            (true, false) => {
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
) -> Result<TermMap, DescribeError> {
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
) -> Result<(), DescribeError> {
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

fn iri(text: &str) -> Result<IriS, DescribeError> {
    oxrdf::NamedNode::new(text)
        .map(|_| IriS::new_unchecked(text))
        .map_err(|e| DescribeError::NotAnIri(text.to_string(), e.to_string()))
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
) -> Result<(), DescribeError> {
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
) -> Result<(), DescribeError> {
    notes.push(Annotation {
        subject: about.clone(),
        predicate: iri(&note.predicate)?,
        object: Constant::Literal(Literal::string(value)),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
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
            DescribeError::AmbiguousIri { iri, first, second } => (iri, first, second),
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
