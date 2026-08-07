//! The GraphAr output as in-memory bytes — the single Arrow→Parquet encoder
//! shared by the native fs sink ([`crate::sink`]) and the browser executor
//! (`fossil-df-wasm`).
//!
//! Universal-substrate decision: ONE parquet encoder in Rust (parquet-rs,
//! native + wasm), no parquet-wasm JS. [`GraphArData::to_files`] turns the
//! materialised graph into the W0b GraphAr tree as `(rel_path, bytes)` pairs —
//! one Parquet per vertex type, the CSR/CSC Parquet pair per edge, and the three
//! manifest YAMLs. The native sink writes each pair to disk; the wasm host hands
//! them to JS for signed `PUT`. Pure in-memory (`Vec<u8>` writer), so the encoder
//! itself never touches the filesystem and compiles to `wasm32`.

use datafusion::arrow::record_batch::RecordBatch;
use fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::GraphArData;

/// One output file of the GraphAr dataset: its dataset-relative path
/// (`vertex/<Type>.parquet`, `edge/<dir>/by_source.parquet`, `graph.graph.yml`,
/// …) and its encoded bytes. The path keys both the on-disk layout (native sink)
/// and the signed-`PUT` object key (browser host).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphArFile {
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Encoding the GraphAr dataset to bytes failed — Parquet encode or manifest
/// YAML serialization. Filesystem errors live in [`crate::sink::SinkError`]
/// (native-only); this stays wasm-clean.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("parquet encode: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("manifest yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

impl GraphArData {
    /// The whole GraphAr dataset as in-memory `(rel_path, bytes)` files: every
    /// vertex Parquet, every edge CSR/CSC Parquet pair, and the three manifest
    /// YAMLs ([`Self::manifests`]). A zero-row vertex/edge encodes nothing (no
    /// schema to declare) and is skipped, exactly as the fs sink does.
    ///
    /// This is the single encoder both hosts share: the native [`crate::sink`]
    /// writes each file to a directory, the wasm host returns them to JS.
    ///
    /// # Errors
    /// Parquet encode or manifest YAML serialization failures.
    pub fn to_files(&self) -> Result<Vec<GraphArFile>, EncodeError> {
        let mut out = Vec::new();
        self.try_for_each_file::<EncodeError>(|file| {
            out.push(file);
            Ok(())
        })?;
        Ok(out)
    }

    /// The same dataset, one file at a time: each is encoded, handed to `emit`,
    /// and dropped before the next one is built.
    ///
    /// This is the encoder — [`Self::to_files`] is this with a `Vec` on the end.
    /// Collecting first keeps every Parquet buffer resident at once, which the
    /// native sink never needed: it writes each file and forgets it. The browser
    /// host does, because it hands JS a list.
    ///
    /// **This is a shape, not a measured win.** At ten million vertices the whole
    /// output is 713 MB across six files, and swapping the list for this changed
    /// peak RSS by 0.02 GB — inside the run-to-run noise. The +0.58 GiB the probe
    /// bills to the encode phase is the encoder's own working memory, which this
    /// does not touch. It is here because holding N files to write them one at a
    /// time is indefensible per file, not because it paid at this size.
    ///
    /// # Errors
    /// Whatever `emit` returns, or an encode failure converted through `E`.
    pub fn try_for_each_file<E: From<EncodeError>>(
        &self,
        mut emit: impl FnMut(GraphArFile) -> Result<(), E>,
    ) -> Result<(), E> {
        for v in &self.vertices {
            if let Some(bytes) = batches_to_parquet(&v.batches).map_err(EncodeError::from)? {
                emit(GraphArFile {
                    rel_path: format!("vertex/{}.parquet", v.label),
                    bytes,
                })?;
            }
        }
        for e in &self.edges {
            let dir = format!("{}_{}_{}", e.src_type, e.label, e.dst_type);
            for (orient, batches) in [("by_source", &e.by_source), ("by_target", &e.by_target)] {
                if let Some(bytes) = batches_to_parquet(batches).map_err(EncodeError::from)? {
                    emit(GraphArFile {
                        rel_path: format!("edge/{dir}/{orient}.parquet"),
                        bytes,
                    })?;
                }
            }
        }
        for manifest in self.manifests().map_err(EncodeError::from)? {
            emit(GraphArFile {
                rel_path: manifest.rel_path,
                bytes: manifest.yaml.into_bytes(),
            })?;
        }
        Ok(())
    }
}

/// Encode `batches` to a Parquet byte buffer (uncompressed — `parquet`'s default
/// props; the discovery reader decodes any valid Parquet). Returns `None` for an
/// empty batch set (a 0-row type has no schema to declare).
///
/// **One row group per tile.** The only property set is the row-group row count,
/// and it is [`DEFAULT_CHUNK_SIZE`] — the same 4,096 that addresses a tile
/// (ADR-0042 §3.1). `parquet`'s default is 1,048,576 rows, which puts a whole
/// five-million-row type in five row groups and leaves the footer with five
/// `x`/`y` boxes to prune with. With this set the footer carries one box per
/// tile, which is the index the addressed reader wants and is the whole of
/// ADR-0056 §2.
///
/// Measured at five million (`examples/tile_layout.rs`): 1,221 row groups, a
/// 496 kB footer, 5.6 range requests and 1.38 MB per window against 22.3
/// requests and 1.40 MB for the same tiles as 1,221 separate files. Setting the
/// row-group size does not change the bytes stored — 75.81 MB against
/// 76.46 MB, and the difference is 1,220 footers that stop existing.
///
/// # Errors
/// Parquet encode failures.
pub fn batches_to_parquet(
    batches: &[RecordBatch],
) -> Result<Option<Vec<u8>>, parquet::errors::ParquetError> {
    let Some(first) = batches.first() else {
        return Ok(None);
    };
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(DEFAULT_CHUNK_SIZE as usize))
        .build();
    let mut buf = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, first.schema(), Some(props))?;
    for batch in batches {
        writer.write(batch)?;
    }
    writer.close()?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::Arc;

    use datafusion::arrow::array::UInt32Array;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use parquet::file::metadata::ParquetMetaDataReader;

    use super::{DEFAULT_CHUNK_SIZE, batches_to_parquet};

    /// A row group is a tile, and the last one is the remainder.
    ///
    /// This is the whole of ADR-0056 §2 as a test: the footer of a written type
    /// must carry one `x`/`y` box per addressable tile, and it does that only if
    /// the row groups are cut at the tile boundary. What it cannot prove is that
    /// the boxes prune well — that is a property of the Morton order upstream,
    /// and `examples/tile_layout.rs` is where it is measured.
    #[test]
    fn a_row_group_is_a_tile() {
        let tile = DEFAULT_CHUNK_SIZE as usize;
        let rows = tile * 2 + 7;
        let schema = Arc::new(Schema::new(vec![Field::new(
            "dense_id",
            DataType::UInt32,
            false,
        )]));
        let batch = datafusion::arrow::record_batch::RecordBatch::try_new(
            schema,
            vec![Arc::new(UInt32Array::from_iter_values(0..rows as u32))],
        )
        .expect("one column, one schema");

        let bytes = batches_to_parquet(&[batch])
            .expect("encode")
            .expect("a non-empty batch encodes");
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(&bytes).expect("spill");

        let meta = ParquetMetaDataReader::new()
            .parse_and_finish(f.as_file())
            .expect("parse footer");
        let counts: Vec<i64> = meta.row_groups().iter().map(|g| g.num_rows()).collect();
        assert_eq!(counts, vec![tile as i64, tile as i64, 7]);
    }
}
