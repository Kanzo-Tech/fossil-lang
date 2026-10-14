//! The graph, written: [`write`](crate::write::write) writes an Apache `GraphAr`
//! graph — a vertex type's chunk, a relation's adjacency list and offsets, and
//! the information files — through `graphar-parquet`.
//!
//! **The graph carries the graph, not a picture.** Where a vertex is drawn is
//! the view's choice — two columns it binds, or its own simulation — so the
//! writer places nothing and computes no community. `/docs/design/position`
//! has the argument and `/docs/design/discarded` the nine placements before it.
//!
//! **The graph document goes last, and that is the commit.** Every table is in
//! the store before the document that names it, so a reader that finds
//! `fossil.graph.yml` finds everything it lists; a write that fails half way
//! leaves chunks with no document over them, which no reader opens. `GraphAr`
//! defines no commit: the order is `/docs/design/graphar`'s.
//!
//! **One writer.** Two writers write two different graphs. This is the only
//! function in the workspace that encodes a graph's Parquet, and the only host
//! that calls it is [`crate::Executor`].

use datafusion::arrow::error::ArrowError;
use datafusion::error::DataFusionError;
use datafusion::parquet::basic::{Compression, Encoding, ZstdLevel};
use datafusion::parquet::file::metadata::KeyValue;
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::parquet::schema::types::ColumnPath;
use datafusion::prelude::SessionContext;
use fossil_graph_schema::{Failure, Problem};
use fossil_storage::{Storage, StorageError};
use graphar_info::{AdjListType, GraphInfo};
use graphar_parquet::{Options, SRC_INDEX_COL, VERTEX_INDEX_COL};

use crate::Graph;
use crate::facts::{self, FOOTER_KEY};
use crate::layout::{self, user_types};
use crate::report::EdgeDrops;

/// What [`write`](fn@write) answers: the graph it wrote, and what the edge join
/// discarded — the one fact the graph cannot hold about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// `fossil.graph.yml` and the information files it names, as written.
    pub info: GraphInfo,
    /// One entry per edge type, in the order `info` lists them.
    pub dropped: Vec<EdgeDrops>,
}

/// Failure modes of [`write`](fn@write).
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// An Arrow kernel refused a batch — reachable only through a column whose
    /// type is not what the executor produced.
    #[error("arrow: {0}")]
    Arrow(#[from] ArrowError),
    /// The graph's information files, or its chunks, would not encode. `at` is
    /// the path under the graph's root that was being written.
    #[error("write `{at}`: {source}")]
    Graph {
        at: String,
        #[source]
        source: graphar_parquet::Error,
    },
    /// The graph does not describe itself validly — `graphar-info`'s rules.
    #[error("graph information: {0}")]
    Info(#[from] graphar_info::Error),
    /// A table not in the order the executor promises: fossil's fault, with no
    /// `write/` code of its own.
    #[error("{0}")]
    Internal(String),
    /// A relation naming a vertex type the graph did not materialise. The
    /// schema the executor builds names only types it has, so this is fossil's
    /// fault, and it has no `write/` code of its own: no program reaches it.
    #[error("relation `{relation}` references vertex type `{vertex_type}`, which has no table")]
    UnknownType {
        relation: String,
        vertex_type: String,
    },
    /// A value that must be unique and is written twice (A1, A4, A5).
    #[error("`{at}` holds `{value}` twice")]
    DuplicateValue { at: String, value: String },
    /// A vertex whose `subject` is NULL or not an absolute IRI (A1).
    #[error("a `{vertex_type}` vertex has the subject {subject}, which is not an absolute IRI")]
    InvalidSubject {
        vertex_type: String,
        subject: String,
    },
    /// A value `GraphAr`'s types cannot hold.
    #[error("`{property}` holds `{value}`, of type {datatype}, which GraphAr cannot hold")]
    Unrepresentable {
        property: String,
        datatype: String,
        value: String,
    },
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
    #[error("counting the graph: {0}")]
    Engine(#[from] DataFusionError),
    /// Resolving where the graph goes; the store's refusal, kept whole.
    #[error("write `{path}`: {source}")]
    Store {
        path: String,
        #[source]
        source: StorageError,
    },
}

/// The code each way a write fails is. The store's own refusal has one; the data
/// that cannot be a graph has one each; the rest are fossil's fault,
/// `internal/bug`, with the error as cause. Too many vertices is refused before
/// the writer, where the executor numbers them (`run/too-large`).
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
            WriteError::Graph {
                at,
                source: graphar_parquet::Error::Store(store),
            } => Self::new(Problem::WriteFailed { path: at.clone() })
                .caused_by(Self::new(Problem::Unreachable { location: at }).caused_by(store)),
            WriteError::DuplicateValue { at, value } => {
                Self::new(Problem::WriteDuplicateValue { at, value })
            }
            WriteError::InvalidSubject {
                vertex_type,
                subject,
            } => Self::new(Problem::WriteInvalidSubject {
                vertex_type,
                subject,
            }),
            WriteError::Unrepresentable {
                property,
                datatype,
                value,
            } => Self::new(Problem::WriteUnrepresentable {
                property,
                datatype,
                value,
            }),
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
            e @ WriteError::Internal(_) => bug("a table is not in the order promised").caused_by(e),
            WriteError::Arrow(e) => bug("an Arrow kernel refused a batch").caused_by(e),
            WriteError::Info(e) => bug("the graph's information files are not valid").caused_by(e),
            WriteError::Graph { source, .. } => bug("a chunk did not encode").caused_by(source),
        }
    }
}

/// Write `graph` under `dest`, a prefix ending in `/` that `storage` covers,
/// counting what it writes on `ctx`, the run's session, under its pool.
///
/// Vertex types and relations are written in the order the compiled schema
/// lists them, which is the order `fossil.graph.yml` lists them in. A vertex
/// type's rows are the executor's: sorted by `subject`, numbered from 0 within
/// the type. A relation's are sorted by `(src, dst)`, as indices within the end
/// types.
///
/// **The writer counts what it writes** before each vertex chunk
/// ([`crate::facts`]): the counts are in the chunk's own footer, and a graph
/// never exists without them (`/docs/design/facts`).
///
/// # Errors
///
/// [`WriteError`] on the first failure. Nothing is written after it, and
/// `fossil.graph.yml` never is; the data that cannot be a graph — a subject
/// that is not an IRI, a value `GraphAr` cannot hold, a set with an element
/// twice — is refused before any chunk is.
#[tracing::instrument(skip_all, fields(dest = %dest))]
pub async fn write(
    graph: &Graph,
    ctx: &SessionContext,
    storage: &Storage,
    dest: &str,
) -> Result<Written, WriteError> {
    let laid = layout::layout(graph)?;
    laid.info.validate()?;
    let (store, root) = storage.at(dest).map_err(|source| WriteError::Store {
        path: dest.to_string(),
        source,
    })?;
    let at = |prefix: &str| {
        let prefix = prefix.to_string();
        move |source| WriteError::Graph { at: prefix, source }
    };

    for vertex in &laid.vertices {
        let stats = facts::footer(ctx, vertex).await?;
        let options = Options {
            props: properties(&[VERTEX_INDEX_COL], Some(stats)),
            user_types: user_types(),
        };
        graphar_parquet::write_vertices(
            &*store,
            &root,
            &vertex.info,
            std::slice::from_ref(&vertex.batch),
            &options,
        )
        .await
        .map_err(at(&vertex.info.prefix))?;
    }

    // The sorted key and the offsets are runs of ascending integers, which is
    // what `DELTA_BINARY_PACKED` is for. The destination is not sorted.
    let options = Options {
        props: properties(&[SRC_INDEX_COL, graphar_parquet::OFFSET_COL], None),
        user_types: user_types(),
    };
    let mut dropped = Vec::with_capacity(laid.edges.len());
    for edge in &laid.edges {
        graphar_parquet::write_edges(
            &*store,
            &root,
            &edge.info,
            AdjListType::OrderedBySource,
            edge.sources,
            std::slice::from_ref(&edge.batch),
            &options,
        )
        .await
        .map_err(at(&edge.info.prefix))?;
        dropped.push(EdgeDrops {
            table: edge.info.triple(),
            dropped: edge.dropped,
        });
    }

    // F1b: `mapping.ttl`, the RML-Core document that says what the tables mean as
    // RDF, is written HERE — after every table it describes and before the graph
    // document, whose presence is the reader's sign that the rest is there.
    // `describe` builds it from `laid.info` and the schema with rudof's `rml`.

    graphar_parquet::write_info(&*store, &root, &laid.info)
        .await
        .map_err(at(&laid.info.file_name()))?;
    Ok(Written {
        info: laid.info,
        dropped,
    })
}

/// Rows per Parquet row group, in every chunk of a graph.
///
/// `DuckDB`'s own default, set explicitly because parquet-rs defaults to
/// 1,048,576 — which at a million vertices is one row group and no pruning by
/// the statistics an index range reads. A reader prunes on the footer's
/// statistics; nothing reads this number back.
const ROW_GROUP_ROWS: usize = 122_880;

/// The ZSTD level every page is written at. Compression is Parquet-internal —
/// a reader learns the codec from the footer — so this is a size/time trade, not
/// a format decision. Level 3 is zstd's own default. Measured 2026-09-30 on
/// `docs/programs/shop` over 60,000 generated people and 180,000 orders, through
/// the wasm executor: 13.47 MB uncompressed, 2.31 MB here, and no measurable
/// change in the run's time.
const ZSTD_LEVEL: i32 = 3;

/// The writer's properties: [`ROW_GROUP_ROWS`] rows per row group, every page ZSTD
/// at [`ZSTD_LEVEL`], the sorted columns `delta` delta-encoded — dictionary
/// encoding a column of unique values only falls back to plain after trying —
/// and, for a vertex chunk, its facts under [`FOOTER_KEY`] in the file's
/// `key_value_metadata`. A column a file does not have is not set.
fn properties(delta: &[&str], stats: Option<String>) -> WriterProperties {
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
    if let Some(stats) = stats {
        builder = builder
            .set_key_value_metadata(Some(vec![KeyValue::new(FOOTER_KEY.to_string(), stats)]));
    }
    builder.build()
}
