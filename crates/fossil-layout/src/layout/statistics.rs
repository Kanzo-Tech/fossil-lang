//! A closed footer, read into the tile manifest's entries.
//!
//! The statistics a planner prunes on are the ones the encoder already wrote
//! into each row group's footer, so they are read back off the
//! [`ParquetMetaData`] the writer returns on close rather than computed a second
//! time over the rows. That is how Iceberg's writers fill a manifest, and it is
//! what makes "the manifest matches the footers" true by construction here and
//! checked, rather than assumed, by the guard that reads the footers back from
//! disk. [`fossil_sinks::tiles`] has the model and which columns carry bounds.

use arrow::datatypes::{DataType, Schema};
use fossil_sinks::tiles::{Bound, TileManifest, TileMatrix, TileStatistics};
use parquet::file::metadata::ParquetMetaData;
use parquet::file::statistics::Statistics;

/// One entry per row group of a closed container, in row-group order.
///
/// Row group `k` is tile `k`: a vertex zoom has no empty tile, so the ordinals
/// are the tile numbers. `schema` is the Arrow schema the container was written
/// with — the footer's physical types cannot tell a `uint32` from an `int32`,
/// and the bound has to be read in the column's own domain.
pub(super) fn tile_statistics(meta: &ParquetMetaData, schema: &Schema) -> Vec<TileStatistics> {
    meta.row_groups()
        .iter()
        .enumerate()
        .map(|(tile, group)| {
            let mut entry = TileStatistics {
                tile: tile as u64,
                record_count: u64::try_from(group.num_rows()).unwrap_or(0),
                null_value_counts: std::collections::BTreeMap::new(),
                lower_bounds: std::collections::BTreeMap::new(),
                upper_bounds: std::collections::BTreeMap::new(),
            };
            for column in group.columns() {
                let name = column.column_descr().name().to_string();
                let Some(stats) = column.statistics() else {
                    continue;
                };
                if let Some(nulls) = stats.null_count_opt() {
                    entry.null_value_counts.insert(name.clone(), nulls);
                }
                let Ok(field) = schema.field_with_name(&name) else {
                    continue;
                };
                if let Some((lower, upper)) = bounds(stats, field.data_type()) {
                    entry.lower_bounds.insert(name.clone(), lower);
                    entry.upper_bounds.insert(name, upper);
                }
            }
            entry
        })
        .collect()
}

/// A type's tile manifest, out of its payload's entries and its rungs', the
/// rungs in rung order — rung 1, the finest, first.
///
/// The zooms are `/docs/design/backend`'s: rung `k` of `R` is `z = R − k` and
/// the payload is `z = R`, so the list comes out coarsest first.
pub(super) fn manifest(
    payload: Vec<TileStatistics>,
    rungs: Vec<Vec<TileStatistics>>,
) -> TileManifest {
    let top = u32::try_from(rungs.len()).unwrap_or(u32::MAX);
    let mut matrices: Vec<TileMatrix> = rungs
        .into_iter()
        .rev()
        .zip(0..)
        .map(|(tiles, z)| TileMatrix { z, tiles })
        .collect();
    matrices.push(TileMatrix {
        z: top,
        tiles: payload,
    });
    TileManifest { matrices }
}

/// A column chunk's min and max in the column's own domain, or `None` for a
/// column that carries no bound — see [`fossil_sinks::tiles`] for which.
///
/// The casts reinterpret and never truncate: an unsigned column is stored in the
/// signed physical type of its width, and the encoder orders its statistics as
/// unsigned, so the bits are the unsigned value.
#[allow(clippy::cast_sign_loss)]
fn bounds(stats: &Statistics, data_type: &DataType) -> Option<(Bound, Bound)> {
    let pair = |lo: Bound, hi: Bound| Some((lo, hi));
    match (stats, data_type) {
        (Statistics::Int32(v), DataType::UInt8 | DataType::UInt16 | DataType::UInt32) => pair(
            Bound::UInt(u64::from(*v.min_opt()? as u32)),
            Bound::UInt(u64::from(*v.max_opt()? as u32)),
        ),
        (Statistics::Int32(v), DataType::Int8 | DataType::Int16 | DataType::Int32) => pair(
            Bound::signed(i64::from(*v.min_opt()?)),
            Bound::signed(i64::from(*v.max_opt()?)),
        ),
        (Statistics::Int64(v), DataType::UInt64) => pair(
            Bound::UInt(*v.min_opt()? as u64),
            Bound::UInt(*v.max_opt()? as u64),
        ),
        (Statistics::Int64(v), DataType::Int64) => {
            pair(Bound::signed(*v.min_opt()?), Bound::signed(*v.max_opt()?))
        }
        (Statistics::Float(v), DataType::Float32) => {
            pair(Bound::float(*v.min_opt()?), Bound::float(*v.max_opt()?))
        }
        (Statistics::Double(v), DataType::Float64) => {
            pair(Bound::double(*v.min_opt()?), Bound::double(*v.max_opt()?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{Float32Array, Int32Array, RecordBatch, StringArray, UInt32Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use fossil_sinks::tiles::Bound;
    use fossil_tile_writer::TileWriter;

    use super::tile_statistics;

    /// Two tiles, each read back in its column's own domain: an unsigned id past
    /// `i32::MAX` stays unsigned, a negative year stays signed, a float stays the
    /// `f32` it was, and a string is counted and not bounded.
    #[test]
    fn a_closed_footer_reads_back_as_one_entry_per_tile() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("dense_id", DataType::UInt32, false),
            Field::new("year", DataType::Int32, true),
            Field::new("x", DataType::Float32, false),
            Field::new("subject", DataType::Utf8, true),
        ]));
        let batch = |ids: Vec<u32>, years: Vec<Option<i32>>, xs: Vec<f32>| {
            let subjects: Vec<Option<&str>> = ids.iter().map(|_| Some("s")).collect();
            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(UInt32Array::from(ids)),
                    Arc::new(Int32Array::from(years)),
                    Arc::new(Float32Array::from(xs)),
                    Arc::new(StringArray::from(subjects)),
                ],
            )
            .expect("batch")
        };
        let mut buf = Vec::new();
        let mut writer = TileWriter::new(&mut buf, Arc::clone(&schema)).expect("open");
        writer
            .tile(&batch(vec![0, 1], vec![Some(-4), None], vec![0.1, 0.3]))
            .expect("tile 0");
        writer
            .tile(&batch(
                vec![3_000_000_000, 3_000_000_001],
                vec![Some(1990), Some(2001)],
                vec![-1.5, 2.0],
            ))
            .expect("tile 1");
        let meta = writer.finish().expect("close");

        let tiles = tile_statistics(&meta, &schema);
        assert_eq!(tiles.len(), 2);
        assert_eq!((tiles[0].tile, tiles[0].record_count), (0, 2));
        assert_eq!(tiles[0].null_value_counts["year"], 1);
        assert_eq!(tiles[0].null_value_counts["subject"], 0);
        assert_eq!(tiles[0].lower_bounds["year"], Bound::Int(-4));
        assert_eq!(tiles[0].upper_bounds["x"], Bound::float(0.3));
        assert!(!tiles[0].lower_bounds.contains_key("subject"));
        assert_eq!(
            tiles[1].lower_bounds["dense_id"],
            Bound::UInt(3_000_000_000)
        );
        assert_eq!(tiles[1].upper_bounds["year"], Bound::UInt(2001));
        assert_eq!(tiles[1].lower_bounds["x"], Bound::float(-1.5));
    }
}
