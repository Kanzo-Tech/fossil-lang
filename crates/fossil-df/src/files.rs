//! The GraphAr output as in-memory bytes — the dataset's manifests, and the
//! baseline Parquet encoder the tiling benches measure against.
//!
//! Universal-substrate decision: ONE parquet encoder in Rust (parquet-rs,
//! native + wasm), no parquet-wasm JS.
//!
//! # What this module emits, and what it stopped emitting
//!
//! [`GraphArData::manifest_files`] turns the materialised graph into the
//! dataset's **manifest YAMLs** as `(rel_path, bytes)` pairs. It used to emit
//! the payload too — one Parquet per vertex type and the CSR/CSC pair per edge —
//! and the layout pass then read every one of those back, rewrote it as tiles
//! and deleted it. Measured on com-DBLP: 115.9 MB written against 72.6 MB of
//! final corpus, **43.3 MB staged and deleted, a 1.60× write amplification**.
//!
//! So the payload is written **once**, by `fossil_layout`, out of the same
//! `RecordBatch`es this module would have encoded — through
//! `fossil_tile_writer::TileWriter`, which was a struct in this file and is a
//! crate of its own now: nothing in `fossil-df` ever called it, and the one
//! `use` in the layout pass was what put `salsa` and `DataFusion` into that
//! pass's dependency closure.
//!
//! Pure in-memory (`Vec<u8>` writer), so nothing here touches the filesystem and
//! all of it compiles to `wasm32`.

use datafusion::arrow::record_batch::RecordBatch;
use fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::GraphArData;

/// One output file of the GraphAr dataset: its dataset-relative path
/// (`graph.graph.yml`, `vertex/<Type>.vertex.yml`, `vertex/<Type>/tiles.parquet`,
/// …) and its encoded bytes. The path keys both the on-disk layout (native sink)
/// and the signed-`PUT` object key (browser host).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphArFile {
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Encoding the GraphAr dataset to bytes failed — Parquet encode (through
/// [`batches_to_parquet`]) or manifest YAML serialization.
/// Filesystem errors live in [`crate::sink::SinkError`] (native-only); this
/// stays wasm-clean.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("parquet encode: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("manifest yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

impl GraphArData {
    /// The dataset's manifest YAMLs as in-memory `(rel_path, bytes)` files.
    ///
    /// **The payload is not here** — see the module header. What describes the
    /// corpus is written by this crate; what the corpus *is* comes out of
    /// `fossil_layout`'s pass, which holds the same batches and the addresses to
    /// cut them on.
    ///
    /// # Errors
    /// Manifest YAML serialization failures.
    pub fn manifest_files(&self) -> Result<Vec<GraphArFile>, EncodeError> {
        let mut out = Vec::new();
        self.try_for_each_manifest::<EncodeError>(|file| {
            out.push(file);
            Ok(())
        })?;
        Ok(out)
    }

    /// The same manifests, one at a time: each is serialized, handed to `emit`,
    /// and dropped before the next one is built.
    ///
    /// [`Self::manifest_files`] is this with a `Vec` on the end. The native
    /// [`crate::sink`] writes each one to a directory and forgets it; the wasm
    /// host collects them, because it hands JS a list.
    ///
    /// # Errors
    /// Whatever `emit` returns, or a serialization failure converted through `E`.
    pub fn try_for_each_manifest<E: From<EncodeError>>(
        &self,
        mut emit: impl FnMut(GraphArFile) -> Result<(), E>,
    ) -> Result<(), E> {
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
/// **Nothing in the write path calls this any more**, and that is the point of
/// the phase inversion: it encoded the writer's staged payload, which the layout
/// pass then read back and replaced. What still calls it is
/// `examples/tile_layout.rs` and `fossil-layout`'s `examples/compaction_pass.rs`
/// — the benches that measure `fossil_tile_writer::TileWriter` against the
/// alternative of one Parquet per tile, which is the comparison this function is
/// the baseline of. It is kept for that and for being the statement of the
/// row-group property that writer enforces by cutting.
///
/// **One row group per tile.** The only property set is the row-group row count,
/// and it is [`DEFAULT_CHUNK_SIZE`] — the same 4,096 rows of `dense_id` that
/// address a tile. `parquet`'s default is 1,048,576 rows, which puts a whole
/// five-million-row type in five row groups and leaves the footer with five
/// `x`/`y` boxes to prune with. With this set the footer carries one box per
/// tile, and that footer IS the reader's index: arithmetic gives which tiles
/// exist, but only the boxes give which ones intersect a window. The row-group
/// size is the only Parquet property this function fixes.
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
    /// This is the rule as a test: the footer of a written type
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
