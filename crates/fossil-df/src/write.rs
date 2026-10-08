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
//! **One writer.** There were three — a native host that wrote a directory, the
//! browser's in-memory map, and a tile container under both — and they wrote
//! different corpora. This is the only function in the workspace that encodes
//! a corpus's Parquet, and the only host that calls it is [`crate::Executor`].

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
use fossil_sinks::generated::{
    ColumnRole, EDGE_COLUMNS, ENDPOINT_DST, ENDPOINT_SRC, PAYLOAD_ADDRESS, PAYLOAD_COLUMNS,
    PAYLOAD_IDENTITY, PROPERTY_COLUMNS, WriterColumn,
};
use fossil_sinks::manifest::{
    EdgeTable as EdgeEntry, Endpoint, Format, MANIFEST_FILE, Manifest, Property,
    PropertyTable as PropertyEntry, ROW_GROUP_ROWS, RR_IRI, RR_LITERAL, VertexTable as VertexEntry,
    data_type_name, edge_path, edge_table_name, property_path, property_table_name, vertex_path,
};
use fossil_storage::{Storage, StorageError};

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
    /// fault — `write/unknown-type`, retired after `v0.3.0-alpha.18`, had no
    /// program that reached it.
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
            e @ WriteError::UnknownType { .. } => {
                bug("a relation names a vertex type with no table").caused_by(e)
            }
            WriteError::Arrow(e) => bug("an Arrow kernel refused a batch").caused_by(e),
            WriteError::Parquet(e) => bug("a table's Parquet did not encode").caused_by(e),
            WriteError::Json(e) => bug("`fossil.json` did not encode").caused_by(e),
        }
    }
}

/// Write `graph` under `dest`, a prefix ending in `/` that `storage` covers.
///
/// Vertex types, relations and multi-valued properties are written in the
/// order the compiled schema lists them, which is the order `fossil.json` lists
/// them in. The rows are the executor's, as they are: a vertex type sorted by
/// `dense_id`, a relation by `(src, dst)`, a property by `(src, value)`.
///
/// # Errors
///
/// [`WriteError`] on the first failure. Nothing is written after it, and
/// `fossil.json` never is; two tables under one name are refused before
/// anything is.
#[tracing::instrument(skip_all, fields(dest = %dest))]
pub async fn write(graph: &Graph, storage: &Storage, dest: &str) -> Result<Written, WriteError> {
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

    let mut vertex_tables = Vec::with_capacity(graph.schema.nodes.len());
    let mut property_tables = Vec::new();
    for node in &graph.schema.nodes {
        let vertex = graph.vertices.iter().find(|v| v.label == node.label);
        let batches = vertex.map_or(empty.as_slice(), |v| v.batches.as_slice());
        let path = vertex_path(&node.label);
        let (bytes, properties) = vertex_parquet(node, batches)?;
        put(storage, dest, &path, bytes).await?;
        vertex_tables.push(VertexEntry {
            name: node.label.clone(),
            iri: node.iri.clone().filter(|i| !i.is_empty()),
            path,
            key: PAYLOAD_ADDRESS.to_string(),
            identity: PAYLOAD_IDENTITY.to_string(),
            record_count: rows(batches),
            derived_from: vertex.map(|v| v.derived_from.clone()).unwrap_or_default(),
            properties,
        });

        for property in multi(node) {
            let values = vertex.and_then(|v| v.properties.iter().find(|p| p.name == property));
            let batches = values.map_or(empty.as_slice(), |p| p.batches.as_slice());
            let name = property_table_name(&node.label, &property);
            let path = property_path(&name);
            let (bytes, value) = property_parquet(node, &property, batches)?;
            put(storage, dest, &path, bytes).await?;
            property_tables.push(PropertyEntry {
                name,
                path,
                source: Endpoint {
                    key: ENDPOINT_SRC.to_string(),
                    references: node.label.clone(),
                },
                record_count: rows(batches),
                derived_from: values.map(|p| p.derived_from.clone()).unwrap_or_default(),
                properties: PROPERTY_COLUMNS
                    .iter()
                    .map(fixed)
                    .chain(std::iter::once(value))
                    .collect(),
            });
        }
    }

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
        put(storage, dest, &path, edge_parquet(batches)?).await?;
        edge_tables.push(EdgeEntry {
            name: name.clone(),
            label: edge.label.clone(),
            iri: edge.iri.clone().filter(|i| !i.is_empty()),
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
            derived_from: table.map(|t| t.derived_from.clone()).unwrap_or_default(),
            properties: EDGE_COLUMNS.iter().map(fixed).collect(),
        });
        dropped.push(EdgeDrops {
            table: name,
            dropped: table.map_or(0, |t| t.dropped),
        });
    }

    let manifest = Manifest {
        format: Format,
        vertex_tables,
        edge_tables,
        property_tables,
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
    }
}

/// The ZSTD level every page is written at. Compression is Parquet-internal —
/// a reader learns the codec from the footer and `fossil.json` names none — so
/// this is a size/time trade, not a format decision. Level 3 is zstd's own
/// default. Measured 2026-09-30 on `docs/programs/shop` over 60,000 generated
/// people and 180,000 orders, through the wasm executor: 13.47 MB uncompressed,
/// 2.31 MB here, and no measurable change in the run's time. DuckDB-WASM reads
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

/// Encode `batches` as one Parquet of `schema` under [`properties`], a row
/// group per [`ROW_GROUP_ROWS`]. Each batch is taken column for column: the
/// schema only renames and sets nullability.
fn encode(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    delta: &[&str],
) -> Result<Bytes, WriteError> {
    let mut writer = ArrowWriter::try_new(Vec::new(), Arc::clone(schema), Some(properties(delta)))?;
    for batch in batches {
        writer.write(&RecordBatch::try_new(
            Arc::clone(schema),
            batch.columns().to_vec(),
        )?)?;
    }
    Ok(Bytes::from(writer.into_inner()?))
}

/// One vertex type's table: the executor's rows as they are — `dense_id`,
/// `subject`, then the program's columns — and the properties that describe
/// them.
fn vertex_parquet(
    node: &NodeType,
    batches: &[RecordBatch],
) -> Result<(Bytes, Vec<Property>), WriteError> {
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
        let bytes = encode(&Arc::new(Schema::new(fields)), &[], &[PAYLOAD_ADDRESS])?;
        return Ok((bytes, properties));
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
    let bytes = encode(&Arc::new(Schema::new(fields)), batches, &[PAYLOAD_ADDRESS])?;
    Ok((bytes, properties))
}

/// A program's column of `node`, as the manifest lists it. The type is the
/// column's, as written: `properties` describes the file a reader opens, and
/// the checker's belief about a column the executor never introspected is not
/// what the bytes hold. The IRI and the term are the shape's, copied as it
/// declared them.
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
    }
}

/// One multi-valued property's table: `src`, then the value under the
/// property's name, a row per value and none for an absent one — the
/// executor's rows, sorted by both.
fn property_parquet(
    node: &NodeType,
    property: &str,
    batches: &[RecordBatch],
) -> Result<(Bytes, Property), WriteError> {
    let value = batches.first().map_or(DataType::Utf8, |b| {
        b.schema().field(PROPERTY_COLUMNS.len()).data_type().clone()
    });
    let fields: Vec<Field> = PROPERTY_COLUMNS
        .iter()
        .map(|c| Field::new(c.name, DataType::UInt32, false))
        .chain(std::iter::once(Field::new(property, value.clone(), false)))
        .collect();
    let bytes = encode(&Arc::new(Schema::new(fields)), batches, &[ENDPOINT_SRC])?;
    Ok((bytes, program(node, property, &value, false)))
}

/// One relation's table: `src`, `dst`, sorted by both — the executor's
/// `src_dense`/`dst_dense`, which are global `dense_id`s, renamed.
fn edge_parquet(batches: &[RecordBatch]) -> Result<Bytes, WriteError> {
    let schema: SchemaRef = Arc::new(Schema::new(
        EDGE_COLUMNS
            .iter()
            .map(|c| Field::new(c.name, DataType::UInt32, false))
            .collect::<Vec<_>>(),
    ));
    encode(&schema, batches, &[ENDPOINT_SRC])
}
