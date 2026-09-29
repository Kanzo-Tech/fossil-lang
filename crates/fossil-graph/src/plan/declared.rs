//! What a vertex document declares beyond its payload's address — the zooms
//! below the payload (`cells:`), where their statistics are (`tile_manifest:`),
//! which coordinate system `x` and `y` are in (`coordinates:`) and what a view
//! can colour by (`channels:`) — read the way the rest of [`super`] reads a
//! manifest: off a generic [`Value`], asking the questions a stranger asks.
//!
//! **A rung is not a projection**, and it is not addressed as one. A projection
//! is the payload's rows at some scale; a rung's rows are cells, which exist
//! nowhere in the source (`/docs/design/cells`). What they share is the cut: a
//! rung is tiled at the type's own `chunk_size`, and cell `c` of rung `k` is
//! `dense_id >> shift(k)`, so tile `t` of a rung covers the `dense_id` range
//! `[t · chunk_size · 2^shift, (t+1) · chunk_size · 2^shift)` — which is what
//! lets `/docs/design/backend` call the rungs the zooms of one tile matrix.
//!
//! The shift is `fossil_sinks::manifest::CellTree::shift_at`, reached rather
//! than respelled: the base is declared per type and the octave is written in
//! exactly one place.

use fossil_sinks::manifest::CellTree;
use serde::Serialize;
use serde_yaml_ng::Value;

use super::{
    Container, PROJECTION_STEM, invalid, join, mapping, prefix_of, required, scalar,
    sequence_of_mappings, sequence_of_strings, shift_for, tile_url_for, tiles_of,
};
use crate::Result;

/// A vertex type's cell tree, resolved — the rungs finest first, as the
/// document lists them.
#[derive(Debug, Clone, Serialize)]
pub struct CellsAddress {
    /// The declared base, in vertices per cell. A power of four.
    pub vertices_per_cell: u64,
    /// The relations the partition was computed over, by label — the edges a
    /// rung's quotient and a cell's internal weight sum.
    pub relations: Vec<String>,
    /// Which of the type's channels a rung's `mode` is the mode of, by name.
    pub mode_channel: Option<String>,
    /// Every rung, finest first.
    pub rungs: Vec<RungAddress>,
}

/// One rung: where its tiles are and which ids a row of it stands for.
#[derive(Debug, Clone, Serialize)]
pub struct RungAddress {
    /// `k`, from 1 at the finest.
    pub rung: u32,
    /// Where its tiles are, resolved against the corpus base, with a trailing
    /// separator.
    pub prefix: String,
    /// How many bits of `dense_id` a cell id drops — `B + 2(k − 1)`.
    pub shift: u32,
    /// How many cells it has — the document's `cell_count`.
    pub cell_count: u64,
    /// Rows per tile: the type's own `chunk_size`, which a rung does not declare
    /// a second time.
    pub chunk_size: u64,
    /// `ceil(cell_count / chunk_size)`.
    pub tiles: u64,
    /// Which container carries its tiles. The corpus's.
    pub container: Container,
    /// The quotient beside it, where the writer published one.
    pub quotient: Option<QuotientAddress>,
}

/// A rung's quotient: the edges between its cells, tiled on the source cell's
/// tile.
#[derive(Debug, Clone, Serialize)]
pub struct QuotientAddress {
    /// Where its tiles are, with a trailing separator.
    pub prefix: String,
    /// How many quotient edges it holds — rows, not their summed weight.
    pub edge_count: u64,
}

impl RungAddress {
    /// The file tile `t` of this rung is in, spelled by the corpus's container.
    #[must_use]
    pub fn tile_url(&self, tile: u64) -> String {
        tile_url_for(&self.prefix, PROJECTION_STEM, self.container, tile)
    }

    /// The file tile `t` of this rung's quotient is in, or `None` where the
    /// rung publishes none.
    #[must_use]
    pub fn quotient_tile_url(&self, tile: u64) -> Option<String> {
        self.quotient
            .as_ref()
            .map(|q| tile_url_for(&q.prefix, PROJECTION_STEM, self.container, tile))
    }
}

/// The `cells:` block of a vertex document, or `None` where it has none.
///
/// Refused rather than degraded where it cannot be addressed: a base that is not
/// a power of four has no shift, and a rung with no `path` names tiles nobody
/// can ask for. A tree with no rungs is a legal tree that publishes no bytes.
pub(super) fn cells_address(
    vertex_prefix: &str,
    path: &str,
    doc: &Value,
    chunk_size: u64,
    container: Container,
) -> Result<Option<CellsAddress>> {
    let Some(tree) = mapping(doc, path, "cells")? else {
        return Ok(None);
    };
    let base = required(tree, path, "vertices_per_cell")?;
    let vertices_per_cell = base
        .parse::<u64>()
        .ok()
        .filter(|&b| CellTree::base_bits(b).is_some())
        .ok_or_else(|| {
            invalid(format!(
                "{path} declares a cell base of {base}, which is not a power of four, so no \
                 shift addresses a cell"
            ))
        })?;
    let tree_prefix = prefix_of(&join(&[vertex_prefix, &required(tree, path, "prefix")?]));
    let mut rungs = Vec::new();
    for (entry, rung) in sequence_of_mappings(tree, "rungs").into_iter().zip(1u32..) {
        let location = required(entry, path, "path")?;
        let prefix = prefix_of(&join(&[&tree_prefix, &location]));
        let raw = required(entry, path, "cell_count")?;
        let cell_count = raw.parse::<u64>().map_err(|_| {
            invalid(format!(
                "{path} declares rung {rung} with {raw} cells, which is not a count"
            ))
        })?;
        // `base_bits` held above, so the shift exists for every rung.
        let shift = CellTree::shift_at(vertices_per_cell, rung).unwrap_or(u32::MAX);
        let quotient = match mapping(entry, path, "quotient")? {
            None => None,
            Some(declared) => Some(QuotientAddress {
                prefix: prefix_of(&join(&[&prefix, &required(declared, path, "path")?])),
                edge_count: scalar(declared, "edge_count")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0),
            }),
        };
        rungs.push(RungAddress {
            rung,
            prefix,
            shift,
            cell_count,
            chunk_size,
            tiles: tiles_of(cell_count, chunk_size).unwrap_or(0),
            container,
            quotient,
        });
    }
    debug_assert!(
        shift_for(chunk_size).is_some(),
        "the caller checked the cut"
    );
    Ok(Some(CellsAddress {
        vertices_per_cell,
        relations: sequence_of_strings(tree, "relations"),
        mode_channel: scalar(tree, "mode_channel").filter(|m| !m.is_empty()),
        rungs,
    }))
}

/// One channel a view can draw a type with — the document's `channels:` entry,
/// as a stranger reads it.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelAddress {
    /// The name a cell tree's `mode_channel` refers to it by.
    pub name: String,
    /// The payload column it reads.
    pub column: String,
    /// `categorical` or `quantitative`, as the document spells it.
    pub scale: String,
    /// A categorical's value count over the writer's ordinal `0..domain`, where
    /// declared — the whole of what a legend is drawn from.
    pub domain: Option<u64>,
    /// What computed the column, where the writer did.
    pub derived_by: Option<String>,
}

/// The `channels:` entries, in document order. An entry naming no `name` or no
/// `column` is not a channel and is left out.
pub(super) fn channels_of(doc: &Value) -> Vec<ChannelAddress> {
    sequence_of_mappings(doc, "channels")
        .into_iter()
        .filter_map(|entry| {
            Some(ChannelAddress {
                name: scalar(entry, "name").filter(|n| !n.is_empty())?,
                column: scalar(entry, "column").filter(|c| !c.is_empty())?,
                scale: scalar(entry, "scale").unwrap_or_default(),
                domain: scalar(entry, "domain").and_then(|d| d.parse().ok()),
                derived_by: scalar(entry, "derived_by").filter(|d| !d.is_empty()),
            })
        })
        .collect()
}

/// The name of the coordinate system the payload's `x` and `y` columns are
/// in, or `None` where the document declares none that uses them.
pub(super) fn coordinates_of(doc: &Value) -> Option<String> {
    sequence_of_mappings(doc, "coordinates")
        .into_iter()
        .find(|system| {
            scalar(system, "x").as_deref() == Some("x")
                && scalar(system, "y").as_deref() == Some("y")
        })
        .and_then(|system| scalar(system, "name"))
}

/// Where the type's tile manifest is, resolved, or `None` where the document
/// names none — a corpus written before the field, whose reader reads footers.
pub(super) fn tile_manifest_path(vertex_prefix: &str, doc: &Value) -> Option<String> {
    scalar(doc, "tile_manifest")
        .filter(|name| !name.trim().is_empty())
        .map(|name| join(&[vertex_prefix, name.trim()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Value {
        serde_yaml_ng::from_str(text).expect("yaml")
    }

    #[test]
    fn a_rung_is_the_type_cut_at_a_wider_shift() {
        let tree = doc(
            "cells:\n  prefix: cell/\n  vertices_per_cell: 16\n  relations: [knows]\n  \
             mode_channel: community\n  rungs:\n  - path: r1/\n    cell_count: 62500\n    \
             quotient:\n      path: quotient/\n      edge_count: 9\n  - path: r2/\n    \
             cell_count: 15625\n",
        );
        let cells = cells_address(
            "vertex/Node/",
            "Node.vertex.yml",
            &tree,
            4096,
            Container::RowGroups,
        )
        .expect("resolves")
        .expect("declared");
        assert_eq!(cells.relations, ["knows"]);
        assert_eq!(cells.mode_channel.as_deref(), Some("community"));
        let [r1, r2] = cells.rungs.as_slice() else {
            panic!("two rungs");
        };
        assert_eq!((r1.rung, r1.shift, r1.tiles), (1, 4, 16));
        assert_eq!((r2.rung, r2.shift, r2.tiles), (2, 6, 4));
        assert_eq!(r1.tile_url(3), "vertex/Node/cell/r1/tiles.parquet");
        assert_eq!(
            r1.quotient_tile_url(0).as_deref(),
            Some("vertex/Node/cell/r1/quotient/tiles.parquet")
        );
        assert_eq!(r2.quotient_tile_url(0), None);
    }

    #[test]
    fn a_base_that_is_not_a_power_of_four_has_no_shift() {
        let tree =
            doc("cells:\n  prefix: cell/\n  vertices_per_cell: 8\n  relations: []\n  rungs: []\n");
        assert!(cells_address("v/", "v.yml", &tree, 64, Container::Files).is_err());
    }

    #[test]
    fn channels_and_the_coordinate_system_are_read_as_declared() {
        let declared = doc(
            "coordinates:\n- name: layout\n  x: x\n  y: y\n  provenance: derived\n\
             channels:\n- name: community\n  column: cluster_id\n  scale: categorical\n  \
             domain: 16\n- name: age\n  column: birth_year\n  scale: quantitative\n",
        );
        assert_eq!(coordinates_of(&declared).as_deref(), Some("layout"));
        let channels = channels_of(&declared);
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].domain, Some(16));
        assert_eq!(channels[1].domain, None);
        assert_eq!(channels[1].scale, "quantitative");
    }

    #[test]
    fn the_tile_manifest_is_beside_the_tiles() {
        let named = doc("tile_manifest: tile-manifest.json\n");
        assert_eq!(
            tile_manifest_path("vertex/Person/", &named).as_deref(),
            Some("vertex/Person/tile-manifest.json")
        );
        assert_eq!(
            tile_manifest_path("vertex/Person/", &doc("type: Person\n")),
            None
        );
    }
}
