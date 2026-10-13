//! The corpus, written: [`write`](crate::write::write) writes one Parquet per
//! vertex type, per relation and per multi-valued property, then `fossil.json`.
//!
//! **The corpus carries the graph, not a picture.** Where a vertex is drawn is
//! the view's choice — two columns it binds, or its own simulation — so the
//! writer places nothing and computes no community. `/docs/design/position`
//! has the argument and `/docs/design/discarded` the nine placements before it.
//!
//! **`fossil.json` goes last, and that is the commit.** Every table is in the
//! store before the document that names it, so a reader that finds the
//! manifest finds everything it lists; a write that fails half way leaves
//! Parquet with no manifest over it, which no reader opens.
//!
//! **One writer.** Two writers write two different corpora. This is the only
//! function in the workspace that encodes a corpus's Parquet, and the only host
//! that calls it is [`crate::Executor`].

use std::sync::Arc;

use bytes::Bytes;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::error::ArrowError;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::basic::{Compression, Encoding, ZstdLevel};
use datafusion::parquet::errors::ParquetError;
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::parquet::schema::types::ColumnPath;
use fossil_graph_schema::{Cardinality, Failure, NodeType, Problem, Term};
use fossil_storage::{Storage, StorageError};

use datafusion::arrow::array::{Array, StringArray};
use datafusion::error::DataFusionError;
use datafusion::prelude::SessionContext;
use fossil_graph_schema::Primitive;
use fossil_profile::{ColumnKind, TableColumn};

use crate::corpus::generated::{
    ColumnRole, EDGE_COLUMNS, ENDPOINT_DST, ENDPOINT_SRC, PAYLOAD_ADDRESS, PAYLOAD_COLUMNS,
    PAYLOAD_IDENTITY, PROPERTY_COLUMNS, PROPERTY_LANGUAGE, WriterColumn,
};
use crate::corpus::manifest::{
    ColumnAt, EdgeTable as EdgeEntry, Endpoint, Format, Inclusion, MANIFEST_FILE, Manifest,
    Property, PropertyTable as PropertyEntry, ROW_GROUP_ROWS, RR_IRI, RR_LITERAL, Stats, TopValue,
    VertexTable as VertexEntry, data_type_name, edge_path, edge_table_name, property_path,
    property_table_name, vertex_path,
};
use crate::facts::{self, Written as Counted};
use crate::report::EdgeDrops;
use crate::{EdgeTable, Graph};

/// What [`write`](fn@write) answers: the manifest it wrote, and what the edge join
/// discarded — the one fact the corpus cannot hold about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// `fossil.json`, as written.
    pub manifest: Manifest,
    /// One entry per edge table, in manifest order.
    pub dropped: Vec<EdgeDrops>,
}

/// Failure modes of [`write`](fn@write).
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// An Arrow kernel refused a batch — reachable only through a column whose
    /// type is not what the executor produced.
    #[error("arrow: {0}")]
    Arrow(#[from] ArrowError),
    /// Encoding a table's Parquet.
    #[error("parquet: {0}")]
    Parquet(#[from] ParquetError),
    /// Encoding `fossil.json`.
    #[error("fossil.json: {0}")]
    Json(#[from] serde_json::Error),
    /// A relation naming a vertex type the graph did not materialise. The
    /// schema the executor builds names only types it has, so this is fossil's
    /// fault, and it has no `write/` code of its own: no program reaches it.
    #[error("relation `{relation}` references vertex type `{vertex_type}`, which has no table")]
    UnknownType {
        relation: String,
        vertex_type: String,
    },
    /// Two tables under one name — `Person_knows_Person` is both a relation
    /// and `Person`'s multi-valued `knows_Person`. A reader attaches a view per
    /// table name, so the corpus would hold one of them and lose the other.
    #[error("the corpus would declare `{table}` twice")]
    DuplicateTable { table: String },
    /// A row of a language-tagged property whose tag, read from the data, is
    /// not well-formed BCP 47.
    #[error("`{property}` has a value tagged `{tag}`, which is not BCP 47")]
    InvalidLanguageTag { property: String, tag: String },
    /// A row's tag outside the languages the property's shape admits.
    #[error("`{property}` has a value tagged `{tag}`, outside {languages:?}")]
    LanguageNotAllowed {
        property: String,
        tag: String,
        languages: Vec<String>,
    },
    /// Counting what was written: the engine's refusal, `run/over-budget`
    /// among them, which the executor reads off its pool.
    #[error("counting the corpus: {0}")]
    Engine(#[from] DataFusionError),
    /// Putting a file into the store; the store's refusal, kept whole.
    #[error("write `{path}`: {source}")]
    Store {
        path: String,
        #[source]
        source: StorageError,
    },
}

/// The code each way a write fails is. The store's own refusal has one; the rest
/// are fossil's fault, `internal/bug`, with the error as cause. Too many
/// vertices is refused before the writer, where the executor numbers them
/// (`run/too-large`).
impl From<WriteError> for Failure {
    fn from(e: WriteError) -> Self {
        let bug = |what: &str| {
            Self::new(Problem::Bug {
                what: what.to_string(),
            })
        };
        match e {
            WriteError::Store { path, source } => {
                Self::new(Problem::WriteFailed { path }).caused_by(Self::from(source))
            }
            WriteError::DuplicateTable { table } => Self::new(Problem::DuplicateTable { table }),
            WriteError::InvalidLanguageTag { property, tag } => {
                Self::new(Problem::WriteInvalidLanguageTag { property, tag })
            }
            WriteError::LanguageNotAllowed {
                property,
                tag,
                languages,
            } => Self::new(Problem::WriteLanguageNotAllowed {
                property,
                tag,
                languages,
            }),
            WriteError::Engine(e) => Self::new(Problem::EngineFailed {}).caused_by(e),
            e @ WriteError::UnknownType { .. } => {
                bug("a relation names a vertex type with no table").caused_by(e)
            }
            WriteError::Arrow(e) => bug("an Arrow kernel refused a batch").caused_by(e),
            WriteError::Parquet(e) => bug("a table's Parquet did not encode").caused_by(e),
            WriteError::Json(e) => bug("`fossil.json` did not encode").caused_by(e),
        }
    }
}

/// Write `graph` under `dest`, a prefix ending in `/` that `storage` covers,
/// counting what it wrote on `ctx`, the run's session, under its pool.
///
/// Vertex types, relations and multi-valued properties are written in the
/// order the compiled schema lists them, which is the order `fossil.json` lists
/// them in. The rows are the executor's, as they are: a vertex type sorted by
/// `dense_id`, a relation by `(src, dst)`, a property by `(src, value)` — and
/// `lang` after them for a language-tagged one.
///
/// **The writer counts what it wrote** after the last table is final and
/// before `fossil.json` ([`crate::facts`]): the counts are part of the commit,
/// and a corpus never exists without them (`/docs/design/facts`).
///
/// # Errors
///
/// [`WriteError`] on the first failure. Nothing is written after it, and
/// `fossil.json` never is; two tables under one name are refused before
/// anything is, and a tag a language-tagged value carries is held to BCP 47 and
/// to the shape's languages before its table is.
#[tracing::instrument(skip_all, fields(dest = %dest))]
pub async fn write(
    graph: &Graph,
    ctx: &SessionContext,
    storage: &Storage,
    dest: &str,
) -> Result<Written, WriteError> {
    let empty: Vec<RecordBatch> = Vec::new();
    let multi = |node: &'_ NodeType| {
        node.properties
            .iter()
            .filter(|p| p.cardinality == Cardinality::Multi)
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
    };

    let mut names = std::collections::HashSet::new();
    let tables = graph.schema.nodes.iter().map(|n| n.label.clone());
    let tables = tables.chain(
        graph
            .schema
            .edges
            .iter()
            .map(|e| edge_table_name(&e.source, &e.label, &e.destination)),
    );
    let tables = tables.chain(graph.schema.nodes.iter().flat_map(|n| {
        multi(n)
            .into_iter()
            .map(|p| property_table_name(&n.label, &p))
    }));
    for table in tables {
        if !names.insert(table.clone()) {
            return Err(WriteError::DuplicateTable { table });
        }
    }

    // What is counted once every table is written, in manifest order: the
    // vertex tables, the property tables, the edge tables.
    let mut counted_vertices = Vec::new();
    let mut counted_properties = Vec::new();
    let mut vertex_tables = Vec::with_capacity(graph.schema.nodes.len());
    let mut property_tables = Vec::new();
    for node in &graph.schema.nodes {
        let vertex = graph.vertices.iter().find(|v| v.label == node.label);
        let batches = vertex.map_or(empty.as_slice(), |v| v.batches.as_slice());
        let path = vertex_path(&node.label);
        let (bytes, properties, laid) = vertex_parquet(node, batches)?;
        put(storage, dest, &path, bytes).await?;
        counted_vertices.push(counted(&node.label, laid, &properties, node));
        vertex_tables.push(VertexEntry {
            name: node.label.clone(),
            iri: node.iri.clone().filter(|i| !i.is_empty()),
            annotations: node.annotations.clone(),
            path,
            key: PAYLOAD_ADDRESS.to_string(),
            identity: PAYLOAD_IDENTITY.to_string(),
            record_count: rows(batches),
            candidate_keys: Vec::new(),
            derived_from: vertex.map(|v| v.derived_from.clone()).unwrap_or_default(),
            properties,
        });

        for property in multi(node) {
            let values = vertex.and_then(|v| v.properties.iter().find(|p| p.name == property));
            let batches = values.map_or(empty.as_slice(), |p| p.batches.as_slice());
            let name = property_table_name(&node.label, &property);
            let path = property_path(&name);
            hold_tags(node, &property, batches)?;
            let (bytes, properties, laid) = property_parquet(node, &property, batches)?;
            put(storage, dest, &path, bytes).await?;
            counted_properties.push(counted(&name, laid, &properties, node));
            property_tables.push(PropertyEntry {
                name,
                path,
                source: Endpoint {
                    key: ENDPOINT_SRC.to_string(),
                    references: node.label.clone(),
                },
                record_count: rows(batches),
                candidate_keys: Vec::new(),
                derived_from: values.map(|p| p.derived_from.clone()).unwrap_or_default(),
                properties,
            });
        }
    }

    let mut counted_edges = Vec::new();
    let mut edge_tables = Vec::with_capacity(graph.schema.edges.len());
    let mut dropped = Vec::with_capacity(graph.schema.edges.len());
    for edge in &graph.schema.edges {
        let name = edge_table_name(&edge.source, &edge.label, &edge.destination);
        for end in [&edge.source, &edge.destination] {
            if !graph.schema.nodes.iter().any(|n| &n.label == end) {
                return Err(WriteError::UnknownType {
                    relation: name,
                    vertex_type: end.clone(),
                });
            }
        }
        let table: Option<&EdgeTable> = graph.edges.iter().find(|e| {
            (&e.src_type, &e.label, &e.dst_type) == (&edge.source, &edge.label, &edge.destination)
        });
        let batches = table.map_or(empty.as_slice(), |t| t.batches.as_slice());
        let path = edge_path(&name);
        let (bytes, laid) = edge_parquet(batches)?;
        put(storage, dest, &path, bytes).await?;
        let properties: Vec<Property> = EDGE_COLUMNS.iter().map(fixed).collect();
        counted_edges.push(Counted {
            name: name.clone(),
            schema: laid.0,
            batches: laid.1,
            columns: properties.iter().map(writer_column).collect(),
        });
        edge_tables.push(EdgeEntry {
            name: name.clone(),
            label: edge.label.clone(),
            iri: edge.iri.clone().filter(|i| !i.is_empty()),
            annotations: edge.annotations.clone(),
            path,
            source: Endpoint {
                key: ENDPOINT_SRC.to_string(),
                references: edge.source.clone(),
            },
            destination: Endpoint {
                key: ENDPOINT_DST.to_string(),
                references: edge.destination.clone(),
            },
            record_count: rows(batches),
            candidate_keys: Vec::new(),
            derived_from: table.map(|t| t.derived_from.clone()).unwrap_or_default(),
            properties,
        });
        dropped.push(EdgeDrops {
            table: name,
            dropped: table.map_or(0, |t| t.dropped),
        });
    }

    let counted: Vec<Counted> = counted_vertices
        .into_iter()
        .chain(counted_properties)
        .chain(counted_edges)
        .collect();
    let facts = facts::count_written(ctx, &counted).await?;
    let tables = vertex_tables
        .iter_mut()
        .map(|t| (&mut t.candidate_keys, &mut t.properties))
        .chain(
            property_tables
                .iter_mut()
                .map(|t| (&mut t.candidate_keys, &mut t.properties)),
        )
        .chain(
            edge_tables
                .iter_mut()
                .map(|t| (&mut t.candidate_keys, &mut t.properties)),
        );
    for ((keys, properties), (candidate_keys, stats)) in tables.zip(facts.tables) {
        *keys = candidate_keys;
        for (property, s) in properties.iter_mut().zip(stats) {
            property.stats = Some(Stats {
                null_count: s.null_count,
                distinct_count: s.distinct_count,
                min_value: s.min_value,
                max_value: s.max_value,
                top_values: s
                    .top_values
                    .into_iter()
                    .map(|t| TopValue {
                        value: t.value,
                        count: t.count,
                    })
                    .collect(),
            });
        }
    }

    let manifest = Manifest {
        format: Format,
        vertex_tables,
        edge_tables,
        property_tables,
        inclusions: facts
            .inclusions
            .into_iter()
            .map(|(table, column, to_table, to_column)| Inclusion {
                from: ColumnAt { table, column },
                to: ColumnAt {
                    table: to_table,
                    column: to_column,
                },
            })
            .collect(),
    };
    put(
        storage,
        dest,
        MANIFEST_FILE,
        Bytes::from(manifest.to_json()?),
    )
    .await?;
    Ok(Written { manifest, dropped })
}

/// A table as it is counted: its laid-out rows, and what each column may be
/// in an inclusion — read off the manifest entry, so the counts and the
/// document cannot describe two different tables.
fn counted(
    name: &str,
    (schema, batches): (SchemaRef, Vec<RecordBatch>),
    properties: &[Property],
    node: &NodeType,
) -> Counted {
    Counted {
        name: name.to_string(),
        schema,
        batches,
        columns: properties
            .iter()
            .map(|p| {
                if p.role.is_some() {
                    return writer_column(p);
                }
                let primitive = node.properties.iter().find(|d| d.name == p.name).map_or(
                    Primitive::String,
                    |d| match d.datatype {
                        // The tag is beside it: the column holds the form.
                        Primitive::LangString => Primitive::String,
                        other => other,
                    },
                );
                if p.term_type.as_deref() == Some(RR_IRI) {
                    TableColumn {
                        name: p.name.as_str().into(),
                        primitive: Primitive::AnyUri,
                        kind: ColumnKind::Reference,
                    }
                } else {
                    TableColumn {
                        name: p.name.as_str().into(),
                        primitive,
                        kind: ColumnKind::Value,
                    }
                }
            })
            .collect(),
    }
}

/// A column the writer emits, as the counts see it: an identity is what a
/// reference names, and every other role is declared by the manifest.
fn writer_column(p: &Property) -> TableColumn {
    let (primitive, kind) = match p.role {
        Some(ColumnRole::Identity) => (Primitive::AnyUri, ColumnKind::Identity),
        Some(ColumnRole::Language) => (Primitive::String, ColumnKind::Declared),
        _ => (Primitive::Integer, ColumnKind::Declared),
    };
    TableColumn {
        name: p.name.as_str().into(),
        primitive,
        kind,
    }
}

/// Every tag of a language-tagged property's rows, held to BCP 47 (RFC 5646
/// §2.2.9) and to the languages its shape admits. A tag read from the data is
/// only seen by the run, and a bad one refuses it: RML's fallback would write
/// an untagged string into an `rdf:langString` column (`/docs/design/labels`).
fn hold_tags(node: &NodeType, property: &str, batches: &[RecordBatch]) -> Result<(), WriteError> {
    let languages = node
        .properties
        .iter()
        .find(|p| p.name == property)
        .map(|p| p.languages.clone())
        .unwrap_or_default();
    for batch in batches {
        let Some(tags) = batch.column_by_name(PROPERTY_LANGUAGE) else {
            return Ok(());
        };
        let tags = datafusion::arrow::compute::cast(tags, &DataType::Utf8)?;
        let Some(tags) = tags.as_any().downcast_ref::<StringArray>() else {
            continue;
        };
        for i in 0..tags.len() {
            let tag = if tags.is_null(i) { "" } else { tags.value(i) };
            if !fossil_hir::terms::well_formed_language_tag(tag) {
                return Err(WriteError::InvalidLanguageTag {
                    property: property.to_string(),
                    tag: tag.to_string(),
                });
            }
            if !fossil_graph_schema::language_matches(tag, &languages) {
                return Err(WriteError::LanguageNotAllowed {
                    property: property.to_string(),
                    tag: tag.to_string(),
                    languages,
                });
            }
        }
    }
    Ok(())
}

fn rows(batches: &[RecordBatch]) -> u64 {
    batches.iter().map(|b| b.num_rows() as u64).sum()
}

async fn put(storage: &Storage, dest: &str, path: &str, bytes: Bytes) -> Result<(), WriteError> {
    storage
        .put(&format!("{dest}{path}"), bytes)
        .await
        .map_err(|source| WriteError::Store {
            path: path.to_string(),
            source,
        })
}

/// A column the writer emits, as the manifest lists it: `corpus.bnf`'s name,
/// type and role.
fn fixed(column: &WriterColumn) -> Property {
    Property {
        name: column.name.to_string(),
        data_type: column.data_type.to_string(),
        iri: None,
        term_type: None,
        datatype: None,
        nullable: false,
        role: Some(column.role),
        annotations: Vec::new(),
        stats: None,
    }
}

/// The ZSTD level every page is written at. Compression is Parquet-internal —
/// a reader learns the codec from the footer and `fossil.json` names none — so
/// this is a size/time trade, not a format decision. Level 3 is zstd's own
/// default. Measured 2026-09-30 on `docs/programs/shop` over 60,000 generated
/// people and 180,000 orders, through the wasm executor: 13.47 MB uncompressed,
/// 2.31 MB here, and no measurable change in the run's time. `DuckDB`-WASM reads
/// it: `packages/corpus/integration/round-trip.test.ts` opens what this writes.
const ZSTD_LEVEL: i32 = 3;

/// The writer's properties: [`ROW_GROUP_ROWS`] rows per row group, every page
/// ZSTD at [`ZSTD_LEVEL`], and the sorted key columns delta-encoded — a run of
/// ascending `u32`s is what `DELTA_BINARY_PACKED` is for, and dictionary
/// encoding a column of unique values only falls back to plain after trying.
fn properties(delta: &[&str]) -> WriterProperties {
    let mut builder = WriterProperties::builder()
        .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
        .set_max_row_group_bytes(None)
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(ZSTD_LEVEL).expect("a level zstd accepts"),
        ));
    for column in delta {
        let path = ColumnPath::from(*column);
        builder = builder
            .set_column_dictionary_enabled(path.clone(), false)
            .set_column_encoding(path, Encoding::DELTA_BINARY_PACKED);
    }
    builder.build()
}

/// A table's rows laid out as it is written: `schema` over each batch's
/// columns, which only renames and sets nullability.
type Laid = (SchemaRef, Vec<RecordBatch>);

/// Encode `batches` as one Parquet of `schema` under [`properties`], a row
/// group per [`ROW_GROUP_ROWS`], and answer the rows as laid out — what the
/// writer counts once every table is written.
fn encode(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    delta: &[&str],
) -> Result<(Bytes, Laid), WriteError> {
    let laid: Vec<RecordBatch> = batches
        .iter()
        .map(|b| RecordBatch::try_new(Arc::clone(schema), b.columns().to_vec()))
        .collect::<Result<_, _>>()?;
    let mut writer = ArrowWriter::try_new(Vec::new(), Arc::clone(schema), Some(properties(delta)))?;
    for batch in &laid {
        writer.write(batch)?;
    }
    Ok((
        Bytes::from(writer.into_inner()?),
        (Arc::clone(schema), laid),
    ))
}

/// One vertex type's table: the executor's rows as they are — `dense_id`,
/// `subject`, then the program's columns — and the properties that describe
/// them.
fn vertex_parquet(
    node: &NodeType,
    batches: &[RecordBatch],
) -> Result<(Bytes, Vec<Property>, Laid), WriteError> {
    let mut properties: Vec<Property> = PAYLOAD_COLUMNS.iter().map(fixed).collect();

    let Some(first) = batches.first() else {
        // A type with no rows: the writer's columns and the declared ones, and
        // no row group.
        let mut fields: Vec<Field> = PAYLOAD_COLUMNS
            .iter()
            .map(|c| {
                let data_type = if c.role == ColumnRole::Address {
                    DataType::UInt32
                } else {
                    DataType::Utf8
                };
                Field::new(c.name, data_type, false)
            })
            .collect();
        for p in node
            .properties
            .iter()
            .filter(|p| p.cardinality == Cardinality::Single)
        {
            fields.push(Field::new(&p.name, DataType::Utf8, true));
            properties.push(program(node, &p.name, &DataType::Utf8, true));
        }
        let (bytes, laid) = encode(&Arc::new(Schema::new(fields)), &[], &[PAYLOAD_ADDRESS])?;
        return Ok((bytes, properties, laid));
    };

    let input = first.schema();
    let fields: Vec<Field> = input
        .fields()
        .iter()
        .enumerate()
        .map(|(i, field)| {
            if i < PAYLOAD_COLUMNS.len() {
                field.as_ref().clone().with_nullable(false)
            } else {
                properties.push(program(node, field.name(), field.data_type(), true));
                field.as_ref().clone().with_nullable(true)
            }
        })
        .collect();
    let (bytes, laid) = encode(&Arc::new(Schema::new(fields)), batches, &[PAYLOAD_ADDRESS])?;
    Ok((bytes, properties, laid))
}

/// A program's column of `node`, as the manifest lists it. The type is the
/// column's, as written: `properties` describes the file a reader opens, and
/// the checker's belief about a column the executor never introspected is not
/// what the bytes hold. The IRI, the term and the annotations are the shape's,
/// copied as it declared them.
fn program(node: &NodeType, name: &str, arrow: &DataType, nullable: bool) -> Property {
    let declared = node.properties.iter().find(|p| p.name == name);
    let term = declared.and_then(|p| p.term.as_ref());
    Property {
        name: name.to_string(),
        data_type: data_type_name(arrow),
        iri: declared.and_then(|p| p.iri.clone()),
        term_type: term.map(|t| match t {
            Term::Iri => RR_IRI.to_string(),
            Term::Literal(_) => RR_LITERAL.to_string(),
        }),
        datatype: term.and_then(|t| match t {
            Term::Iri => None,
            Term::Literal(datatype) => datatype.clone(),
        }),
        nullable,
        role: None,
        annotations: declared.map(|p| p.annotations.clone()).unwrap_or_default(),
        stats: None,
    }
}

/// One multi-valued property's table: `src`, then the value under the
/// property's name, a row per value and none for an absent one — the
/// executor's rows, sorted by both — and, for a language-tagged value, its
/// tag under `lang` (`corpus.bnf`).
fn property_parquet(
    node: &NodeType,
    property: &str,
    batches: &[RecordBatch],
) -> Result<(Bytes, Vec<Property>, Laid), WriteError> {
    let endpoints: Vec<&WriterColumn> = PROPERTY_COLUMNS
        .iter()
        .filter(|c| c.role == ColumnRole::Endpoint)
        .collect();
    let tagged = node
        .properties
        .iter()
        .any(|p| p.name == property && p.datatype == fossil_graph_schema::Primitive::LangString);
    let value = batches.first().map_or(DataType::Utf8, |b| {
        b.schema().field(endpoints.len()).data_type().clone()
    });
    let trailing: Vec<&WriterColumn> = PROPERTY_COLUMNS
        .iter()
        .filter(|c| tagged && c.role == ColumnRole::Language)
        .collect();
    let fields: Vec<Field> = endpoints
        .iter()
        .map(|c| Field::new(c.name, DataType::UInt32, false))
        .chain(std::iter::once(Field::new(property, value.clone(), false)))
        .chain(
            trailing
                .iter()
                .map(|c| Field::new(c.name, DataType::Utf8, false)),
        )
        .collect();
    let (bytes, laid) = encode(&Arc::new(Schema::new(fields)), batches, &[ENDPOINT_SRC])?;
    let properties = endpoints
        .iter()
        .map(|c| fixed(c))
        .chain(std::iter::once(program(node, property, &value, false)))
        .chain(trailing.iter().map(|c| fixed(c)))
        .collect();
    Ok((bytes, properties, laid))
}

/// One relation's table: `src`, `dst`, sorted by both — the executor's
/// `src_dense`/`dst_dense`, which are global `dense_id`s, renamed.
fn edge_parquet(batches: &[RecordBatch]) -> Result<(Bytes, Laid), WriteError> {
    let schema: SchemaRef = Arc::new(Schema::new(
        EDGE_COLUMNS
            .iter()
            .map(|c| Field::new(c.name, DataType::UInt32, false))
            .collect::<Vec<_>>(),
    ));
    encode(&schema, batches, &[ENDPOINT_SRC])
}
