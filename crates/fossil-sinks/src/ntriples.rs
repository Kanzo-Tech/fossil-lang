//! The N-Triples sink: the triples of a [`Graph`], one line each.
//!
//! **This is the definition of what a program means as RDF** (`⟦P⟧`, decision
//! S1/G1 of the engine split): a vertex is its subject IRI, typed by its class
//! when its type names one; a single-valued property with a predicate is one
//! triple per row that holds a value; a multi-valued one is a triple per
//! element; a relation with a predicate is a triple from the source's subject
//! to the destination's. A type, a property or a relation that names no IRI has
//! no triple, and a NULL generates none (RML-Core §12.2).
//!
//! **A literal's datatype** is the one the program's type declares, and failing
//! that the natural one of the value as the engine holds it (R2RML §10.2): an
//! integer is `xsd:integer`, a float `xsd:double`, a date `xsd:date`, a
//! timestamp `xsd:dateTime`, a decimal `xsd:decimal` and bytes `xsd:hexBinary`.
//! `GYear` and `Time` keep theirs whatever column holds them. Its lexical form is
//! the value's text, Arrow's rendering of it, with bytes in upper-case
//! hexadecimal, the canonical `xsd:hexBinary` form.
//!
//! **The `GraphAr` image means the same triples.** `mapping.ttl` read over the
//! written tables (`rml::sql`) gives this set for every conformance program,
//! comparing a numeric or temporal literal by its value: `fossil-exec`'s
//! `tests/ntriples.rs`.
//!
//! **What it refuses** is what has no RDF term: a subject or a value typed as
//! an IRI that is not an absolute IRI, and a language tag that is not BCP 47.
//! A triple written twice is one triple: the output is a set, sorted.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, UInt32Type};
use datafusion::arrow::error::ArrowError;
use fossil_graph_schema::{
    Cardinality, Failure, NodeType, Primitive, Problem, Property, Term as Declared,
};
use fossil_mir::describe::PAYLOAD_IDENTITY;
use fossil_storage::{Storage, StorageError};
use object_store::{PutOptions, PutPayload};
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{Literal, NamedNode, NamedNodeRef, Term, Triple};

use crate::{ENDPOINT_SRC, Graph, PAYLOAD_ADDRESS, PROPERTY_LANGUAGE};

/// The file the sink writes under its destination.
pub const FILE: &str = "graph.nt";

/// Failure modes of [`triples`] and [`write`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An Arrow kernel refused a column.
    #[error("arrow: {0}")]
    Arrow(#[from] ArrowError),
    /// A subject that is not an absolute IRI: it has no RDF term.
    #[error("`{vertex_type}` has the subject {subject}, which is not an IRI")]
    InvalidSubject { vertex_type: String, subject: String },
    /// An IRI the program's type names that is not one: the checker admits
    /// none, so it is fossil's fault.
    #[error("`{at}`: `{value}` is not an IRI")]
    NotAnIri { at: String, value: String },
    /// A value of a property typed as an IRI that is not one.
    #[error("`{property}` holds `{value}`, which is not an absolute IRI")]
    InvalidIri { property: String, value: String },
    /// A language tag that is not BCP 47.
    #[error("`{property}`: `{tag}` is not a language tag")]
    InvalidLanguageTag { property: String, tag: String },
    /// A column the program's type names that the engine did not produce:
    /// fossil's fault.
    #[error("`{0}` has no column in the engine's table")]
    MissingColumn(String),
    /// The destination could not be reached or written.
    #[error("`{path}`: {source}")]
    Store {
        path: String,
        source: StorageError,
    },
    /// The store refused the file.
    #[error("`{path}`: {source}")]
    Put {
        path: String,
        source: object_store::Error,
    },
}

/// The code each way the sink fails is: the data that has no RDF term has one
/// each, the store's refusal has `write/failed`, and the rest are fossil's
/// fault, `internal/bug`.
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        let bug = |what: &str| {
            Self::new(Problem::Bug {
                what: what.to_string(),
            })
        };
        match e {
            Error::InvalidSubject {
                vertex_type,
                subject,
            } => Self::new(Problem::WriteInvalidSubject {
                vertex_type,
                subject,
            }),
            Error::InvalidIri { property, value } => {
                Self::new(Problem::WriteInvalidIri { property, value })
            }
            Error::InvalidLanguageTag { property, tag } => {
                Self::new(Problem::WriteInvalidLanguageTag { property, tag })
            }
            Error::Store { path, source } => {
                Self::new(Problem::WriteFailed { path }).caused_by(Self::from(source))
            }
            Error::Put { path, source } => Self::new(Problem::WriteFailed { path: path.clone() })
                .caused_by(Self::new(Problem::Unreachable { location: path }).caused_by(source)),
            e @ Error::NotAnIri { .. } => bug("the program's type names an IRI that is not one").caused_by(e),
            e @ Error::MissingColumn(_) => bug("a column the type names is not in its table").caused_by(e),
            Error::Arrow(e) => bug("an Arrow kernel refused a column").caused_by(e),
        }
    }
}

/// The triples of `graph`, as a set.
///
/// # Errors
///
/// [`Error`] for a subject or an IRI that is not one, and a tag that is not
/// BCP 47.
pub fn triples(graph: &Graph) -> Result<HashSet<Triple>, Error> {
    let subjects = subjects(graph)?;
    let mut out = HashSet::new();
    for node in &graph.schema.nodes {
        let Some(table) = graph.vertices.iter().find(|v| v.label == node.label) else {
            continue;
        };
        let class = node
            .iri
            .as_deref()
            .map(|iri| named(&node.label, iri))
            .transpose()?;
        for batch in &table.batches {
            let rows = rows_of(batch, &subjects)?;
            if let Some(class) = &class {
                for subject in &rows {
                    out.insert(Triple::new(subject.clone(), rdf::TYPE, class.clone()));
                }
            }
            for property in node
                .properties
                .iter()
                .filter(|p| p.cardinality == Cardinality::Single)
            {
                let Some(predicate) = predicate(node, property)? else {
                    continue;
                };
                let column = batch
                    .column_by_name(&property.name)
                    .ok_or_else(|| Error::MissingColumn(at(node, property)))?;
                let objects = objects(node, property, column, None)?;
                for (subject, object) in rows.iter().zip(objects) {
                    if let Some(object) = object {
                        out.insert(Triple::new(subject.clone(), predicate.clone(), object));
                    }
                }
            }
        }
        for property in node
            .properties
            .iter()
            .filter(|p| p.cardinality == Cardinality::Multi)
        {
            let Some(predicate) = predicate(node, property)? else {
                continue;
            };
            let Some(values) = table.properties.iter().find(|p| p.name == property.name) else {
                continue;
            };
            for batch in &values.batches {
                let owners = owners(batch, &subjects)?;
                let column = batch
                    .column_by_name(&property.name)
                    .ok_or_else(|| Error::MissingColumn(at(node, property)))?;
                let tags = batch.column_by_name(PROPERTY_LANGUAGE);
                let objects = objects(node, property, column, tags)?;
                for (subject, object) in owners.iter().zip(objects) {
                    if let (Some(subject), Some(object)) = (subject, object) {
                        out.insert(Triple::new(subject.clone(), predicate.clone(), object));
                    }
                }
            }
        }
    }
    for edge in &graph.schema.edges {
        let Some(iri) = edge.iri.as_deref() else {
            continue;
        };
        let triple = format!("{}_{}_{}", edge.source, edge.label, edge.destination);
        let predicate = named(&triple, iri)?;
        let Some(table) = graph.edges.iter().find(|e| {
            (&e.src_type, &e.label, &e.dst_type) == (&edge.source, &edge.label, &edge.destination)
        }) else {
            continue;
        };
        for batch in &table.batches {
            let ends = |i: usize| -> Result<Vec<Option<NamedNode>>, Error> {
                let ids = cast(batch.column(i), &DataType::UInt32)?;
                Ok(ids
                    .as_primitive::<UInt32Type>()
                    .iter()
                    .map(|id| id.and_then(|id| subjects.get(&id).cloned()))
                    .collect())
            };
            for (s, o) in ends(0)?.into_iter().zip(ends(1)?) {
                if let (Some(s), Some(o)) = (s, o) {
                    out.insert(Triple::new(s, predicate.clone(), o));
                }
            }
        }
    }
    Ok(out)
}

/// The triples of `graph` as an N-Triples document, one line each, sorted, so
/// one graph is always the same bytes.
///
/// # Errors
///
/// What [`triples`] refuses.
pub fn document(graph: &Graph) -> Result<String, Error> {
    // `Triple`'s `Display` is the N-Triples form of the statement.
    let mut lines: Vec<String> = triples(graph)?.iter().map(|t| format!("{t} .")).collect();
    lines.sort_unstable();
    let mut text = String::new();
    for line in lines {
        let _ = writeln!(text, "{line}");
    }
    Ok(text)
}

/// Write `graph` as [`FILE`] under `dest`, in one put: nothing is visible until
/// the whole document is.
///
/// # Errors
///
/// What [`triples`] refuses, and what the store refuses.
#[tracing::instrument(skip_all, fields(dest = %dest))]
pub async fn write(graph: &Graph, storage: &Storage, dest: &str) -> Result<(), Error> {
    let text = document(graph)?;
    let (store, root) = storage.at(dest).map_err(|source| Error::Store {
        path: dest.to_string(),
        source,
    })?;
    store
        .put_opts(
            &root.clone().join(FILE),
            PutPayload::from(text.into_bytes()),
            PutOptions::default(),
        )
        .await
        .map_err(|source| Error::Put {
            path: FILE.to_string(),
            source,
        })?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────── subjects

/// Every vertex's subject, by its dense id: the key a property row and an edge
/// name it by.
fn subjects(graph: &Graph) -> Result<HashMap<u32, NamedNode>, Error> {
    let mut all = HashMap::new();
    for table in &graph.vertices {
        for batch in &table.batches {
            let ids = column(batch, PAYLOAD_ADDRESS, &DataType::UInt32)?;
            let iris = column(batch, PAYLOAD_IDENTITY, &DataType::Utf8)?;
            let (ids, iris) = (ids.as_primitive::<UInt32Type>(), iris.as_string::<i32>());
            for i in 0..batch.num_rows() {
                if ids.is_null(i) {
                    continue;
                }
                let subject = if iris.is_null(i) {
                    None
                } else {
                    NamedNode::new(iris.value(i)).ok()
                };
                let subject = subject.ok_or_else(|| Error::InvalidSubject {
                    vertex_type: table.label.clone(),
                    subject: if iris.is_null(i) {
                        "NULL".to_string()
                    } else {
                        format!("`{}`", iris.value(i))
                    },
                })?;
                all.insert(ids.value(i), subject);
            }
        }
    }
    Ok(all)
}

/// The subject of each row of a vertex batch.
fn rows_of(batch: &RecordBatch, subjects: &HashMap<u32, NamedNode>) -> Result<Vec<NamedNode>, Error> {
    let ids = column(batch, PAYLOAD_ADDRESS, &DataType::UInt32)?;
    ids.as_primitive::<UInt32Type>()
        .iter()
        .map(|id| {
            id.and_then(|id| subjects.get(&id).cloned())
                .ok_or_else(|| Error::MissingColumn(PAYLOAD_ADDRESS.to_string()))
        })
        .collect()
}

/// The subject each row of a multi-valued property's table belongs to.
fn owners(
    batch: &RecordBatch,
    subjects: &HashMap<u32, NamedNode>,
) -> Result<Vec<Option<NamedNode>>, Error> {
    let ids = column(batch, ENDPOINT_SRC, &DataType::UInt32)?;
    Ok(ids
        .as_primitive::<UInt32Type>()
        .iter()
        .map(|id| id.and_then(|id| subjects.get(&id).cloned()))
        .collect())
}

fn column(batch: &RecordBatch, name: &str, to: &DataType) -> Result<ArrayRef, Error> {
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| Error::MissingColumn(name.to_string()))?;
    Ok(cast(array, to)?)
}

// ─────────────────────────────────────────────────────────────── objects

fn at(node: &NodeType, property: &Property) -> String {
    format!("{}.{}", node.label, property.name)
}

fn named(at: &str, iri: &str) -> Result<NamedNode, Error> {
    NamedNode::new(iri).map_err(|_| Error::NotAnIri {
        at: at.to_string(),
        value: iri.to_string(),
    })
}

fn predicate(node: &NodeType, property: &Property) -> Result<Option<NamedNode>, Error> {
    property
        .iri
        .as_deref()
        .map(|iri| named(&at(node, property), iri))
        .transpose()
}

/// The object term of each row of `column`, or nothing for a NULL. A
/// language-tagged value is a struct of its form and its tag, or a form with
/// its tags in `tags` beside it.
fn objects(
    node: &NodeType,
    property: &Property,
    column: &ArrayRef,
    tags: Option<&ArrayRef>,
) -> Result<Vec<Option<Term>>, Error> {
    let at = at(node, property);
    if let Some(tagged) = column.as_struct_opt() {
        let forms = cast(tagged.column(0), &DataType::Utf8)?;
        let tags = cast(tagged.column(1), &DataType::Utf8)?;
        return language_tagged(&at, &forms, &tags, |i| !tagged.is_null(i));
    }
    if property.datatype == Primitive::LangString
        && let Some(tags) = tags
    {
        let forms = cast(column, &DataType::Utf8)?;
        let tags = cast(tags, &DataType::Utf8)?;
        return language_tagged(&at, &forms, &tags, |i| !forms.is_null(i));
    }
    let lexical = lexical(column)?;
    let lexical = lexical.as_string::<i32>();
    let datatype = datatype(property, column.data_type());
    (0..lexical.len())
        .map(|i| {
            if lexical.is_null(i) {
                return Ok(None);
            }
            let value = lexical.value(i);
            Ok(Some(match &datatype {
                None => Term::from(NamedNode::new(value).map_err(|_| Error::InvalidIri {
                    property: at.clone(),
                    value: value.to_string(),
                })?),
                Some(datatype) => Term::from(Literal::new_typed_literal(value, datatype.clone())),
            }))
        })
        .collect()
}

fn language_tagged(
    at: &str,
    forms: &ArrayRef,
    tags: &ArrayRef,
    present: impl Fn(usize) -> bool,
) -> Result<Vec<Option<Term>>, Error> {
    let (forms, tags) = (forms.as_string::<i32>(), tags.as_string::<i32>());
    (0..forms.len())
        .map(|i| {
            if !present(i) || forms.is_null(i) {
                return Ok(None);
            }
            let tag = if tags.is_null(i) { "" } else { tags.value(i) };
            let literal = Literal::new_language_tagged_literal(
                forms.value(i),
                tag.to_ascii_lowercase(),
            )
            .map_err(|_| Error::InvalidLanguageTag {
                property: at.to_string(),
                tag: tag.to_string(),
            })?;
            Ok(Some(Term::from(literal)))
        })
        .collect()
}

/// The datatype of a property's literals, or `None` for an IRI: the declared
/// term, and failing it the value's natural datatype.
fn datatype(property: &Property, column: &DataType) -> Option<NamedNode> {
    match &property.term {
        Some(Declared::Iri) => None,
        Some(Declared::Literal(Some(datatype))) => {
            Some(NamedNode::new_unchecked(datatype.as_str()))
        }
        Some(Declared::Literal(None)) | None => Some(match property.datatype {
            Primitive::GYear => xsd::G_YEAR.into_owned(),
            Primitive::Time => xsd::TIME.into_owned(),
            _ => natural(column).into_owned(),
        }),
    }
}

/// The natural RDF datatype of a value of `column` (R2RML §10.2).
fn natural(column: &DataType) -> NamedNodeRef<'static> {
    match column {
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64 => xsd::INTEGER,
        DataType::Float16 | DataType::Float32 | DataType::Float64 => xsd::DOUBLE,
        DataType::Decimal32(..)
        | DataType::Decimal64(..)
        | DataType::Decimal128(..)
        | DataType::Decimal256(..) => xsd::DECIMAL,
        DataType::Boolean => xsd::BOOLEAN,
        DataType::Date32 | DataType::Date64 => xsd::DATE,
        DataType::Time32(_) | DataType::Time64(_) => xsd::TIME,
        DataType::Timestamp(..) => xsd::DATE_TIME,
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => xsd::HEX_BINARY,
        DataType::Dictionary(_, values) => natural(values),
        _ => xsd::STRING,
    }
}

/// Each value's lexical form: its text, bytes in hexadecimal.
fn lexical(column: &ArrayRef) -> Result<ArrayRef, Error> {
    let plain = match column.data_type() {
        DataType::Dictionary(_, values) => cast(column, values)?,
        _ => Arc::clone(column),
    };
    Ok(match plain.data_type() {
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => {
            let bytes = cast(&plain, &DataType::Binary)?;
            let hex: datafusion::arrow::array::StringArray = bytes
                .as_binary::<i32>()
                .iter()
                .map(|b| {
                    b.map(|b| {
                        b.iter().fold(String::new(), |mut hex, byte| {
                            let _ = write!(hex, "{byte:02X}");
                            hex
                        })
                    })
                })
                .collect();
            Arc::new(hex)
        }
        _ => cast(&plain, &DataType::Utf8)?,
    })
}
