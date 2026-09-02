//! The address of a corpus, resolved from its manifest — the Rust reader.
//!
//! A tile is a fixed range of `dense_id` and its address is a shift, so a reader
//! computes every URL it wants before it emits the first request. Two readers
//! already do that: `packages/corpus/src/address.ts` (the published module) and
//! `apps/corpus/conformance/reader.mjs` (written from the conventions and from
//! nothing else). This is the third, and it is the one that reaches a browser
//! through `fossil-graph-wasm`.
//!
//! **It does not go through the writer's structs, and that is the whole point.**
//! [`crate::manifest`] deserialises into [`fossil_sinks::manifest`] — the very
//! types that emitted the YAML — which proves those types round-trip and says
//! nothing about the artefact. Everything here reads a generic
//! [`serde_yaml_ng::Value`] and asks the document the questions a stranger asks
//! it: which container, which prefix, how many rows per tile, how many rows.
//! A field the writer renames breaks this module, which is the behaviour a
//! reader in somebody else's repository would have.
//!
//! **Synchronous, and that is the claim rather than an omission.** Nothing here
//! fetches, opens a connection or reads a byte of Parquet: [`VertexAddress::tile_url`]
//! hands back a string. Which tiles a rectangle touches comes from the per-tile
//! `x`/`y` statistics in the Parquet footers, and reading a footer needs a
//! Parquet reader — the host has one, this module would have to grow one, so the
//! host reads its own footers and hands the tile numbers back here.
//!
//! **What pins it to the other two is `apps/corpus/conformance/expected.json`**,
//! a table none of the three wrote. `crates/fossil-graph/tests/conformance.rs`
//! executes it here, `apps/corpus/conformance/verify.mjs` executes it in plain
//! Node, and `packages/corpus/tests/conformance.test.ts` executes it against the
//! published module. An address that moves in one moves away from the other two.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_yaml_ng::Value;

use crate::{GraphError, Result};

/// Dataset-relative location of the aggregate index. The one path a reader is told.
pub const GRAPH_INFO_PATH: &str = "graph.graph.yml";

/// The payload file of a row-group container: one per set, its row groups the tiles.
const TILES_FILE: &str = "tiles.parquet";

/// Which container carries the tiles — `graph.graph.yml`'s own `container`.
///
/// `files` is one Parquet per tile with the address in the name; `rowgroups` is
/// one Parquet per payload set whose row groups are the tiles. Both are addressed
/// by the same arithmetic. It is a manifest field and not something a reader works
/// out, because working it out means listing a directory and there is no listing
/// over HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// One Parquet per tile, the address in the file name.
    Files,
    /// One Parquet per payload set, its row groups the tiles.
    #[serde(rename = "rowgroups")]
    RowGroups,
}

/// Which endpoint column addresses an adjacency's tiles — the manifest's own
/// `aligned_by`. `src` is CSR and `dst` is CSC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Source-aligned: CSR, addressed by `src_dense`.
    Src,
    /// Destination-aligned: CSC, addressed by `dst_dense`.
    Dst,
}

impl Direction {
    /// The endpoint column tile `k` filters on.
    #[must_use]
    pub const fn column(self) -> &'static str {
        match self {
            Self::Src => "src_dense",
            Self::Dst => "dst_dense",
        }
    }

    /// The manifest's own spelling, as `aligned_by` writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Src => "src",
            Self::Dst => "dst",
        }
    }

    /// Parse the manifest's spelling. Anything else is not an orientation.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "src" => Some(Self::Src),
            "dst" => Some(Self::Dst),
            _ => None,
        }
    }
}

/// Why an orientation is missing from a window. Both reasons are honest; they
/// are not the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    /// The caller did not ask for this direction.
    NotRequested,
    /// The manifest does not publish an address for it, so no URL exists to ask for.
    NotDeclared,
}

/// The shift that addresses a tile of `rows` rows, or `None` if no shift does.
///
/// A tile size that is not a power of two forces a division where a shift does,
/// which is why `chunk_size` is a power of two or the corpus has no address.
#[must_use]
pub const fn shift_for(rows: u64) -> Option<u32> {
    if rows == 0 || !rows.is_power_of_two() {
        return None;
    }
    Some(rows.trailing_zeros())
}

/// The tile a `dense_id` lives in — the whole of the addressing scheme.
#[must_use]
pub const fn tile_of(dense_id: u64, shift: u32) -> u64 {
    dense_id >> shift
}

/// How many tiles a declared row count occupies, or `None` when no shift
/// addresses `chunk_size`.
///
/// **This is the arithmetic the manifest's count exists for.** Tiles are
/// addressed and never listed, HTTP gives no directory, so a tree that stops at
/// `chunk{k}` is indistinguishable from one whose tail was never written: a hole
/// in the middle breaks the addressing and is caught, a missing tail breaks
/// nothing at all. The declared count is what says how far the corpus goes.
#[must_use]
pub const fn tiles_of(count: u64, chunk_size: u64) -> Option<u64> {
    if shift_for(chunk_size).is_none() {
        return None;
    }
    Some(count.div_ceil(chunk_size))
}

/// How many rows the last tile holds — `chunk_size` for a count that divides, the
/// remainder otherwise, and `0` for an empty type, which has no last tile because
/// it has none at all.
#[must_use]
pub const fn tail_rows(count: u64, chunk_size: u64) -> Option<u64> {
    match tiles_of(count, chunk_size) {
        None => None,
        Some(0) => Some(0),
        Some(tiles) => Some(count - (tiles - 1) * chunk_size),
    }
}

/// Where a vertex type's identity index lives, and how to address one of its tiles.
///
/// A second copy of the type ordered by identity. It cannot be a column of the
/// payload: one table has one sort, the payload's is Morton because the spatial
/// order IS the id space, and a lookup by identity needs the other one.
#[derive(Debug, Clone, Serialize)]
pub struct IndexAddress {
    /// Where the index tiles are, resolved against the corpus base, with a trailing separator.
    pub prefix: String,
    /// The column the tiles are sorted by, and the one a lookup is keyed on.
    pub ordered_by: String,
    /// Rows per index tile. Unrelated to the payload's: tile `k` here is the `k`th
    /// slice of the SORTED order, not a `dense_id` range.
    pub chunk_size: u64,
    /// `ceil(count / chunk_size)`, or `None` when the manifest declares no `vertex_count`.
    pub tiles: Option<u64>,
    /// Which container carries the index tiles. The corpus's, never a second answer.
    pub container: Container,
}

impl IndexAddress {
    /// `<prefix>tile{k}.parquet`, or `<prefix>tiles.parquet` under `rowgroups`.
    #[must_use]
    pub fn tile_url(&self, tile: u64) -> String {
        tile_url_for(&self.prefix, "tile", self.container, tile)
    }

    /// Every index file, in order and distinct.
    pub fn files(&self, path: &str, vertex_type: &str) -> Result<Vec<String>> {
        let tiles = self.tiles.ok_or_else(|| {
            invalid(format!(
                "{path} declares no vertex_count, so how many index tiles {vertex_type} has is not derivable"
            ))
        })?;
        Ok(distinct((0..tiles).map(|k| self.tile_url(k))))
    }
}

/// Where a vertex type's **written levels** are, and which ones exist.
///
/// **Level `k` is `dense_id % 2^k == 0`, whatever this says.** A level is a
/// predicate over the payload and a written `l{k}/` is a cache of it, so a corpus
/// declaring none draws the identical picture and only reads more. What this
/// block changes is a byte count.
///
/// **The numbers are declared and not derived**, unlike everything else here.
/// `fossil_sinks::manifest::VertexLevels` argues why: a level list is at most
/// three integers whatever the corpus is, and *which* levels a writer spent bytes
/// on is a policy — a reader re-deriving it from `vertex_count` and `chunk_size`
/// would reimplement the writer's plan and 404 the day the plan moved.
///
/// **A level needs no second anchor.** Level `k`'s row `i` is the payload row
/// with `dense_id == i · 2^k`, so tile `j` of a level covers the `dense_id` range
/// `[j · chunk_size · 2^k, (j+1) · chunk_size · 2^k)` — a contiguous run of
/// payload tiles, which the published `codes:` anchor already bounds in Morton
/// space.
#[derive(Debug, Clone, Serialize)]
pub struct LevelAddress {
    /// The levels written, finest first, as the manifest declares them.
    pub levels: Vec<u32>,
    /// Rows per tile within a level set. Declared rather than inherited from the
    /// payload's, because turning a level tile back into a `dense_id` range
    /// multiplies by it and a number that has to be assumed is one a writer can
    /// change in silence.
    pub chunk_size: u64,
    /// `log2(chunk_size)` — a level tile's address is this shift plus the level.
    pub shift: u32,
    /// Which container carries the level tiles. The corpus's, never a second answer.
    pub container: Container,
    /// The vertex type's own prefix, which a level's prefix is relative to.
    #[serde(skip)]
    vertex_prefix: String,
    /// Filename stem of a level set's prefix — `l`, as the writer spells it.
    #[serde(skip)]
    stem: String,
    /// The type's `vertex_count`, for the row and tile counts a level has.
    #[serde(skip)]
    count: Option<u64>,
    /// The manifest file this was read from, for the error messages that name it.
    #[serde(skip)]
    path: String,
}

impl LevelAddress {
    /// Whether level `k` is written.
    ///
    /// `false` is not a refusal and not an absence of the level: the level exists
    /// at every `k` — it is a predicate — and this says only whether reading it
    /// costs the level's bytes or the type's.
    #[must_use]
    pub fn has(&self, level: u32) -> bool {
        self.levels.contains(&level)
    }

    /// Where level `k`'s tiles are, resolved, with a trailing separator.
    #[must_use]
    pub fn prefix(&self, level: u32) -> String {
        prefix_of(&join(&[&self.vertex_prefix, &format!("{}{level}", self.stem)]))
    }

    /// The tile of level `k` holding `dense_id`: a shift by `log2(chunk_size) + k`.
    ///
    /// Never a division, and `k` more bits fall off than the payload's own
    /// address drops — level `k` holds one row in `2^k`, so a tile of it spans
    /// that many times the ids.
    #[must_use]
    pub const fn tile_of(&self, level: u32, dense_id: u64) -> u64 {
        tile_of(dense_id, self.shift.saturating_add(level))
    }

    /// The file tile `j` of level `k` is in, spelled by the corpus's container.
    #[must_use]
    pub fn tile_url(&self, level: u32, tile: u64) -> String {
        tile_url_for(&self.prefix(level), "chunk", self.container, tile)
    }

    /// How many rows level `k` holds — `ceil(count / 2^k)` — or `None` when the
    /// manifest declares no count.
    #[must_use]
    pub fn rows(&self, level: u32) -> Option<u64> {
        self.count.map(|c| c.div_ceil(1u64 << level.min(63)))
    }

    /// How many tiles level `k` has, or `None` when the manifest declares no count.
    #[must_use]
    pub fn tiles(&self, level: u32) -> Option<u64> {
        self.rows(level).and_then(|r| tiles_of(r, self.chunk_size))
    }

    /// Every file of level `k`, in order and distinct.
    ///
    /// Refuses a level nobody wrote by naming what does answer it — the predicate
    /// over the payload — because a URL under `l{k}/` for an unwritten `k` is the
    /// one failure a reader cannot tell from an empty level.
    pub fn files(&self, level: u32) -> Result<Vec<String>> {
        if !self.has(level) {
            let written = self
                .levels
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(invalid(format!(
                "{} writes levels {written} and not {level}, so its files are not addressable — \
                 the predicate over the payload is what answers that level",
                self.path
            )));
        }
        let tiles = self.tiles(level).ok_or_else(|| {
            invalid(format!(
                "{} declares no vertex_count, so how many tiles level {level} has is not derivable",
                self.path
            ))
        })?;
        Ok(distinct((0..tiles).map(|k| self.tile_url(level, k))))
    }
}

/// One vertex type's address: where its tiles are and which `dense_id` range each holds.
#[derive(Debug, Clone, Serialize)]
pub struct VertexAddress {
    /// The type label, e.g. `Person`.
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// Where its tiles are, resolved against the corpus base and with a trailing separator.
    pub prefix: String,
    /// Rows per tile. A power of two, checked at resolve.
    pub chunk_size: u64,
    /// `log2(chunk_size)` — the shift that turns a `dense_id` into a tile number.
    pub shift: u32,
    /// The manifest's `vertex_count`, or `None` when it declares none.
    pub count: Option<u64>,
    /// `ceil(count / chunk_size)`, or `None` when the manifest declares no count.
    pub tiles: Option<u64>,
    /// Which container carries this type's tiles.
    pub container: Container,
    /// The identity index, when the manifest declares one, and `None` otherwise.
    ///
    /// `None` is a legal corpus and not an incomplete one — a lookup by identity
    /// answers without it, by scanning — so a reader that finds none reports the
    /// cost rather than refusing. That is the opposite of [`VertexAddress::count`],
    /// whose absence makes a question unanswerable.
    pub index: Option<IndexAddress>,
    /// The **written levels** of this type, or `None` when the manifest declares
    /// none.
    ///
    /// `None` is a legal corpus and the most legal of the optional blocks: a
    /// level is a predicate, so every level is answerable with or without this,
    /// and what it changes is which bytes answer it. See [`LevelAddress`].
    pub levels: Option<LevelAddress>,
    /// The manifest file this was read from, for the error messages that name it.
    #[serde(skip)]
    path: String,
}

impl VertexAddress {
    /// The tile holding `dense_id`.
    #[must_use]
    pub const fn tile_of(&self, dense_id: u64) -> u64 {
        tile_of(dense_id, self.shift)
    }

    /// The file tile `k` is in: `<prefix>chunk{k}.parquet` under `files`,
    /// `<prefix>tiles.parquet` under `rowgroups`, where every tile of the type
    /// names the same file and the footer's box on `dense_id` says which row
    /// groups are the tile.
    #[must_use]
    pub fn tile_url(&self, tile: u64) -> String {
        tile_url_for(&self.prefix, "chunk", self.container, tile)
    }

    /// Every payload FILE of this type, in order and distinct.
    ///
    /// Refuses when the manifest declares no count, because then there is no set
    /// to enumerate: that is the difference between addressing a tile somebody
    /// asked for and knowing how many there are.
    pub fn files(&self) -> Result<Vec<String>> {
        let tiles = self.tiles.ok_or_else(|| {
            invalid(format!(
                "{} declares no vertex_count, so how many tiles {} has is not derivable — tiles \
                 are addressed and never listed, and HTTP gives no directory to fall back on",
                self.path, self.vertex_type
            ))
        })?;
        Ok(distinct((0..tiles).map(|k| self.tile_url(k))))
    }

    /// Every index file of this type, in order and distinct.
    pub fn index_files(&self) -> Result<Vec<String>> {
        let index = self.index.as_ref().ok_or_else(|| {
            invalid(format!(
                "{} declares no index for {}",
                self.path, self.vertex_type
            ))
        })?;
        index.files(&self.path, &self.vertex_type)
    }
}

/// One orientation of one edge type: declared by the manifest, or absent from it.
#[derive(Debug, Clone, Serialize)]
pub struct AdjacencyAddress {
    /// Which endpoint column addresses these tiles.
    pub direction: Direction,
    /// Where its tiles are, resolved against the corpus base and with a trailing separator.
    pub prefix: String,
    /// The endpoint column tile `k` filters on.
    pub column: &'static str,
    /// `src_chunk_size` for `src`, `dst_chunk_size` for `dst` — a different space
    /// on a cross-type edge.
    pub chunk_size: u64,
    /// `log2(chunk_size)`.
    pub shift: u32,
    /// How many tiles this orientation has — **the endpoint vertex type's tile
    /// count, not the edge's.** An edge tile is addressed by a *vertex* tile, so
    /// `edge_count / chunk_size` is the wrong division and it is wrong quietly.
    pub tiles: Option<u64>,
    /// Which container carries this orientation's tiles. The corpus's, never a second answer.
    pub container: Container,
}

impl AdjacencyAddress {
    /// The tile holding the edges of `dense_id` in this orientation.
    #[must_use]
    pub const fn tile_of(&self, dense_id: u64) -> u64 {
        tile_of(dense_id, self.shift)
    }

    /// `<edge prefix><adj prefix>tile{k}.parquet`. A 404 is "these vertices have
    /// no edges here".
    #[must_use]
    pub fn tile_url(&self, tile: u64) -> String {
        tile_url_for(&self.prefix, "tile", self.container, tile)
    }
}

/// One edge type's address, with an entry per orientation the manifest declares.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeAddress {
    /// The edge label, e.g. `knows`.
    pub edge_type: String,
    /// The source vertex type.
    pub src_type: String,
    /// The destination vertex type.
    pub dst_type: String,
    /// The manifest's `edge_count`, or `None` when it declares none. **One number
    /// for both orientations** — they are one relation stored twice — so it is not
    /// the tile count of either.
    pub count: Option<u64>,
    /// Where the type lives, resolved against the corpus base and with a trailing separator.
    pub prefix: String,
    /// The orientations that resolve to an address — never one the manifest does
    /// not publish. An `adj_lists` entry the manifest omits, or declares without a
    /// `prefix`, is not here.
    pub directions: Vec<Direction>,
    /// The declared orientations, in `directions` order.
    pub adjacencies: Vec<AdjacencyAddress>,
}

impl EdgeAddress {
    /// The declared orientation, or `None` when the corpus does not publish one.
    #[must_use]
    pub fn adjacency(&self, direction: Direction) -> Option<&AdjacencyAddress> {
        self.adjacencies.iter().find(|a| a.direction == direction)
    }
}

/// One orientation of one edge type that a window did not read, and why.
#[derive(Debug, Clone, Serialize)]
pub struct Gap {
    /// The edge label the missing orientation belongs to.
    pub edge_type: String,
    /// The orientation that is missing.
    pub direction: Direction,
    /// Whether the caller did not ask, or the corpus does not publish.
    pub reason: GapReason,
}

/// The files one edge type contributes to a window, distinct, in the orientation
/// that addresses it.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeTiles {
    /// The edge label.
    pub edge_type: String,
    /// The orientation the URLs are in.
    pub direction: Direction,
    /// The files, distinct and in tile order.
    pub urls: Vec<String>,
}

/// The tiles a set of vertex tiles addresses, **and what that set is complete for.**
///
/// CSR alone is complete for *drawing* — every drawable edge has its source on
/// screen, therefore in a tile the window already fetched — and incomplete for
/// *incidence*. So `complete` is about incidence and `gaps` says which
/// orientations are missing from it, separating the caller not asking from the
/// corpus not publishing.
#[derive(Debug, Clone, Serialize)]
pub struct Window {
    /// The vertex type the tile numbers are in the `dense_id` space of.
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// The tiles asked for, in the order they were asked for.
    pub tiles: Vec<u64>,
    /// The files those tiles are in, distinct and in order.
    pub vertex_urls: Vec<String>,
    /// One entry per edge type and orientation actually read.
    pub edges: Vec<EdgeTiles>,
    /// Every URL in `edges`, flattened and distinct, in declaration order.
    pub edge_urls: Vec<String>,
    /// `true` when every edge incident to a vertex in these tiles is in one of these files.
    pub complete: bool,
    /// Which orientations are missing from the incident set, and why.
    pub gaps: Vec<Gap>,
}

/// A corpus resolved to addresses. Every method is pure and synchronous.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedCorpus {
    /// Where the corpus lives; prepended to every URL and nothing else.
    pub base: String,
    /// Which container the corpus declares. One answer for every payload set in it.
    pub container: Container,
    /// Every vertex type, in the order the index names them.
    pub types: Vec<VertexAddress>,
    /// Every edge type, in the order the index names them.
    pub edges: Vec<EdgeAddress>,
}

impl ResolvedCorpus {
    /// One vertex type by name, or the first the index names when no name is given.
    pub fn vertex_type(&self, name: Option<&str>) -> Result<&VertexAddress> {
        let Some(name) = name else {
            // `resolve` refuses a manifest with no vertex type, so this cannot be empty.
            return self
                .types
                .first()
                .ok_or_else(|| invalid(format!("{GRAPH_INFO_PATH} names no vertex type")));
        };
        self.types
            .iter()
            .find(|t| t.vertex_type == name)
            .ok_or_else(|| {
                let named = self
                    .types
                    .iter()
                    .map(|t| t.vertex_type.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                invalid(format!(
                    "no vertex type {name} in {GRAPH_INFO_PATH}; it names {named}"
                ))
            })
    }

    /// The edge types incident to `vertex_type` — as source, as destination, or
    /// both on a self-edge.
    #[must_use]
    pub fn incident(&self, vertex_type: &str) -> Vec<&EdgeAddress> {
        self.edges
            .iter()
            .filter(|e| e.src_type == vertex_type || e.dst_type == vertex_type)
            .collect()
    }

    /// The URLs a set of vertex tiles addresses.
    ///
    /// `directions` of `[Src]` is the drawing read: it fetches the out-edges of
    /// every vertex in the window and reports `complete: false` with a
    /// `not-requested` gap, because a window of drawn vertices has in-edges it did
    /// not ask for. Both orientations is the incident set.
    pub fn window(
        &self,
        vertex_type: Option<&str>,
        tiles: &[u64],
        directions: &[Direction],
    ) -> Result<Window> {
        let vertex = self.vertex_type(vertex_type)?;
        let mut edge_tiles: Vec<EdgeTiles> = Vec::new();
        let mut gaps: Vec<Gap> = Vec::new();

        for edge in self.incident(&vertex.vertex_type) {
            // Only the orientations whose *own* `dense_id` space is this window's.
            // On a cross-type edge `by_target` tile k addresses tile k of the
            // destination type, which is a different set of vertices — reading it
            // for a window over the source type would answer a question nobody
            // asked and call it the neighbourhood.
            let mut applicable: Vec<Direction> = Vec::new();
            if edge.src_type == vertex.vertex_type {
                applicable.push(Direction::Src);
            }
            if edge.dst_type == vertex.vertex_type {
                applicable.push(Direction::Dst);
            }

            for direction in applicable {
                let Some(adjacency) = edge.adjacency(direction) else {
                    gaps.push(Gap {
                        edge_type: edge.edge_type.clone(),
                        direction,
                        reason: GapReason::NotDeclared,
                    });
                    continue;
                };
                if !directions.contains(&direction) {
                    gaps.push(Gap {
                        edge_type: edge.edge_type.clone(),
                        direction,
                        reason: GapReason::NotRequested,
                    });
                    continue;
                }
                edge_tiles.push(EdgeTiles {
                    edge_type: edge.edge_type.clone(),
                    direction,
                    urls: distinct(tiles.iter().map(|&k| adjacency.tile_url(k))),
                });
            }
        }

        let edge_urls = distinct(edge_tiles.iter().flat_map(|e| e.urls.iter().cloned()));
        Ok(Window {
            vertex_type: vertex.vertex_type.clone(),
            tiles: tiles.to_vec(),
            vertex_urls: distinct(tiles.iter().map(|&k| vertex.tile_url(k))),
            edges: edge_tiles,
            edge_urls,
            complete: gaps.is_empty(),
            gaps,
        })
    }
}

/// Resolve a corpus's manifest set into the addresses a reader composes URLs from.
///
/// `manifest_files` is keyed by dataset-relative path — the same shape a host that
/// pre-fetched them holds. `base` is where the corpus lives, without a trailing
/// slash; it is prepended to every URL and nothing else happens to it.
///
/// # Errors
///
/// [`GraphError::InvalidManifest`] when the manifest cannot address itself — a
/// missing file, a `chunk_size` no shift addresses, an endpoint type the index
/// does not declare, or an edge whose declared tile size disagrees with the vertex
/// type that addresses it. It does **not** fail for an orientation the corpus does
/// not publish: that is a legitimate corpus, and it is reported as an address that
/// does not exist rather than one that 404s.
pub fn resolve<S: AsRef<str>>(
    manifest_files: &BTreeMap<String, S>,
    base: &str,
) -> Result<ResolvedCorpus> {
    let read = |path: &str| -> Result<Value> {
        let text = manifest_files.get(path).ok_or_else(|| {
            invalid(format!(
                "the manifest names {path}, which is not among the {} file(s) given",
                manifest_files.len()
            ))
        })?;
        serde_yaml_ng::from_str::<Value>(text.as_ref()).map_err(|e| invalid(format!("{path}: {e}")))
    };

    let index = read(GRAPH_INFO_PATH)?;
    // `prefix` on the index is what the per-type paths are relative to; it is `''`
    // in every corpus fossil writes, and honoured rather than assumed because the
    // field exists to be set.
    let root = join(&[base, scalar(&index, "prefix").unwrap_or_default().as_str()]);
    let container = container_of(&index)?;

    let mut types = Vec::new();
    for path in sequence_of_strings(&index, "vertices") {
        types.push(vertex_address(&root, &path, &read(&path)?, container)?);
    }
    if types.is_empty() {
        return Err(invalid(format!("{GRAPH_INFO_PATH} names no vertex type")));
    }

    let mut edges = Vec::new();
    for path in sequence_of_strings(&index, "edges") {
        edges.push(edge_address(
            &root,
            &path,
            &read(&path)?,
            &types,
            container,
        )?);
    }

    Ok(ResolvedCorpus {
        base: base.to_string(),
        container,
        types,
        edges,
    })
}

// ── the manifest, read the way a stranger reads it ────────────────────────────

const fn invalid(message: String) -> GraphError {
    GraphError::InvalidManifest(message)
}

/// One scalar, stringified the way the JS scanners see it. A YAML number and a
/// YAML string are the same fact to an address.
fn scalar(doc: &Value, key: &str) -> Option<String> {
    match doc.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A scalar the address depends on. Absent is an error, because a default would
/// compose a URL.
fn required(doc: &Value, path: &str, key: &str) -> Result<String> {
    match scalar(doc, key) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(invalid(format!(
            "{path} declares no {key}, so it addresses nothing"
        ))),
    }
}

/// A scalar that has to be a positive integer — a chunk size, and nothing else so far.
fn required_number(doc: &Value, path: &str, key: &str) -> Result<u64> {
    let raw = required(doc, path, key)?;
    match raw.parse::<u64>() {
        Ok(value) if value > 0 => Ok(value),
        _ => Err(invalid(format!(
            "{path} declares {key}: {raw}, which is not a row count"
        ))),
    }
}

/// A declared row count — `vertex_count` or `edge_count` — or `None` when absent.
///
/// `None` rather than `0`: a declared `0` is an empty type and legal, an absent
/// count is a manifest that cannot say how far the corpus goes. Anything that is
/// not a run of decimal digits is `None` — a reader that guessed would be deciding
/// what the writer meant.
fn optional_count(doc: &Value, key: &str) -> Option<u64> {
    let raw = scalar(doc, key)?;
    let raw = raw.trim();
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

/// A sequence of paths — `vertices` and `edges` on the index.
fn sequence_of_strings(doc: &Value, key: &str) -> Vec<String> {
    match doc.get(key) {
        Some(Value::Sequence(items)) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A sequence of small mappings — `adj_lists`, and nothing else so far.
fn sequence_of_mappings<'a>(doc: &'a Value, key: &str) -> Vec<&'a Value> {
    match doc.get(key) {
        Some(Value::Sequence(items)) => items
            .iter()
            .filter(|v| matches!(v, Value::Mapping(_)))
            .collect(),
        _ => Vec::new(),
    }
}

/// A nested mapping under `key`, or `None` when the manifest has none.
///
/// `None` covers both "the key is absent" and "the key is an empty collection",
/// because those are the same fact to a reader: nothing to compose an address
/// from. A key that is a SEQUENCE is not `None` and not a mapping either — that is
/// a manifest saying something this cannot read, and it is refused rather than
/// degraded to "declares none", which is exactly how the index went invisible from
/// the JS scanner before it could see a nested map.
fn mapping<'a>(doc: &'a Value, path: &str, key: &str) -> Result<Option<&'a Value>> {
    match doc.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Sequence(items)) if items.is_empty() => Ok(None),
        Some(Value::Sequence(_)) => Err(invalid(format!(
            "{path} writes {key} as a list, and it is a mapping here"
        ))),
        Some(Value::Mapping(_)) => Ok(doc.get(key)),
        Some(_) => Err(invalid(format!(
            "{path} writes {key} as a scalar, and it is a mapping here"
        ))),
    }
}

/// Join dataset-relative segments the way the manifest writes them: forward
/// slashes, always.
fn join(parts: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let trimmed = if i == 0 {
            part.trim_end_matches('/')
        } else {
            part.trim_matches('/')
        };
        if !trimmed.is_empty() {
            out.push(trimmed);
        }
    }
    out.join("/")
}

/// A prefix as the manifest writes it: dataset-relative, one trailing separator.
fn prefix_of(value: &str) -> String {
    format!("{}/", value.trim_end_matches('/'))
}

/// Where the tiles of one payload set are, in whichever container the corpus
/// declares. One function for the three sets that would otherwise have a copy of
/// it each — the vertex payload, the identity index and each adjacency
/// orientation — because the only thing that ever differs between them is the stem
/// of the filename. Under `rowgroups` even that goes: a set is one file.
fn tile_url_for(prefix: &str, stem: &str, container: Container, tile: u64) -> String {
    match container {
        Container::RowGroups => format!("{prefix}{TILES_FILE}"),
        Container::Files => format!("{prefix}{stem}{tile}.parquet"),
    }
}

/// Distinct, in order. Under `rowgroups` every tile of a set names the same file.
fn distinct<I: IntoIterator<Item = String>>(urls: I) -> Vec<String> {
    let mut seen = Vec::new();
    for url in urls {
        if !seen.contains(&url) {
            seen.push(url);
        }
    }
    seen
}

/// Which container the corpus declares, from `graph.graph.yml`.
///
/// Absent is `files`, and it is the one field here with a default rather than a
/// [`required`]: a corpus written before the field existed is the file-per-tile
/// container, so absence is a statement and not a gap. A third spelling is
/// refused, because it would compose a URL.
fn container_of(index: &Value) -> Result<Container> {
    match scalar(index, "container").as_deref() {
        None | Some("" | "files") => Ok(Container::Files),
        Some("rowgroups") => Ok(Container::RowGroups),
        Some(other) => Err(invalid(format!(
            "{GRAPH_INFO_PATH} declares container {other}; a tile is a file or a row group"
        ))),
    }
}

fn vertex_address(
    root: &str,
    path: &str,
    doc: &Value,
    container: Container,
) -> Result<VertexAddress> {
    let vertex_type = required(doc, path, "type")?;
    let chunk_size = required_number(doc, path, "chunk_size")?;
    let shift = shift_for(chunk_size).ok_or_else(|| {
        invalid(format!(
            "{path} declares a tile of {chunk_size} rows, which no shift addresses"
        ))
    })?;
    let prefix = prefix_of(&join(&[root, &required(doc, path, "prefix")?]));
    let count = optional_count(doc, "vertex_count");
    let tiles = count.and_then(|c| tiles_of(c, chunk_size));
    let index = index_address(&prefix, path, doc, count, container)?;
    let levels = level_address(&prefix, path, doc, count, container)?;

    Ok(VertexAddress {
        vertex_type,
        prefix,
        chunk_size,
        shift,
        count,
        tiles,
        container,
        index,
        levels,
        path: path.to_string(),
    })
}

/// The `levels:` block of a vertex manifest, resolved, or `None` when there is
/// none.
///
/// Every field is required once the block is present, on [`index_address`]'s
/// argument: a block naming a prefix without its level list reads exactly like a
/// corpus that declares no pyramid, and the difference between those two is a
/// reader opening `l6/` or striding a million rows.
fn level_address(
    vertex_prefix: &str,
    path: &str,
    doc: &Value,
    count: Option<u64>,
    container: Container,
) -> Result<Option<LevelAddress>> {
    let Some(declared) = mapping(doc, path, "levels")? else {
        return Ok(None);
    };
    let stem = match scalar(declared, "prefix") {
        Some(v) if !v.is_empty() => v,
        _ => {
            return Err(invalid(format!(
                "{path} declares levels and no prefix, so the tiles they name cannot be composed"
            )));
        }
    };
    let listed = declared
        .get("levels")
        .and_then(Value::as_sequence)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if listed.is_empty() {
        return Err(invalid(format!(
            "{path} declares levels and no level list, so which of them is written is not \
             derivable — and it is a policy, not arithmetic a reader can redo"
        )));
    }
    let levels = listed
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| {
                    invalid(format!(
                        "{path} declares level {}, which is not a level",
                        value.as_str().unwrap_or("a non-scalar")
                    ))
                })
        })
        .collect::<Result<Vec<u32>>>()?;
    let raw = scalar(declared, "chunk_size").unwrap_or_default();
    let chunk_size = raw.parse::<u64>().unwrap_or(0);
    let shift = shift_for(chunk_size).ok_or_else(|| {
        invalid(format!(
            "{path} declares a level tile of {raw} rows, which no shift addresses"
        ))
    })?;
    Ok(Some(LevelAddress {
        levels,
        chunk_size,
        shift,
        container,
        vertex_prefix: vertex_prefix.to_string(),
        stem,
        count,
        path: path.to_string(),
    }))
}

/// The `index:` block of a vertex manifest, resolved, or `None` when there is none.
///
/// Every field is required ONCE the block is present: a `prefix` with no
/// `ordered_by` names files whose sort a reader would have to guess, and guessing
/// it wrong returns a plausible stranger rather than nothing. A half-declared
/// index is refused rather than ignored, because ignoring it would read exactly
/// like a corpus that declares none.
fn index_address(
    vertex_prefix: &str,
    path: &str,
    doc: &Value,
    count: Option<u64>,
    container: Container,
) -> Result<Option<IndexAddress>> {
    let Some(declared) = mapping(doc, path, "index")? else {
        return Ok(None);
    };
    let need = |key: &str| -> Result<String> {
        match scalar(declared, key) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(invalid(format!(
                "{path} declares an index and no {key}, so its tiles address nothing"
            ))),
        }
    };
    let prefix = prefix_of(&join(&[vertex_prefix, &need("prefix")?]));
    let ordered_by = need("ordered_by")?;
    let raw = need("chunk_size")?;
    let chunk_size = match raw.parse::<u64>() {
        Ok(value) if value > 0 => value,
        _ => {
            return Err(invalid(format!(
                "{path} declares an index chunk_size of {raw}, which is not a row count"
            )));
        }
    };
    Ok(Some(IndexAddress {
        prefix,
        ordered_by,
        chunk_size,
        tiles: count.and_then(|c| tiles_of(c, chunk_size)),
        container,
    }))
}

fn edge_address(
    root: &str,
    path: &str,
    doc: &Value,
    types: &[VertexAddress],
    container: Container,
) -> Result<EdgeAddress> {
    let src_type = required(doc, path, "src_type")?;
    let dst_type = required(doc, path, "dst_type")?;
    let edge_type = required(doc, path, "edge_type")?;
    let prefix = prefix_of(&join(&[root, &required(doc, path, "prefix")?]));

    let endpoint = |name: &str, role: &str| -> Result<&VertexAddress> {
        types.iter().find(|t| t.vertex_type == name).ok_or_else(|| {
            invalid(format!(
                "{path} names {role} type {name}, which the index does not declare"
            ))
        })
    };
    let src = endpoint(&src_type, "source")?;
    let dst = endpoint(&dst_type, "destination")?;

    // An edge tile is addressed by a *vertex* tile, so a different number here
    // would address nothing — and it would address nothing silently, because the
    // URLs still compose and the files they name mostly exist. Checked once, here,
    // rather than trusted in a comment beside a reader.
    let src_chunk = required_number(doc, path, "src_chunk_size")?;
    let dst_chunk = required_number(doc, path, "dst_chunk_size")?;
    for (key, declared, vertex) in [
        ("src_chunk_size", src_chunk, src),
        ("dst_chunk_size", dst_chunk, dst),
    ] {
        if declared != vertex.chunk_size {
            return Err(invalid(format!(
                "{path} declares {key} {declared} against {}'s chunk_size {}, so its tiles \
                 address nothing",
                vertex.vertex_type, vertex.chunk_size
            )));
        }
    }
    let chunk_size = required_number(doc, path, "chunk_size")?;
    if chunk_size != src_chunk {
        return Err(invalid(format!(
            "{path} declares chunk_size {chunk_size} and src_chunk_size {src_chunk}"
        )));
    }

    let mut adjacencies: Vec<AdjacencyAddress> = Vec::new();
    for entry in sequence_of_mappings(doc, "adj_lists") {
        let Some(direction) = scalar(entry, "aligned_by")
            .as_deref()
            .and_then(Direction::parse)
        else {
            continue;
        };
        // The one part of a tile's URL a reader cannot compute. An orientation
        // declared without it has tiles nobody can address, so it is not an address
        // and does not become one here.
        let adj_prefix = scalar(entry, "prefix").unwrap_or_default();
        let adj_prefix = adj_prefix.trim_end_matches('/');
        if adj_prefix.is_empty() {
            continue;
        }
        let vertex = if direction == Direction::Src {
            src
        } else {
            dst
        };
        adjacencies.push(AdjacencyAddress {
            direction,
            prefix: prefix_of(&join(&[&prefix, adj_prefix])),
            column: direction.column(),
            chunk_size: vertex.chunk_size,
            shift: vertex.shift,
            tiles: vertex.tiles,
            container,
        });
    }
    // `src` before `dst`, whatever order the manifest wrote them in — the two
    // readers this is diffed against both report the orientations in that order.
    adjacencies.sort_by_key(|a| a.direction);

    Ok(EdgeAddress {
        edge_type,
        src_type,
        dst_type,
        count: optional_count(doc, "edge_count"),
        prefix,
        directions: adjacencies.iter().map(|a| a.direction).collect(),
        adjacencies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The border vectors `apps/corpus/guards/vectors.json` publishes for the
    /// shift, in the language that wrote the corpus. The JS ports need a `BigInt`
    /// to reproduce them; `u64` is the shape they were computed in.
    #[test]
    fn the_shift_holds_at_the_published_borders() {
        assert_eq!(shift_for(4096), Some(12));
        assert_eq!(shift_for(1), Some(0));
        assert_eq!(shift_for(0), None);
        assert_eq!(shift_for(4095), None);
        assert_eq!(tile_of(4095, 12), 0);
        assert_eq!(tile_of(4096, 12), 1);
        // 2^31 and 2^53, where a port that took the shift as signed gives a
        // negative tile and one that went through a double stops being exact.
        assert_eq!(tile_of(1 << 31, 12), 1 << 19);
        assert_eq!(tile_of(1 << 53, 12), 1 << 41);
    }

    #[test]
    fn a_count_that_divides_has_no_tail_tile() {
        assert_eq!(tiles_of(4096, 4096), Some(1));
        assert_eq!(tiles_of(4097, 4096), Some(2));
        assert_eq!(tail_rows(4096, 4096), Some(4096));
        assert_eq!(tail_rows(4097, 4096), Some(1));
        assert_eq!(tiles_of(0, 4096), Some(0));
        assert_eq!(tail_rows(0, 4096), Some(0));
        // The border a `Number` division loses: 2^53 + 1 is two tiles at 2^53.
        assert_eq!(tiles_of((1 << 53) + 1, 1 << 53), Some(2));
    }

    #[test]
    fn join_writes_forward_slashes_and_no_doubles() {
        assert_eq!(join(&["", "vertex/Person/"]), "vertex/Person");
        assert_eq!(
            join(&["/bench/1000000", "/vertex/"]),
            "/bench/1000000/vertex"
        );
        assert_eq!(prefix_of("vertex/Person"), "vertex/Person/");
        assert_eq!(prefix_of("vertex/Person//"), "vertex/Person/");
    }
}
