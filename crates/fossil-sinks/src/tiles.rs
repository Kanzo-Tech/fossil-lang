//! The tile manifest: every tile of one vertex type, at every zoom, with the
//! statistics a planner prunes on.
//!
//! **Iceberg's manifest, and its vocabulary.** A manifest there is one entry
//! per data file carrying `record_count`, `null_value_counts`, `lower_bounds`
//! and `upper_bounds`, and a scan is planned against those entries without
//! opening a file. Here an entry is a **tile** — one row group of the payload at
//! `z = Z`, or of a rung below it — because a tile is the unit a read selects
//! and a view caches. The four statistic names are Iceberg's, spelled as it
//! spells them, so a reader who knows one manifest recognises the other.
//!
//! **The numbers are the footers' and nothing else.** A writer that computed
//! them a second way would publish a second answer to a question the Parquet
//! already answers, and the two could disagree. So the writer reads them off
//! the footer it has just closed — which is how Iceberg's own writers fill a
//! manifest (`ParquetUtil.footerMetrics`, iceberg-rust's
//! `DataFileBuilder` over the `ParquetMetaData` its writer returns) — and a
//! guard over a written corpus holds the published figures against the footers
//! read back from disk.
//!
//! **Which columns carry bounds.** Every column carries a null count; only a
//! column of an integer or floating-point type carries a lower and an upper
//! bound. A string's footer bound is truncated to 64 bytes by the encoder, is
//! ordered by UTF-8 bytes where a JavaScript planner compares UTF-16 code units,
//! and over `subject` — the one string every type has — prunes nothing, because
//! tiles are in Hilbert order and identities are not. What would reverse it is a
//! string filter measured to prune a tile.
//!
//! **The box is not a field.** A tile's box is the bounds of its coordinate
//! columns, which this already publishes; a `bbox` beside them would be a second
//! statement of four numbers.
//!
//! **The edges a tile addresses are entries too.** At `z = Z` each adjacency
//! orientation aligned on this type lists its tiles, and below it each rung's
//! quotient does — the same four statistics, numbered by the aligned tile
//! rather than by the row group. The two numbers differ there and only there:
//! a tile of sources with no edges writes no row group, so the ordinals stay
//! dense while the tiles skip, and an entry is how a reader learns that a tile
//! has no edges without asking the engine.
//!
//! **No byte range, and that is a finding rather than an omission.**
//! `DuckDB`'s Parquet reader has no read by row group: a tile is selected by one
//! conjunctive range on its aligned column, which the engine prunes against each
//! row group's own statistics in the footer it reads anyway. So a published
//! offset would have no reader, and a document carries a fact exactly when a
//! reader needs it. What would reverse it is an engine that reads a row group
//! by its byte range — a `Range` request a host issues itself.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Every tile of one vertex type, one matrix per zoom, coarsest first.
///
/// `z = 0` is the rung holding a single cell and the payload is the last
/// matrix, `z = Z`; a type with no cell tree has one matrix, its payload, at
/// `z = 0`. The arithmetic is `/docs/design/backend`'s, and a matrix's tiles are
/// the row groups of that zoom's one Parquet, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileManifest {
    /// One entry per zoom, in ascending `z`.
    pub matrices: Vec<TileMatrix>,
}

/// One zoom's tiles, and the tiles of the edges its rows address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileMatrix {
    /// The zoom: `0` for the coarsest rung, `Z` for the payload.
    pub z: u32,
    /// Every tile of this zoom, in tile order. No tile of a vertex zoom is
    /// empty — `dense_id` is gapless and so is a rung's `cell_id` — so the list
    /// is complete and `tiles[t].tile == t`.
    pub tiles: Vec<TileStatistics>,
    /// At `z = Z`, every adjacency orientation whose tiles are cut on this
    /// type's — `by_source` of a relation this type is the source of, and
    /// `by_target` of one it is the target of. Empty below `Z`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjacencies: Vec<AdjacencyTiles>,
    /// Below `Z`, the rung's quotient where the writer published one, tiled on
    /// its source cell's tile. `None` at `Z`, and on a rung whose cells have no
    /// edge between them — which is also when the document declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quotient: Option<Vec<TileStatistics>>,
}

/// One orientation of one relation, as the tiles of the type it is cut on.
///
/// Sparse where a vertex zoom is not: a tile of the aligned type whose vertices
/// have no edge in this orientation has no row group and no entry, so an absent
/// tile is a read that answers nothing and need not be made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjacencyTiles {
    /// The relation's label, as the edge document writes `edge_type`.
    pub edge_type: String,
    /// The type `src_dense` indexes.
    pub src_type: String,
    /// The type `dst_dense` indexes.
    pub dst_type: String,
    /// Which endpoint the tiles are cut on — the edge document's `aligned_by`,
    /// `src` for `by_source` and `dst` for `by_target`.
    pub aligned_by: String,
    /// The orientation's tiles, ascending, each numbered by the aligned type's
    /// tile it holds the edges of.
    pub tiles: Vec<TileStatistics>,
}

/// One tile's entry: Iceberg's `data_file` metrics, for a row group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileStatistics {
    /// The tile's number within its zoom — its row group's ordinal on a vertex
    /// zoom, and the aligned tile on an adjacency or a quotient.
    ///
    /// Stated although it is the entry's position on a vertex zoom, because an
    /// edge set's tiles are sparse: a tile of sources with no edges writes no
    /// row group, so there the ordinal and the tile part.
    pub tile: u64,
    /// Rows in the tile.
    pub record_count: u64,
    /// Nulls per column, for every column the footer counted.
    pub null_value_counts: BTreeMap<String, u64>,
    /// The smallest value per bounded column. Inclusive.
    pub lower_bounds: BTreeMap<String, Bound>,
    /// The largest value per bounded column. Inclusive.
    pub upper_bounds: BTreeMap<String, Bound>,
}

/// A column bound, in the column's own domain.
///
/// Untagged, so it is written as a bare number. A `float` column's bound is its
/// `f32` widened to `f64` — exact, and printed with enough digits that a reader
/// parsing it as a double recovers the `f32` the column holds. Printing the
/// `f32` itself would round `0.1f32` to `0.1`, which is a smaller number than
/// the column's value and would let a planner prune a tile holding the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Bound {
    /// An unsigned integer column.
    UInt(u64),
    /// A signed integer column holding a negative bound.
    Int(i64),
    /// A floating-point column.
    Float(Float),
}

/// An `f64` compared by its bits, so a manifest holding one stays `Eq`.
///
/// Bitwise equality is the equality a guard wants: the published bound and the
/// footer's are the same number or the check fails, `-0.0` against `0.0`
/// included.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Float(pub f64);

impl PartialEq for Float {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Float {}

impl TileManifest {
    /// Serialize to the JSON a reader fetches beside the type's document.
    ///
    /// # Errors
    /// The underlying `serde_json` error, which these plain structs cannot
    /// raise; the signature is honest.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Bound {
    /// A signed value, as the variant a reader parses it back into.
    #[must_use]
    pub fn signed(value: i64) -> Self {
        u64::try_from(value).map_or(Self::Int(value), Self::UInt)
    }

    /// An `f32` column's bound, widened exactly.
    #[must_use]
    pub fn float(value: f32) -> Self {
        Self::Float(Float(f64::from(value)))
    }

    /// An `f64` column's bound.
    #[must_use]
    pub const fn double(value: f64) -> Self {
        Self::Float(Float(value))
    }
}

#[cfg(test)]
mod tests {
    use super::{AdjacencyTiles, Bound, TileManifest, TileMatrix, TileStatistics};

    fn one_tile() -> TileManifest {
        TileManifest {
            matrices: vec![TileMatrix {
                z: 0,
                adjacencies: Vec::new(),
                quotient: None,
                tiles: vec![TileStatistics {
                    tile: 0,
                    record_count: 3,
                    null_value_counts: [("subject".to_string(), 0), ("x".to_string(), 0)]
                        .into_iter()
                        .collect(),
                    lower_bounds: [
                        ("dense_id".to_string(), Bound::UInt(0)),
                        ("x".to_string(), Bound::float(0.1)),
                        ("year".to_string(), Bound::signed(-4)),
                    ]
                    .into_iter()
                    .collect(),
                    upper_bounds: [
                        ("dense_id".to_string(), Bound::UInt(2)),
                        ("x".to_string(), Bound::float(2.5)),
                        ("year".to_string(), Bound::signed(1990)),
                    ]
                    .into_iter()
                    .collect(),
                }],
            }],
        }
    }

    /// A float bound survives a trip through text as the `f32` it was, which is
    /// the property a planner's pruning is only safe under.
    #[test]
    fn an_f32_bound_round_trips_exactly() {
        let bound = Bound::float(0.1);
        let text = serde_json::to_string(&bound).expect("emit");
        let back: Bound = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, bound, "{text}");
        let Bound::Float(wide) = back else {
            panic!("a float bound parsed back as {back:?}");
        };
        #[allow(clippy::cast_possible_truncation)]
        let narrow = wide.0 as f32;
        assert_eq!(narrow.to_bits(), 0.1f32.to_bits());
    }

    /// An edge set's entries are numbered by the aligned tile, so a manifest
    /// that skips one says so in the numbers and keeps saying it after a trip
    /// through text; a vertex zoom with no edge set writes neither key.
    #[test]
    fn edge_entries_round_trip_and_stay_out_of_a_zoom_without_them() {
        let mut manifest = one_tile();
        let text = manifest.to_json().expect("emit");
        assert!(
            !text.contains("adjacencies") && !text.contains("quotient"),
            "{text}"
        );
        let sparse = |tile| TileStatistics {
            tile,
            ..one_tile().matrices[0].tiles[0].clone()
        };
        manifest.matrices[0].adjacencies.push(AdjacencyTiles {
            edge_type: "knows".to_string(),
            src_type: "Person".to_string(),
            dst_type: "Person".to_string(),
            aligned_by: "src".to_string(),
            tiles: vec![sparse(0), sparse(2)],
        });
        manifest.matrices.insert(
            0,
            TileMatrix {
                z: 0,
                tiles: vec![sparse(0)],
                adjacencies: Vec::new(),
                quotient: Some(vec![sparse(0)]),
            },
        );
        let text = manifest.to_json().expect("emit");
        let back: TileManifest = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, manifest, "{text}");
        assert_eq!(back.matrices[1].adjacencies[0].tiles[1].tile, 2);
    }

    #[test]
    fn a_manifest_round_trips() {
        let manifest = one_tile();
        let text = manifest.to_json().expect("emit");
        let back: TileManifest = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, manifest, "{text}");
    }
}
