//! One payload set as ONE Parquet whose row groups **are** its tiles.
//!
//! This is the row-group container of
//! `/docs/format/conventions/addressing#two-containers-one-address`, and it is
//! the only byte-writer of a corpus's payload in the tree. Pure in-memory — the
//! sink is a `W: Write`, so nothing here touches a filesystem and all of it
//! compiles to `wasm32`.
//!
//! # Why it is a crate
//!
//! It was a module of `fossil-df`, which never called it. Its only consumer is
//! `fossil_layout::layout`, and that one `use` put `datafusion`, `fossil-hir`,
//! `fossil-mir`, `fossil-descriptors-output`, `fossil-base` and **`salsa`** into
//! the closure of a batch pass that holds no database and resolves no name —
//! a compiler substrate paid for by a 37-line Parquet writer.
//!
//! `fossil-sinks` is not the home either, for the two reasons its own
//! `Cargo.toml` states: it takes `arrow-schema` alone so that `arrow-ipc` and
//! `mio` stay out of the browser bundle, and it declares the tiling without
//! byte-writing any of it. See this crate's `Cargo.toml`.

use std::io::Write;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

/// One payload set as ONE Parquet whose row groups **are** its tiles: each
/// [`Self::tile`] closes a row group, so the `k`th call is row group `k`.
///
/// It is the other half of the property `fossil_df::files::batches_to_parquet`
/// states: a tile is a row group either way, and what changes is whether the
/// file boundary sits between them. Measured at five million in 1,221 tiles:
/// 5.6 range requests per window against 22.3, and a 496,373 B footer in one
/// piece against 1,150,490 B in 1,221.
///
/// **The cut is explicit and not a row count**, which is what lets an adjacency
/// use the same writer as a vertex payload. A vertex tile is exactly
/// `chunk_size` gapless `dense_id`s; an adjacency tile is however many edges its
/// vertices happen to have, so no `max_row_group_row_count` describes both. Both
/// automatic limits are therefore `None` — the documented spelling for "one row
/// group until told otherwise" — and [`Self::tile`] is the telling.
///
/// **`DuckDB` cannot do this under 2,048 rows and `arrow-rs` can.** Row groups
/// come out of `DuckDB` in multiples of its 2,048-row vector and a smaller
/// `ROW_GROUP_SIZE` is clamped in silence. `parquet`'s `ArrowWriter` cuts where
/// it is told: `a_tile_can_be_smaller_than_duckdbs_vector` writes 64-row tiles
/// and reads back 64-row row groups.
///
/// The sink is generic so the bytes can go straight to a `File` — the layout
/// pass writes a whole vertex type through one of these, and buffering it into a
/// `Vec` first would put the encoded corpus beside the corpus.
pub struct TileWriter<W: Write + Send>(ArrowWriter<W>);

/// Hand-written because `ArrowWriter` has no `Debug`, and the workspace warns on
/// a `pub` type without one. There is nothing to print that is not either the
/// sink's business or the footer's: the row groups closed so far are the
/// writer's only state and `parquet` does not expose a count.
impl<W: Write + Send> std::fmt::Debug for TileWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TileWriter(..)")
    }
}

impl<W: Write + Send> TileWriter<W> {
    /// Open a row-group container over `sink`.
    ///
    /// # Errors
    /// Parquet encode failures.
    pub fn new(sink: W, schema: SchemaRef) -> Result<Self, parquet::errors::ParquetError> {
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(None)
            .set_max_row_group_bytes(None)
            .build();
        Ok(Self(ArrowWriter::try_new(sink, schema, Some(props))?))
    }

    /// Write one tile as one row group. An empty tile writes nothing, so it
    /// consumes no ordinal — which is why a row-group ordinal addresses a
    /// fixed-stride set and an adjacency is addressed by its footer box.
    ///
    /// # Errors
    /// Parquet encode failures.
    pub fn tile(&mut self, batch: &RecordBatch) -> Result<(), parquet::errors::ParquetError> {
        if batch.num_rows() == 0 {
            return Ok(());
        }
        self.0.write(batch)?;
        self.0.flush()
    }

    /// Close the file, writing the footer that IS the reader's index.
    ///
    /// # Errors
    /// Parquet encode failures.
    pub fn finish(self) -> Result<(), parquet::errors::ParquetError> {
        self.0.close().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::Arc;

    use arrow_array::{RecordBatch, UInt32Array};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::file::metadata::{ParquetMetaDataReader, RowGroupMetaData};

    use super::TileWriter;

    /// The row-group container cuts where it is told, and a tile of 64 rows is
    /// 64 rows.
    ///
    /// This is the limit that decides which container a corpus can be in, and it
    /// belongs to a writer rather than to the format: `DuckDB` emits row groups in
    /// multiples of its 2,048-row vector and clamps a smaller `ROW_GROUP_SIZE`
    /// without a warning, so 300 rows at `ROW_GROUP_SIZE 64` come back as ONE
    /// group of 300 — which is why `apps/corpus`'s fixture, which writes through
    /// `DuckDB`, cannot put a `chunk_size` 64 corpus in this container.
    ///
    /// fossil writes through `arrow-rs`, so the limit is not fossil's. The
    /// numbers below are the ones `DuckDB` cannot produce: five tiles of a 300-row
    /// type at 64 rows, the last one short.
    #[test]
    fn a_tile_can_be_smaller_than_duckdbs_vector() {
        let tile = 64u32;
        let rows = 300u32;
        let schema = Arc::new(Schema::new(vec![Field::new(
            "dense_id",
            DataType::UInt32,
            false,
        )]));
        let column = |lo: u32, len: u32| {
            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![Arc::new(UInt32Array::from_iter_values(lo..(lo + len)))],
            )
            .expect("one column, one schema")
        };

        let mut buf = Vec::new();
        let mut writer =
            TileWriter::new(&mut buf, Arc::clone(&schema) as _).expect("open the container");
        for k in 0..rows.div_ceil(tile) {
            let lo = k * tile;
            writer
                .tile(&column(lo, (rows - lo).min(tile)))
                .expect("one tile is one row group");
        }
        writer.finish().expect("close the container");

        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(&buf).expect("spill");
        let meta = ParquetMetaDataReader::new()
            .parse_and_finish(f.as_file())
            .expect("parse footer");
        let counts: Vec<i64> = meta
            .row_groups()
            .iter()
            .map(RowGroupMetaData::num_rows)
            .collect();
        assert_eq!(counts, vec![64, 64, 64, 64, 44]);
    }
}
