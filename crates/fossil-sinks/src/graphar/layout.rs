//! The graph a run holds, laid out as an Apache `GraphAr` graph: the information
//! files and the tables each chunk is cut from, before a byte is written.
//!
//! **The reference is the [`GraphAr` format specification]**, and where it is
//! silent `graphar-info` and `graphar-parquet`, which follow the C++ library.
//! The layout is `/docs/design/graphar`'s and `/docs/format`'s:
//!
//! - a vertex type is one property group, `subject` first and the type's
//!   primary key, one chunk, indices `0 … count − 1` in subject order;
//! - a relation is one `ordered_by_source` adjacency list, directed, with no
//!   property group, its rows `(src, dst)` as indices *within* their types;
//! - a multi-valued property is a property of the vertex type with
//!   `cardinality: set`, and a language-tagged value is the user-defined type
//!   `langstring` (`Struct<value, lang>`), which `GraphAr` declares through the
//!   version line and leaves the caller to store.
//!
//! **This is also where the graph is held to what its RDF image can recover**
//! (`/docs/format/round-trip`): a subject is a non-null absolute IRI, unique in
//! the whole graph (A1); a set holds no element twice, and every tag is
//! well-formed (A4); a relation holds no pair twice (A5); and a value `GraphAr`'s
//! types cannot hold is refused, not rounded. A2 is a property of the mapping,
//! which `describe` holds to it, and A6 and A7 hold by construction there.
//!
//! [`GraphAr` format specification]: https://graphar.apache.org/docs/specification/format

// `pub(crate)` and not `pub`: `unreachable_pub` holds the crate's surface to
// what it exports, and this module exports nothing.
#![allow(clippy::redundant_pub_crate)]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BinaryArray, Int64Array, ListArray, StringArray, StructArray,
    new_empty_array,
};
use datafusion::arrow::buffer::{NullBuffer, OffsetBuffer};
use datafusion::arrow::compute::{CastOptions, cast_with_options, concat, kernels::numeric::sub};
use datafusion::arrow::datatypes::{
    DataType, Field, Fields, Int64Type, Schema, TimeUnit, TimestampMillisecondType, UInt64Type,
};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::array_value_to_string;
use fossil_graph_schema::{Cardinality, NodeType, Primitive, Property as Declared};
use graphar_info::{
    AdjListType, AdjacentList, Cardinality as Several, DataType as Stored, EdgeInfo, FileType,
    GraphInfo, InfoVersion, Property, PropertyGroup, VertexInfo,
};
use graphar_parquet::{DST_INDEX_COL, SRC_INDEX_COL, UserTypes};

use fossil_mir::describe::PAYLOAD_IDENTITY;

use super::write::WriteError;
use crate::{ENDPOINT_SRC, EdgeTable, Graph, PROPERTY_LANGUAGE, PropertyValues, VertexTable};

/// The graph's name, which names its document: `fossil.graph.yml`.
const GRAPH: &str = "fossil";

/// The user-defined type of a language-tagged value, as the version line
/// declares it (`gar/v1 (langstring)`).
pub(crate) const LANG_STRING: &str = "langstring";

/// What `GraphAr` leaves to the caller: how `langstring` is stored. A struct of
/// the lexical form and the tag, neither null.
pub(crate) fn lang_string() -> DataType {
    DataType::Struct(lang_string_fields())
}

fn lang_string_fields() -> Fields {
    Fields::from(vec![
        Field::new("value", DataType::Utf8, false),
        Field::new("lang", DataType::Utf8, false),
    ])
}

/// The one user-defined type a fossil graph declares, for the writers.
pub(crate) fn user_types() -> UserTypes {
    UserTypes::from([(LANG_STRING.to_string(), lang_string())])
}

/// A vertex type, laid out: its information file, and its one logical table.
#[derive(Debug)]
pub(crate) struct Vertex {
    pub info: VertexInfo,
    /// A column per property, in the information file's order and nullability.
    pub batch: RecordBatch,
    /// The XSD datatype, by its local name, of each column that was stored as
    /// a `string` and is not one: `decimal`, `time` and `hexBinary`. `GraphAr`
    /// has no word for them, so the mapping states them
    /// (`/docs/format/conventions/datatypes`).
    pub natural: HashMap<String, &'static str>,
}

/// A relation, laid out: its information file, and its one adjacency list.
#[derive(Debug)]
pub(crate) struct Edge {
    pub info: EdgeInfo,
    /// `_graphArSrcIndex`, `_graphArDstIndex`: `int64`, indices within the end
    /// types, sorted by both.
    pub batch: RecordBatch,
    /// The vertices of the source type, which the list is aligned by.
    pub sources: u64,
    /// Rows of the relation's input that resolved no endpoint pair.
    pub dropped: u64,
}

/// Everything a run writes, before it is written.
#[derive(Debug)]
pub(crate) struct Laid {
    pub info: GraphInfo,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
}

impl Laid {
    /// The columns stored as text that are not text, by `(vertex type, column)`.
    pub(crate) fn natural(&self) -> fossil_mir::describe::Natural {
        self.vertices
            .iter()
            .flat_map(|v| {
                v.natural
                    .iter()
                    .map(|(column, xsd)| ((v.info.r#type.clone(), column.clone()), *xsd))
            })
            .collect()
    }
}

/// Lay `graph` out, refusing what `GraphAr` cannot hold or the RDF image cannot
/// recover.
pub(crate) fn layout(graph: &Graph) -> Result<Laid, WriteError> {
    let nodes = &graph.schema.nodes;
    let table = |label: &str| graph.vertices.iter().find(|v| v.label == label);

    // Each type's first dense id and row count, in the order the schema lists
    // the types: the numbering the executor gave, which the indices subtract.
    let mut ranges: HashMap<&str, (u64, u64)> = HashMap::new();
    let mut next = 0;
    for node in nodes {
        let count = table(&node.label).map_or(0, |v| rows(&v.batches));
        ranges.insert(node.label.as_str(), (next, count));
        next += count;
    }

    let subjects = subjects(graph)?;

    let mut vertices = Vec::with_capacity(nodes.len());
    for (node, subject) in nodes.iter().zip(&subjects) {
        let (first, count) = ranges[node.label.as_str()];
        vertices.push(vertex(node, table(&node.label), subject, first, count)?);
    }

    let mut edges = Vec::with_capacity(graph.schema.edges.len());
    for edge in &graph.schema.edges {
        let triple = format!("{}_{}_{}", edge.source, edge.label, edge.destination);
        let end = |label: &str| -> Result<usize, WriteError> {
            nodes
                .iter()
                .position(|n| n.label == label)
                .ok_or_else(|| WriteError::UnknownType {
                    relation: triple.clone(),
                    vertex_type: label.to_string(),
                })
        };
        let (src, dst) = (end(&edge.source)?, end(&edge.destination)?);
        let found: Option<&EdgeTable> = graph.edges.iter().find(|e| {
            (&e.src_type, &e.label, &e.dst_type) == (&edge.source, &edge.label, &edge.destination)
        });
        edges.push(relation(
            (
                &vertices[src].info,
                &subjects[src],
                ranges[edge.source.as_str()].0,
            ),
            (
                &vertices[dst].info,
                &subjects[dst],
                ranges[edge.destination.as_str()].0,
            ),
            &edge.label,
            found,
        )?);
    }

    let info = GraphInfo {
        name: GRAPH.to_string(),
        // No `prefix`: the data is under the document, which is `GraphAr`'s
        // default and the same for every reader. C++ resolves a relative one
        // against the process's working directory.
        prefix: None,
        vertices: vertices.iter().map(|v| v.info.clone()).collect(),
        edges: edges.iter().map(|e| e.info.clone()).collect(),
        labels: Vec::new(),
        version: InfoVersion::default(),
        // No `extra_info`: nothing a graph document could point at is a second
        // statement of what the directory already says.
        extra_info: Vec::new(),
    };
    Ok(Laid {
        info,
        vertices,
        edges,
    })
}

fn rows(batches: &[RecordBatch]) -> u64 {
    batches.iter().map(|b| b.num_rows() as u64).sum()
}

/// A column of every batch, as one array. `batches` may be empty, and then the
/// array is an empty one of `empty`.
fn gather(
    batches: &[RecordBatch],
    column: impl Fn(&RecordBatch) -> Option<ArrayRef>,
    empty: &DataType,
) -> Result<ArrayRef, WriteError> {
    let arrays: Vec<ArrayRef> = batches.iter().filter_map(column).collect();
    if arrays.is_empty() {
        return Ok(new_empty_array(empty));
    }
    let refs: Vec<&dyn Array> = arrays.iter().map(AsRef::as_ref).collect();
    Ok(concat(&refs)?)
}

fn exact(array: &ArrayRef, to: &DataType) -> Result<ArrayRef, WriteError> {
    let options = CastOptions {
        safe: false,
        ..CastOptions::default()
    };
    Ok(cast_with_options(array, to, &options)?)
}

// ───────────────────────────────────────────────────────────────── subjects

/// Every type's `subject` column, held to A1: non-null, an absolute IRI, and
/// the subject of one vertex in the whole graph. RDF identity is the IRI over
/// every type together, so a subject two types both mint is one resource with
/// two classes, which one vertex table cannot hold.
fn subjects(graph: &Graph) -> Result<Vec<StringArray>, WriteError> {
    let mut all = Vec::with_capacity(graph.schema.nodes.len());
    for node in &graph.schema.nodes {
        let batches = graph
            .vertices
            .iter()
            .find(|v| v.label == node.label)
            .map_or(&[][..], |v| v.batches.as_slice());
        let array = gather(
            batches,
            |b| b.column_by_name(PAYLOAD_IDENTITY).cloned(),
            &DataType::Utf8,
        )?;
        let array = exact(&array, &DataType::Utf8)?;
        all.push(array.as_string::<i32>().clone());
    }
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for (node, subjects) in graph.schema.nodes.iter().zip(&all) {
        for i in 0..subjects.len() {
            if subjects.is_null(i) || oxrdf::NamedNode::new(subjects.value(i)).is_err() {
                return Err(WriteError::InvalidSubject {
                    vertex_type: node.label.clone(),
                    subject: if subjects.is_null(i) {
                        "NULL".to_string()
                    } else {
                        format!("`{}`", subjects.value(i))
                    },
                });
            }
            if let Some(other) = seen.insert(subjects.value(i), node.label.as_str()) {
                return Err(WriteError::DuplicateValue {
                    at: if other == node.label {
                        format!("{}.{PAYLOAD_IDENTITY}", node.label)
                    } else {
                        format!(
                            "{other}.{PAYLOAD_IDENTITY} and {}.{PAYLOAD_IDENTITY}",
                            node.label
                        )
                    },
                    value: subjects.value(i).to_string(),
                });
            }
        }
    }
    Ok(all)
}

// ─────────────────────────────────────────────────────────────────── vertices

/// The one `GraphAr` type a property of `primitive` is stored as, whatever type
/// the source handed the run: a graph's schema is its program's, and does not
/// change with the files it was built from. `time` is a `string` because the
/// reference readers refuse `GraphAr`'s `time` (`graphar-parquet`'s `holds`),
/// and the mapping states `xsd:time` over it.
pub(crate) fn stored_type(primitive: Primitive) -> Stored {
    match primitive {
        Primitive::Integer => Stored::Int64,
        Primitive::YEAR => Stored::Int32,
        Primitive::Float => Stored::Double,
        Primitive::Bool => Stored::Bool,
        Primitive::DATE => Stored::Date,
        Primitive::DATE_TIME => Stored::Timestamp,
        Primitive::LANG_STRING => Stored::UserDefined(LANG_STRING.to_string()),
        Primitive::STRING | Primitive::HOUR | Primitive::Uri => Stored::String,
    }
}

/// The Arrow type a column of `primitive` is written as: [`stored_type`]'s, a
/// `timestamp` in milliseconds and without a zone, as C++ writes it.
fn arrow_type(primitive: Primitive) -> DataType {
    match stored_type(primitive) {
        Stored::Int64 => DataType::Int64,
        Stored::Int32 => DataType::Int32,
        Stored::Double => DataType::Float64,
        Stored::Bool => DataType::Boolean,
        Stored::Date => DataType::Date32,
        Stored::Timestamp => DataType::Timestamp(TimeUnit::Millisecond, None),
        Stored::UserDefined(_) => lang_string(),
        _ => DataType::Utf8,
    }
}

fn vertex(
    node: &NodeType,
    table: Option<&VertexTable>,
    subject: &StringArray,
    first: u64,
    count: u64,
) -> Result<Vertex, WriteError> {
    let label = node.label.as_str();
    let batches = table.map_or(&[][..], |t| t.batches.as_slice());

    let mut natural: HashMap<String, &'static str> = HashMap::new();
    let mut columns: Vec<(Property, ArrayRef)> = vec![(
        Property {
            name: PAYLOAD_IDENTITY.to_string(),
            data_type: Stored::String,
            is_primary: true,
            is_nullable: false,
            cardinality: Several::Single,
        },
        Arc::new(subject.clone()),
    )];

    // The single-valued columns: the executor's, after `dense_id` and
    // `subject`. A type with no rows has none to read, and the schema says them.
    let singles: Vec<(String, DataType)> = batches.first().map_or_else(
        || {
            node.properties
                .iter()
                .filter(|p| p.cardinality == Cardinality::Single)
                .map(|p| (p.name.clone(), arrow_type(p.datatype)))
                .collect()
        },
        |first| {
            first
                .schema()
                .fields()
                .iter()
                .skip(2)
                .map(|f| (f.name().clone(), f.data_type().clone()))
                .collect()
        },
    );
    for (name, arrow) in singles {
        let declared = node.properties.iter().find(|p| p.name == name);
        let array = gather(batches, |b| b.column_by_name(&name).cloned(), &arrow)?;
        let source = array.data_type().clone();
        let (data_type, array) = match (array.data_type(), declared) {
            (DataType::Struct(_), _) => tagged(&name, declared, &array)?,
            (_, Some(declared)) => stored(&format!("{label}.{name}"), declared.datatype, &array)?,
            (_, None) => {
                return Err(WriteError::Internal(format!(
                    "`{label}.{name}` is a column the type does not declare"
                )));
            }
        };
        if data_type == Stored::String
            && let Some(xsd) = text_of(&source)
        {
            natural.insert(name.clone(), xsd);
        }
        columns.push((
            Property {
                name,
                data_type,
                is_primary: false,
                // Written for every property that is not the key: `GraphAr`'s
                // default is nullable and fossil's is not. A column is nullable
                // when it holds a null, so the information file says what the
                // chunk holds.
                is_nullable: array.null_count() > 0,
                cardinality: Several::Single,
            },
            array,
        ));
    }
    for declared in node
        .properties
        .iter()
        .filter(|p| p.cardinality == Cardinality::Multi)
    {
        let values = table.and_then(|t| t.properties.iter().find(|p| p.name == declared.name));
        let column = set(label, declared, values, first, count)?;
        if let Some(xsd) = column.2 {
            natural.insert(declared.name.clone(), xsd);
        }
        columns.push((column.0, column.1));
    }

    let schema = Schema::new(
        columns
            .iter()
            .map(|(p, a)| Field::new(&p.name, a.data_type().clone(), p.is_nullable))
            .collect::<Vec<_>>(),
    );
    let batch = RecordBatch::try_new(
        Arc::new(schema),
        columns.iter().map(|(_, a)| Arc::clone(a)).collect(),
    )?;
    let properties: Vec<Property> = columns.into_iter().map(|(p, _)| p).collect();
    let tagged = properties
        .iter()
        .any(|p| p.data_type == Stored::UserDefined(LANG_STRING.to_string()));
    let group = PropertyGroup {
        // `GraphAr`'s own default for a group's prefix: the names, joined by `_`.
        prefix: format!(
            "{}/",
            properties
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join("_")
        ),
        properties,
        file_type: FileType::Parquet,
    };
    Ok(Vertex {
        info: VertexInfo {
            r#type: node.label.clone(),
            // One chunk: the chunk is as long as the table.
            chunk_size: count.max(1),
            prefix: format!("vertex/{label}/"),
            labels: Vec::new(),
            property_groups: vec![group],
            version: InfoVersion {
                version: 1,
                user_define_types: if tagged {
                    vec![LANG_STRING.to_string()]
                } else {
                    Vec::new()
                },
            },
        },
        batch,
        natural,
    })
}

/// The XSD datatype a column of `data_type` has, when [`stored`] writes it as a
/// `string`: the type the engine reads back is `VARCHAR`, and the natural
/// mapping (R2RML §10.2) would make `xsd:string` of it.
fn text_of(data_type: &DataType) -> Option<&'static str> {
    match data_type {
        DataType::Time32(_) | DataType::Time64(_) => Some("time"),
        DataType::Decimal32(..)
        | DataType::Decimal64(..)
        | DataType::Decimal128(..)
        | DataType::Decimal256(..) => Some("decimal"),
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => Some("hexBinary"),
        DataType::Dictionary(_, values) => text_of(values),
        _ => None,
    }
}

/// A column of structs as the user-defined type `langstring`: the lexical form
/// and its tag, the tag held to BCP 47 and to the shape's languages and
/// written in lower case. A struct row that is null stays null.
fn tagged(
    name: &str,
    declared: Option<&Declared>,
    array: &ArrayRef,
) -> Result<(Stored, ArrayRef), WriteError> {
    let unrepresentable = || WriteError::Unrepresentable {
        property: name.to_string(),
        datatype: array.data_type().to_string(),
        value: String::new(),
    };
    let tagged = array.as_struct_opt().ok_or_else(unrepresentable)?;
    if declared.map(|d| d.datatype) != Some(Primitive::LANG_STRING) || tagged.num_columns() != 2 {
        return Err(unrepresentable());
    }
    let form = exact(tagged.column(0), &DataType::Utf8)?;
    let tags = exact(tagged.column(1), &DataType::Utf8)?;
    let tags = tags.as_string::<i32>();
    let languages = declared.map_or(&[][..], |d| d.languages.as_slice());
    hold_tags(name, languages, tags, |i| !tagged.is_null(i))?;
    let lang = lowered(tags);
    let value = StructArray::try_new(
        lang_string_fields(),
        vec![form, lang],
        tagged.nulls().cloned(),
    )?;
    Ok((
        Stored::UserDefined(LANG_STRING.to_string()),
        Arc::new(value),
    ))
}

/// Every tag of `tags` whose row `present` holds, held to BCP 47 (RFC 5646
/// §2.2.9) and to the languages its shape admits. A tag read from the data is
/// only seen by the run, and a bad one refuses it: RML's fallback would make an
/// untagged string of a value in an `rdf:langString` column
/// (`/docs/design/labels`), and a NULL or empty tag is the same case.
fn hold_tags(
    property: &str,
    languages: &[String],
    tags: &StringArray,
    present: impl Fn(usize) -> bool,
) -> Result<(), WriteError> {
    for i in (0..tags.len()).filter(|&i| present(i)) {
        let tag = if tags.is_null(i) { "" } else { tags.value(i) };
        if !fossil_hir::terms::well_formed_language_tag(tag) {
            return Err(WriteError::InvalidLanguageTag {
                property: property.to_string(),
                tag: tag.to_string(),
            });
        }
        if !fossil_graph_schema::language_matches(tag, languages) {
            return Err(WriteError::LanguageNotAllowed {
                property: property.to_string(),
                tag: tag.to_string(),
                languages: languages.to_vec(),
            });
        }
    }
    Ok(())
}

/// Tags in lower case: RDF 1.1 Concepts §3.3 writes language tags in lower case,
/// and RDF 1.2 §3.4 compares them ASCII case-insensitively. A well-formed tag is
/// ASCII.
fn lowered(tags: &StringArray) -> ArrayRef {
    Arc::new(
        tags.iter()
            .map(|t| t.map(str::to_ascii_lowercase))
            .collect::<StringArray>(),
    )
}

/// A multi-valued property as a column of the vertex table: for each vertex, the
/// set of its values, in a `LIST`. A vertex with no value holds NULL, which
/// reads back as the empty set it is: an empty set, a NULL one and an absent
/// column make the same triples (`/docs/format/conventions/datatypes`).
///
/// The executor's rows are `(src, value[, lang])` sorted by `src` and then by
/// value; `src` is the dense id of the vertex, `first` the type's first.
fn set(
    label: &str,
    declared: &Declared,
    values: Option<&PropertyValues>,
    first: u64,
    count: u64,
) -> Result<(Property, ArrayRef, Option<&'static str>), WriteError> {
    let name = declared.name.as_str();
    let at = format!("{label}.{name}");
    let batches = values.map_or(&[][..], |v| v.batches.as_slice());
    let is_tagged = declared.datatype == Primitive::LANG_STRING;

    let src = gather(
        batches,
        |b| b.column_by_name(ENDPOINT_SRC).cloned(),
        &DataType::UInt32,
    )?;
    let src = exact(&src, &DataType::UInt32)?;
    let src = src.as_primitive::<datafusion::arrow::datatypes::UInt32Type>();
    let elements = gather(
        batches,
        |b| b.column_by_name(name).cloned(),
        &if is_tagged {
            DataType::Utf8
        } else {
            arrow_type(declared.datatype)
        },
    )?;

    // Offsets: the rows of each vertex are a run of `src`.
    let mut offsets = Vec::with_capacity(usize::try_from(count).unwrap_or_default() + 1);
    offsets.push(0_i32);
    let mut position = 0;
    for vertex in 0..count {
        while position < src.len() && u64::from(src.value(position)) == first + vertex {
            position += 1;
        }
        offsets.push(
            i32::try_from(position).map_err(|_| WriteError::Unrepresentable {
                property: at.clone(),
                datatype: "list".to_string(),
                value: format!("{position} values"),
            })?,
        );
    }
    if position != src.len() {
        return Err(WriteError::Internal(format!(
            "the values of `{at}` are not sorted by the vertex they belong to"
        )));
    }

    let natural = (stored_type(declared.datatype) == Stored::String)
        .then(|| text_of(elements.data_type()))
        .flatten();
    let (data_type, elements) = if is_tagged {
        let form = exact(&elements, &DataType::Utf8)?;
        let lang = gather(
            batches,
            |b| b.column_by_name(PROPERTY_LANGUAGE).cloned(),
            &DataType::Utf8,
        )?;
        let lang = exact(&lang, &DataType::Utf8)?;
        let tags = lang.as_string::<i32>();
        hold_tags(name, &declared.languages, tags, |_| true)?;
        let lang = lowered(tags);
        // A tagged element is the pair, tag lower-cased.
        distinct(&at, &offsets, |i| {
            Some(format!(
                "\"{}\"@{}",
                form.as_string::<i32>().value(i),
                lang.as_string::<i32>().value(i)
            ))
        })?;
        (
            Stored::UserDefined(LANG_STRING.to_string()),
            Arc::new(StructArray::try_new(
                lang_string_fields(),
                vec![form, lang],
                None,
            )?) as ArrayRef,
        )
    } else {
        let (data_type, array) = stored(&at, declared.datatype, &elements)?;
        distinct(&at, &offsets, |i| array_value_to_string(&array, i).ok())?;
        (data_type, array)
    };

    let valid: Vec<bool> = offsets.windows(2).map(|w| w[1] > w[0]).collect();
    let nullable = valid.iter().any(|v| !v);
    let item = Arc::new(Field::new("item", elements.data_type().clone(), true));
    let list = ListArray::try_new(
        item,
        OffsetBuffer::new(offsets.into()),
        elements,
        nullable.then(|| NullBuffer::from(valid)),
    )?;
    Ok((
        Property {
            name: name.to_string(),
            data_type,
            is_primary: false,
            is_nullable: nullable,
            cardinality: Several::Set,
        },
        Arc::new(list),
        natural,
    ))
}

/// Refuse a set that holds an element twice, or a NULL one (A4). `key` answers
/// the RDF term of element `i`, or nothing for a NULL.
fn distinct(
    at: &str,
    offsets: &[i32],
    key: impl Fn(usize) -> Option<String>,
) -> Result<(), WriteError> {
    for run in offsets.windows(2) {
        let (from, to) = (
            usize::try_from(run[0]).unwrap_or_default(),
            usize::try_from(run[1]).unwrap_or_default(),
        );
        let mut seen = HashSet::with_capacity(to - from);
        for i in from..to {
            let Some(term) = key(i) else {
                return Err(WriteError::DuplicateValue {
                    at: at.to_string(),
                    value: "NULL".to_string(),
                });
            };
            if !seen.insert(term.clone()) {
                return Err(WriteError::DuplicateValue {
                    at: at.to_string(),
                    value: term,
                });
            }
        }
    }
    Ok(())
}

// ───────────────────────────────────────────────────────────────────── values

/// A column as `GraphAr` stores it: `GraphAr`'s built-in types are `bool`,
/// `int32`, `int64`, `float`, `double`, `string`, `date` and `timestamp`, and a
/// value is written as the one that holds it without loss
/// (`/docs/format/conventions/datatypes`):
///
/// - `int8`, `int16`, `uint8`, `uint16` widen to `int32`; `uint32` to `int64`;
///   `uint64` is `int64`, and a value above `i64::MAX` is refused.
/// - a timestamp is milliseconds, and one with a sub-millisecond part is
///   refused: a string would lose the type every other reader sees.
/// - `time`, `binary` (hexadecimal) and `decimal` are a `string`, so a graph
///   without a tagged property stays within what the reference readers parse.
///
/// `at` names the property for the refusal.
fn held(at: &str, array: &ArrayRef) -> Result<(Stored, ArrayRef), WriteError> {
    let unrepresentable = |value: String| WriteError::Unrepresentable {
        property: at.to_string(),
        datatype: array.data_type().to_string(),
        value,
    };
    Ok(match array.data_type() {
        DataType::Boolean => (Stored::Bool, Arc::clone(array)),
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::UInt8 | DataType::UInt16 => {
            (Stored::Int32, exact(array, &DataType::Int32)?)
        }
        DataType::Int64 => (Stored::Int64, Arc::clone(array)),
        DataType::UInt32 => (Stored::Int64, exact(array, &DataType::Int64)?),
        DataType::UInt64 => {
            let largest = datafusion::arrow::compute::max(array.as_primitive::<UInt64Type>());
            if let Some(largest) = largest.filter(|m| i64::try_from(*m).is_err()) {
                return Err(unrepresentable(largest.to_string()));
            }
            (Stored::Int64, exact(array, &DataType::Int64)?)
        }
        DataType::Float16 | DataType::Float32 => (Stored::Float, exact(array, &DataType::Float32)?),
        DataType::Float64 => (Stored::Double, Arc::clone(array)),
        DataType::Utf8 => (Stored::String, Arc::clone(array)),
        DataType::LargeUtf8 | DataType::Utf8View => {
            (Stored::String, exact(array, &DataType::Utf8)?)
        }
        DataType::Date32 => (Stored::Date, Arc::clone(array)),
        DataType::Date64 => (Stored::Date, exact(array, &DataType::Date32)?),
        DataType::Timestamp(unit, zone) => {
            let finer = match unit {
                TimeUnit::Second | TimeUnit::Millisecond => None,
                TimeUnit::Microsecond => Some(1_000),
                TimeUnit::Nanosecond => Some(1_000_000),
            };
            if let Some(per_millisecond) = finer {
                let ticks = exact(array, &DataType::Int64)?;
                let ticks = ticks.as_primitive::<Int64Type>();
                if let Some(i) = ticks
                    .iter()
                    .position(|t| t.is_some_and(|t| t % per_millisecond != 0))
                {
                    return Err(unrepresentable(
                        array_value_to_string(array, i).unwrap_or_default(),
                    ));
                }
            }
            // The instant, without its zone: `timestamp` is milliseconds
            // since the epoch, and C++ writes it without one.
            let millis = exact(
                array,
                &DataType::Timestamp(TimeUnit::Millisecond, zone.clone()),
            )?;
            (
                Stored::Timestamp,
                Arc::new(
                    millis
                        .as_primitive::<TimestampMillisecondType>()
                        .clone()
                        .with_timezone_opt(None::<String>),
                ),
            )
        }
        DataType::Time32(_)
        | DataType::Time64(_)
        | DataType::Decimal32(..)
        | DataType::Decimal64(..)
        | DataType::Decimal128(..)
        | DataType::Decimal256(..)
        | DataType::Null => (Stored::String, exact(array, &DataType::Utf8)?),
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => {
            let bytes = exact(array, &DataType::Binary)?;
            let hex: StringArray = bytes
                .as_any()
                .downcast_ref::<BinaryArray>()
                .into_iter()
                .flat_map(BinaryArray::iter)
                .map(|b| {
                    b.map(|b| {
                        b.iter().fold(String::new(), |mut hex, byte| {
                            let _ = write!(hex, "{byte:02x}");
                            hex
                        })
                    })
                })
                .collect();
            (Stored::String, Arc::new(hex))
        }
        // A dictionary is written as its values, never its keys: besides
        // `GraphAr` naming no dictionary type, parquet-rs counts a dictionary
        // array's `distinct_count` over its keys (apache/arrow-rs#11172), and
        // that count is what a reader takes for the property's.
        DataType::Dictionary(_, values) => {
            let plain = exact(array, values)?;
            return held(at, &plain);
        }
        _ => return Err(unrepresentable(String::new())),
    })
}

/// A column of `primitive`, as [`stored_type`] stores it. The values are first
/// held as [`held`] says; a value the source typed otherwise — an `int32` for an
/// `Integer`, a `decimal` for a `Float` — is converted only when it survives the
/// round trip, so an `Integer` is always an `int64` and a decimal with more
/// digits than a `double` keeps is refused, not rounded.
fn stored(
    at: &str,
    primitive: Primitive,
    array: &ArrayRef,
) -> Result<(Stored, ArrayRef), WriteError> {
    let want = stored_type(primitive);
    let (have, value) = held(at, array)?;
    if have == want {
        return Ok((want, value));
    }
    let converted = exact(array, &arrow_type(primitive))?;
    let back = exact(&converted, array.data_type())?;
    if let Some(i) = (0..array.len())
        .find(|&i| array_value_to_string(array, i).ok() != array_value_to_string(&back, i).ok())
    {
        return Err(WriteError::Unrepresentable {
            property: at.to_string(),
            datatype: array.data_type().to_string(),
            value: array_value_to_string(array, i).unwrap_or_default(),
        });
    }
    Ok((want, converted))
}

// ───────────────────────────────────────────────────────────────── relations

/// A relation: `(src, dst)` as indices within the end types, sorted by both as
/// the executor sorted them by dense id. A pair twice is a parallel edge, which
/// an RDF graph (a set of triples) cannot hold (A5).
fn relation(
    (src_info, src_subjects, src_first): (&VertexInfo, &StringArray, u64),
    (dst_info, dst_subjects, dst_first): (&VertexInfo, &StringArray, u64),
    label: &str,
    table: Option<&EdgeTable>,
) -> Result<Edge, WriteError> {
    let batches = table.map_or(&[][..], |t| t.batches.as_slice());
    let index = |position: usize, first: u64| -> Result<Int64Array, WriteError> {
        let ids = gather(
            batches,
            |b| Some(Arc::clone(b.column(position))),
            &DataType::UInt32,
        )?;
        let ids = exact(&ids, &DataType::Int64)?;
        let first = Int64Array::new_scalar(i64::try_from(first).unwrap_or(i64::MAX));
        let within = sub(&ids, &first)?;
        Ok(within.as_primitive::<Int64Type>().clone())
    };
    let (src, dst) = (index(0, src_first)?, index(1, dst_first)?);

    let pair = |i: usize| (src.value(i), dst.value(i));
    if let Some(i) = (1..src.len()).find(|&i| pair(i) == pair(i - 1)) {
        let name = |subjects: &StringArray, id: i64| {
            subjects
                .value(usize::try_from(id).unwrap_or_default())
                .to_string()
        };
        return Err(WriteError::DuplicateValue {
            at: format!("{}_{label}_{}", src_info.r#type, dst_info.r#type),
            value: format!(
                "{} -> {}",
                name(src_subjects, src.value(i)),
                name(dst_subjects, dst.value(i))
            ),
        });
    }

    let edges = src.len() as u64;
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new(SRC_INDEX_COL, DataType::Int64, false),
            Field::new(DST_INDEX_COL, DataType::Int64, false),
        ])),
        vec![Arc::new(src), Arc::new(dst)],
    )?;
    let info = EdgeInfo {
        src_type: src_info.r#type.clone(),
        edge_type: label.to_string(),
        dst_type: dst_info.r#type.clone(),
        chunk_size: edges.max(1),
        // The chunk sizes of the end types, which the offset table aligns with.
        src_chunk_size: src_info.chunk_size,
        dst_chunk_size: dst_info.chunk_size,
        directed: true,
        prefix: format!("edge/{}_{label}_{}/", src_info.r#type, dst_info.r#type),
        adj_lists: vec![AdjacentList {
            r#type: AdjListType::OrderedBySource,
            file_type: FileType::Parquet,
            prefix: "ordered_by_source/".to_string(),
        }],
        // The language has no way to put a property on an edge.
        property_groups: Vec::new(),
        version: InfoVersion::default(),
    };
    Ok(Edge {
        info,
        batch,
        sources: src_subjects.len() as u64,
        dropped: table.map_or(0, |t| t.dropped),
    })
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::array::{
        Decimal128Array, Int32Array, StringArray, TimestampMillisecondArray,
        TimestampNanosecondArray, UInt32Array, UInt64Array,
    };
    use fossil_graph_schema::{EdgeType, GraphSchema};

    use super::*;

    fn property(name: &str, datatype: Primitive, cardinality: Cardinality) -> Declared {
        Declared {
            name: name.to_string(),
            datatype,
            iri: None,
            term: None,
            cardinality,
            annotations: Vec::new(),
            languages: Vec::new(),
        }
    }

    fn node(label: &str, properties: Vec<Declared>) -> NodeType {
        NodeType {
            label: label.to_string(),
            iri: None,
            properties,
            annotations: Vec::new(),
        }
    }

    /// A vertex table of `dense_id` from `first`, `subject`, and one column each.
    fn vertices(
        label: &str,
        first: u32,
        subjects: &[&str],
        columns: Vec<(&str, ArrayRef)>,
    ) -> VertexTable {
        let ids: ArrayRef = Arc::new(UInt32Array::from_iter_values(
            first..first + u32::try_from(subjects.len()).unwrap(),
        ));
        let mut fields = vec![
            Field::new("dense_id", DataType::UInt32, false),
            Field::new("subject", DataType::Utf8, false),
        ];
        let mut arrays = vec![
            ids,
            Arc::new(StringArray::from(subjects.to_vec())) as ArrayRef,
        ];
        for (name, array) in columns {
            fields.push(Field::new(name, array.data_type().clone(), true));
            arrays.push(array);
        }
        VertexTable {
            label: label.to_string(),
            batches: vec![RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()],
            derived_from: Vec::new(),
            properties: Vec::new(),
        }
    }

    fn graph(nodes: Vec<NodeType>, vertices: Vec<VertexTable>) -> Graph {
        Graph {
            schema: GraphSchema {
                nodes,
                edges: Vec::new(),
            },
            vertices,
            edges: Vec::new(),
        }
    }

    fn refusal(graph: &Graph) -> WriteError {
        layout(graph).expect_err("the graph is refused")
    }

    const A: &str = "https://example.org/a";

    #[test]
    fn a_subject_that_is_not_an_absolute_iri_is_refused() {
        let g = graph(
            vec![node("T", vec![])],
            vec![vertices(
                "T",
                0,
                &["https://example.org/with space"],
                vec![],
            )],
        );
        assert!(matches!(
            refusal(&g),
            WriteError::InvalidSubject { vertex_type, .. } if vertex_type == "T"
        ));
        let g = graph(
            vec![node("T", vec![])],
            vec![vertices("T", 0, &["relative/path"], vec![])],
        );
        assert!(matches!(refusal(&g), WriteError::InvalidSubject { .. }));
    }

    #[test]
    fn a_subject_is_unique_in_the_whole_graph_not_only_in_its_type() {
        let g = graph(
            vec![node("T", vec![]), node("U", vec![])],
            vec![
                vertices("T", 0, &[A], vec![]),
                vertices("U", 1, &[A], vec![]),
            ],
        );
        match refusal(&g) {
            WriteError::DuplicateValue { at, value } => {
                assert_eq!(at, "T.subject and U.subject");
                assert_eq!(value, A);
            }
            other => panic!("{other:?}"),
        }
        let g = graph(
            vec![node("T", vec![])],
            vec![vertices("T", 0, &[A, A], vec![])],
        );
        assert!(matches!(
            refusal(&g),
            WriteError::DuplicateValue { at, .. } if at == "T.subject"
        ));
    }

    #[test]
    fn an_unsigned_64_bit_integer_above_i64_max_is_refused_and_below_it_widens() {
        let big: ArrayRef = Arc::new(UInt64Array::from(vec![
            u64::try_from(i64::MAX).unwrap() + 1,
        ]));
        let g = graph(
            vec![node(
                "T",
                vec![property("n", Primitive::Integer, Cardinality::Single)],
            )],
            vec![vertices("T", 0, &[A], vec![("n", big)])],
        );
        assert!(matches!(
            refusal(&g),
            WriteError::Unrepresentable { property, datatype, .. }
                if property == "T.n" && datatype == "UInt64"
        ));
        let small: ArrayRef = Arc::new(UInt64Array::from(vec![7_u64]));
        let g = graph(
            vec![node(
                "T",
                vec![property("n", Primitive::Integer, Cardinality::Single)],
            )],
            vec![vertices("T", 0, &[A], vec![("n", small)])],
        );
        let laid = layout(&g).expect("a value below i64::MAX is written");
        assert_eq!(
            laid.vertices[0].info.property_groups[0].properties[1].data_type,
            Stored::Int64
        );
    }

    #[test]
    fn a_timestamp_finer_than_a_millisecond_is_refused_and_a_whole_one_is_written() {
        let finer: ArrayRef = Arc::new(TimestampNanosecondArray::from(vec![1_000_000_001_i64]));
        let g = graph(
            vec![node(
                "T",
                vec![property("at", Primitive::DATE_TIME, Cardinality::Single)],
            )],
            vec![vertices("T", 0, &[A], vec![("at", finer)])],
        );
        assert!(matches!(refusal(&g), WriteError::Unrepresentable { .. }));
        let whole: ArrayRef = Arc::new(TimestampNanosecondArray::from(vec![1_000_000_000_i64]));
        let g = graph(
            vec![node(
                "T",
                vec![property("at", Primitive::DATE_TIME, Cardinality::Single)],
            )],
            vec![vertices("T", 0, &[A], vec![("at", whole)])],
        );
        let laid = layout(&g).expect("a whole number of milliseconds is written");
        let p = &laid.vertices[0].info.property_groups[0].properties[1];
        assert_eq!(p.data_type, Stored::Timestamp);
        assert_eq!(
            laid.vertices[0].batch.column(1).data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
    }

    /// The one-column graph `T.v` of `primitive`, holding `values`.
    fn one(primitive: Primitive, values: ArrayRef) -> Graph {
        graph(
            vec![node(
                "T",
                vec![property("v", primitive, Cardinality::Single)],
            )],
            vec![vertices("T", 0, &[A], vec![("v", values)])],
        )
    }

    /// The type a property is stored as is its primitive's, whatever type the
    /// source handed the run: an `Integer` is an `int64` from an `int32`.
    #[test]
    fn a_property_is_stored_as_its_primitive_not_as_its_source() {
        let small: ArrayRef = Arc::new(Int32Array::from(vec![7]));
        let laid = layout(&one(Primitive::Integer, small)).expect("an int32 widens");
        let p = &laid.vertices[0].info.property_groups[0].properties[1];
        assert_eq!(p.data_type, Stored::Int64);
        assert_eq!(
            laid.vertices[0].batch.column(1).data_type(),
            &DataType::Int64
        );
        for primitive in Primitive::ALL {
            assert_eq!(
                arrow_type(primitive) == lang_string(),
                stored_type(primitive) == Stored::UserDefined(LANG_STRING.to_string()),
                "{primitive}"
            );
        }
    }

    /// A decimal under a `Float` is a `double` when the `double` holds it, and
    /// refused when it would be rounded.
    #[test]
    fn a_decimal_is_a_double_only_when_nothing_is_rounded() {
        let decimal = |v: i128| -> ArrayRef {
            Arc::new(
                Decimal128Array::from(vec![v])
                    .with_precision_and_scale(38, 2)
                    .unwrap(),
            )
        };
        let laid = layout(&one(Primitive::Float, decimal(1_050))).expect("10.50 is a double");
        let p = &laid.vertices[0].info.property_groups[0].properties[1];
        assert_eq!(p.data_type, Stored::Double);
        assert!(
            laid.natural().is_empty(),
            "a double is xsd:double, not text"
        );
        assert!(matches!(
            refusal(&one(Primitive::Float, decimal(12_345_678_901_234_567_891))),
            WriteError::Unrepresentable { property, .. } if property == "T.v"
        ));
    }

    /// A zoned instant is written as the instant, without its zone.
    #[test]
    fn a_timestamp_is_written_without_its_zone() {
        let zoned: ArrayRef =
            Arc::new(TimestampMillisecondArray::from(vec![1_000_i64]).with_timezone("+01:00"));
        let laid = layout(&one(Primitive::DATE_TIME, zoned)).expect("written");
        let column = laid.vertices[0].batch.column(1);
        assert_eq!(
            column.data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
        assert_eq!(
            column.as_primitive::<TimestampMillisecondType>().value(0),
            1_000,
            "the same instant"
        );
    }

    /// A multi-valued `nickname` of `T`: `(src, value)` rows, with a tag when
    /// `tags` has them.
    fn with_nicknames(values: &[(u32, &str)], tags: Option<&[&str]>) -> Graph {
        let mut fields = vec![
            Field::new("src", DataType::UInt32, false),
            Field::new("nickname", DataType::Utf8, true),
        ];
        let mut arrays: Vec<ArrayRef> = vec![
            Arc::new(values.iter().map(|(s, _)| *s).collect::<UInt32Array>()),
            Arc::new(StringArray::from_iter_values(
                values.iter().map(|(_, v)| *v),
            )),
        ];
        if let Some(tags) = tags {
            fields.push(Field::new("lang", DataType::Utf8, true));
            arrays.push(Arc::new(StringArray::from(tags.to_vec())));
        }
        let datatype = if tags.is_some() {
            Primitive::LANG_STRING
        } else {
            Primitive::STRING
        };
        let mut table = vertices("T", 0, &[A], vec![]);
        table.properties.push(PropertyValues {
            name: "nickname".to_string(),
            batches: vec![RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()],
            derived_from: Vec::new(),
        });
        graph(
            vec![node(
                "T",
                vec![property("nickname", datatype, Cardinality::Multi)],
            )],
            vec![table],
        )
    }

    #[test]
    fn a_set_holds_no_element_twice() {
        match refusal(&with_nicknames(&[(0, "ada"), (0, "ada")], None)) {
            WriteError::DuplicateValue { at, value } => {
                assert_eq!(at, "T.nickname");
                assert_eq!(value, "ada");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_tagged_elements_are_one_term_when_the_tags_differ_only_in_case() {
        match refusal(&with_nicknames(
            &[(0, "cat"), (0, "cat")],
            Some(&["en", "EN"]),
        )) {
            WriteError::DuplicateValue { value, .. } => assert_eq!(value, "\"cat\"@en"),
            other => panic!("{other:?}"),
        }
        // A different tag is a different term.
        let laid = layout(&with_nicknames(
            &[(0, "cat"), (0, "cat")],
            Some(&["en", "es"]),
        ))
        .expect("two tags are two terms");
        let p = &laid.vertices[0].info.property_groups[0].properties[1];
        assert_eq!(p.data_type, Stored::UserDefined("langstring".to_string()));
        assert_eq!(p.cardinality, Several::Set);
        assert_eq!(
            laid.vertices[0].info.version.to_string(),
            "gar/v1 (langstring)"
        );
    }

    #[test]
    fn a_tag_is_lower_cased_and_held_to_bcp_47() {
        let laid =
            layout(&with_nicknames(&[(0, "colour")], Some(&["en-GB"]))).expect("well-formed");
        let list = laid.vertices[0].batch.column(1).as_list::<i32>();
        let tags = list
            .values()
            .as_struct()
            .column(1)
            .as_string::<i32>()
            .clone();
        assert_eq!(tags.value(0), "en-gb");
        for bad in ["abcdefghi", ""] {
            assert!(matches!(
                refusal(&with_nicknames(&[(0, "x")], Some(&[bad]))),
                WriteError::InvalidLanguageTag { .. }
            ));
        }
    }

    #[test]
    fn a_relation_holds_no_pair_twice() {
        let mut g = graph(
            vec![node("T", vec![])],
            vec![vertices(
                "T",
                0,
                &["https://example.org/a", "https://example.org/b"],
                vec![],
            )],
        );
        g.schema.edges.push(EdgeType {
            label: "knows".to_string(),
            iri: None,
            source: "T".to_string(),
            destination: "T".to_string(),
            cardinality: Cardinality::Multi,
            annotations: Vec::new(),
        });
        let pairs = |rows: &[(u32, u32)]| EdgeTable {
            label: "knows".to_string(),
            src_type: "T".to_string(),
            dst_type: "T".to_string(),
            batches: vec![
                RecordBatch::try_new(
                    Arc::new(Schema::new(vec![
                        Field::new("src_dense", DataType::UInt32, false),
                        Field::new("dst_dense", DataType::UInt32, false),
                    ])),
                    vec![
                        Arc::new(UInt32Array::from_iter_values(rows.iter().map(|r| r.0))),
                        Arc::new(UInt32Array::from_iter_values(rows.iter().map(|r| r.1))),
                    ],
                )
                .unwrap(),
            ],
            derived_from: Vec::new(),
            dropped: 0,
        };
        g.edges.push(pairs(&[(0, 1), (1, 0)]));
        let laid = layout(&g).expect("distinct pairs");
        assert_eq!(laid.edges[0].info.triple(), "T_knows_T");
        assert_eq!(laid.edges[0].sources, 2);

        g.edges[0] = pairs(&[(0, 1), (0, 1)]);
        match refusal(&g) {
            WriteError::DuplicateValue { at, value } => {
                assert_eq!(at, "T_knows_T");
                assert_eq!(value, "https://example.org/a -> https://example.org/b");
            }
            other => panic!("{other:?}"),
        }
    }
}
