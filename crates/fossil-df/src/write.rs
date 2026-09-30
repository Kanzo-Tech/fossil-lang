//! The corpus, written: [`write`](crate::write::write) lays the whole graph out and writes one
//! Parquet per vertex type and per relation, then `fossil.json`.
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
use datafusion::arrow::array::{ArrayRef, Float32Array, RecordBatch, UInt32Array, new_empty_array};
use datafusion::arrow::compute::interleave_record_batch;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::error::ArrowError;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::basic::{Compression, Encoding, ZstdLevel};
use datafusion::parquet::errors::ParquetError;
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::parquet::schema::types::ColumnPath;
use fossil_graph_schema::NodeType;
use fossil_layout::layout::{Layout, LayoutError, Relation, VertexType, layout};
use fossil_mem_probe::Probe;
use fossil_sinks::generated::{EDGE_COLUMNS, ENDPOINT_DST, ENDPOINT_SRC, PAYLOAD_COLUMNS};
use fossil_sinks::manifest::{
    EdgeTable as EdgeEntry, Endpoint, FOSSIL_FORMAT, MANIFEST_FILE, Manifest, Position, Property,
    ROW_GROUP_ROWS, VertexTable as VertexEntry, data_type_name, edge_path, edge_table_name,
    vertex_path,
};
use fossil_storage::Storage;

use crate::Graph;
use crate::report::EdgeDrops;

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
    /// The layout pass refused the graph.
    #[error(transparent)]
    Layout(#[from] LayoutError),
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
    /// A relation naming a vertex type the graph did not materialise.
    #[error("relation `{relation}` references vertex type `{vertex_type}`, which has no table")]
    UnknownType {
        relation: String,
        vertex_type: String,
    },
    /// Putting a file into the store.
    #[error("write `{path}`: {message}")]
    Store { path: String, message: String },
}

/// Lay `graph` out and write it under `dest`, a prefix ending in `/` that
/// `storage` covers.
///
/// Vertex types and relations are written in the order the compiled schema
/// lists them, which is the order `fossil.json` lists them in.
///
/// # Errors
///
/// [`WriteError`] on the first failure. Nothing is written after it, and
/// `fossil.json` never is.
pub async fn write(graph: &Graph, storage: &Storage, dest: &str) -> Result<Written, WriteError> {
    let mut probe = Probe::new("write");
    let nodes = &graph.schema.nodes;
    let empty: Vec<RecordBatch> = Vec::new();
    let batches: Vec<&[RecordBatch]> = nodes
        .iter()
        .map(|node| {
            graph
                .vertices
                .iter()
                .find(|v| v.label == node.label)
                .map_or(empty.as_slice(), |v| v.batches.as_slice())
        })
        .collect();
    let types: Vec<VertexType<'_>> = nodes
        .iter()
        .zip(&batches)
        .map(|(node, batches)| VertexType {
            name: &node.label,
            batches,
        })
        .collect();

    let index_of = |relation: &str, name: &str| {
        nodes
            .iter()
            .position(|n| n.label == name)
            .ok_or_else(|| WriteError::UnknownType {
                relation: relation.to_string(),
                vertex_type: name.to_string(),
            })
    };
    let mut names = Vec::with_capacity(graph.schema.edges.len());
    let mut tables = Vec::with_capacity(graph.schema.edges.len());
    for edge in &graph.schema.edges {
        let name = edge_table_name(&edge.source, &edge.label, &edge.destination);
        let table = graph.edges.iter().find(|e| {
            (&e.src_type, &e.label, &e.dst_type) == (&edge.source, &edge.label, &edge.destination)
        });
        names.push(name);
        tables.push(table);
    }
    let mut relations = Vec::with_capacity(names.len());
    for ((edge, name), table) in graph.schema.edges.iter().zip(&names).zip(&tables) {
        relations.push(Relation {
            name,
            source: index_of(name, &edge.source)?,
            destination: index_of(name, &edge.destination)?,
            batches: table.map_or(empty.as_slice(), |t| t.batches.as_slice()),
        });
    }

    let placed = layout(&types, &relations, None)?;
    probe.mark("layout");

    let mut vertex_tables = Vec::with_capacity(nodes.len());
    for (t, node) in nodes.iter().enumerate() {
        let path = vertex_path(&node.label);
        let (bytes, properties, rows) = vertex_parquet(node, batches[t], &placed, t)?;
        put(storage, dest, &path, bytes).await?;
        vertex_tables.push(VertexEntry {
            name: node.label.clone(),
            iri: node.iri.clone().filter(|i| !i.is_empty()),
            path,
            key: PAYLOAD_COLUMNS[0].name.to_string(),
            identity: PAYLOAD_COLUMNS[1].name.to_string(),
            record_count: rows,
            properties,
            position: Some(Position::Layout {
                x: "x".to_string(),
                y: "y".to_string(),
            }),
        });
    }
    probe.mark("write vertex tables");

    let mut edge_tables = Vec::with_capacity(relations.len());
    let mut dropped = Vec::with_capacity(relations.len());
    for ((edge, relation), table) in graph.schema.edges.iter().zip(&relations).zip(&tables) {
        let (sources, destinations) = placed.edges(relation)?;
        let rows = sources.len() as u64;
        let path = edge_path(relation.name);
        put(storage, dest, &path, edge_parquet(sources, destinations)?).await?;
        edge_tables.push(EdgeEntry {
            name: relation.name.to_string(),
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
            record_count: rows,
            properties: EDGE_COLUMNS
                .iter()
                .map(|c| fixed(c.name, c.data_type))
                .collect(),
        });
        dropped.push(EdgeDrops {
            table: relation.name.to_string(),
            dropped: table.map_or(0, |t| t.dropped),
        });
    }
    probe.mark("write edge tables");

    let manifest = Manifest {
        format: FOSSIL_FORMAT.to_string(),
        vertex_tables,
        edge_tables,
    };
    put(
        storage,
        dest,
        MANIFEST_FILE,
        Bytes::from(manifest.to_json()?),
    )
    .await?;
    probe.finish();
    Ok(Written { manifest, dropped })
}

async fn put(storage: &Storage, dest: &str, path: &str, bytes: Bytes) -> Result<(), WriteError> {
    storage
        .put(&format!("{dest}{path}"), bytes)
        .await
        .map_err(|e| WriteError::Store {
            path: path.to_string(),
            message: e.to_string(),
        })
}

/// A column the writer emits, as the manifest lists it.
fn fixed(name: &str, data_type: &str) -> Property {
    Property {
        name: name.to_string(),
        data_type: data_type.to_string(),
        iri: None,
        nullable: false,
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

/// Encode `batches` as one Parquet under [`properties`], a row group per
/// [`ROW_GROUP_ROWS`].
fn encode(
    schema: SchemaRef,
    batches: impl IntoIterator<Item = Result<RecordBatch, WriteError>>,
    delta: &[&str],
) -> Result<Bytes, WriteError> {
    let mut writer = ArrowWriter::try_new(Vec::new(), schema, Some(properties(delta)))?;
    for batch in batches {
        writer.write(&batch?)?;
    }
    Ok(Bytes::from(writer.into_inner()?))
}

/// One vertex type's table: its rows in global `dense_id` order, the five
/// writer columns first and the program's after, and the properties that
/// describe them.
fn vertex_parquet(
    node: &NodeType,
    batches: &[RecordBatch],
    placed: &Layout,
    t: usize,
) -> Result<(Bytes, Vec<Property>, u64), WriteError> {
    let fixed_names: Vec<&str> = PAYLOAD_COLUMNS.iter().map(|c| c.name).collect();
    let mut properties: Vec<Property> = PAYLOAD_COLUMNS
        .iter()
        .map(|c| fixed(c.name, c.data_type))
        .collect();
    // The type is the column's, as written: `properties` describes the file a
    // reader opens, and the checker's belief about a column the executor never
    // introspected is not what the bytes hold. The IRI is the shape's.
    let program = |name: &str, arrow: &DataType| Property {
        name: name.to_string(),
        data_type: data_type_name(arrow),
        iri: node
            .properties
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.iri.clone()),
        nullable: true,
    };

    let Some(first) = batches.first() else {
        // A type with no rows: the writer's columns and the declared ones, and
        // no row group.
        let mut fields: Vec<Field> = PAYLOAD_COLUMNS
            .iter()
            .map(|c| Field::new(c.name, fixed_type(c.name), false))
            .collect();
        for p in &node.properties {
            fields.push(Field::new(&p.name, DataType::Utf8, true));
            properties.push(program(&p.name, &DataType::Utf8));
        }
        let schema = Arc::new(Schema::new(fields));
        return Ok((
            encode(schema, std::iter::empty(), &["dense_id"])?,
            properties,
            0,
        ));
    };

    let input = first.schema();
    let rest: Vec<usize> = (0..input.fields().len())
        .filter(|&i| !fixed_names.contains(&input.field(i).name().as_str()))
        .collect();
    let subject = input.index_of(PAYLOAD_COLUMNS[1].name)?;
    let mut fields: Vec<Field> = PAYLOAD_COLUMNS
        .iter()
        .map(|c| {
            let data_type = if c.name == PAYLOAD_COLUMNS[1].name {
                input.field(subject).data_type().clone()
            } else {
                fixed_type(c.name)
            };
            Field::new(c.name, data_type, false)
        })
        .collect();
    for &i in &rest {
        let field = input.field(i);
        fields.push(field.as_ref().clone().with_nullable(true));
        properties.push(program(field.name(), field.data_type()));
    }
    let schema: SchemaRef = Arc::new(Schema::new(fields));

    let refs: Vec<&RecordBatch> = batches.iter().collect();
    let starts: Vec<usize> = batches
        .iter()
        .scan(0usize, |acc, b| {
            let start = *acc;
            *acc += b.num_rows();
            Some(start)
        })
        .collect();
    let locate = |row: u32| {
        let row = row as usize;
        let batch = starts.partition_point(|&s| s <= row) - 1;
        (batch, row - starts[batch])
    };
    let order = &placed.order[t];
    let rows = order.len() as u64;
    let groups = order.chunks(ROW_GROUP_ROWS).map(|locals| {
        let picks: Vec<(usize, usize)> = locals.iter().map(|&l| locate(l)).collect();
        let gathered = interleave_record_batch(&refs, &picks)?;
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(UInt32Array::from_iter_values(
                locals.iter().map(|&l| placed.ids[t][l as usize]),
            )),
            Arc::clone(gathered.column(subject)),
            Arc::new(Float32Array::from_iter_values(
                locals.iter().map(|&l| placed.x[t][l as usize]),
            )),
            Arc::new(Float32Array::from_iter_values(
                locals.iter().map(|&l| placed.y[t][l as usize]),
            )),
            Arc::new(UInt32Array::from_iter_values(
                locals.iter().map(|&l| placed.cluster[t][l as usize]),
            )),
        ];
        columns.extend(rest.iter().map(|&i| Arc::clone(gathered.column(i))));
        Ok(RecordBatch::try_new(Arc::clone(&schema), columns)?)
    });
    let bytes = encode(Arc::clone(&schema), groups, &["dense_id"])?;
    Ok((bytes, properties, rows))
}

/// The Arrow type of a writer column other than `subject`.
fn fixed_type(name: &str) -> DataType {
    match name {
        "x" | "y" => DataType::Float32,
        _ => DataType::UInt32,
    }
}

/// One relation's table: `src`, `dst`, sorted by both.
fn edge_parquet(src: Vec<u32>, dst: Vec<u32>) -> Result<Bytes, WriteError> {
    let schema: SchemaRef = Arc::new(Schema::new(
        EDGE_COLUMNS
            .iter()
            .map(|c| Field::new(c.name, DataType::UInt32, false))
            .collect::<Vec<_>>(),
    ));
    let rows = src.len();
    let (src, dst): (ArrayRef, ArrayRef) = if rows == 0 {
        (
            new_empty_array(&DataType::UInt32),
            new_empty_array(&DataType::UInt32),
        )
    } else {
        (
            Arc::new(UInt32Array::from(src)),
            Arc::new(UInt32Array::from(dst)),
        )
    };
    let whole = RecordBatch::try_new(Arc::clone(&schema), vec![src, dst])?;
    let groups = (0..rows)
        .step_by(ROW_GROUP_ROWS)
        .map(|lo| Ok(whole.slice(lo, ROW_GROUP_ROWS.min(rows - lo))));
    encode(schema, groups, &[ENDPOINT_SRC])
}
