//! The plan for reading a corpus, resolved from its manifest — the Rust reader.
//!
//! A tile is a fixed range of `dense_id` and its address is a shift, so a reader
//! computes every URL it wants before it emits the first request. Two readers
//! already do that: `packages/corpus/src/address.ts` (the published module) and
//! `apps/corpus/conformance/reader.mjs` (written from the conventions and from
//! nothing else). This is the third, and it is the one that reaches a browser
//! through `fossil-graph-wasm`.
//!
//! **An address is the smallest thing here, not the subject.** The types named
//! `…Address` each turn an id into a URL and a tile number, and that is exactly
//! what they are. What the module does with them is larger: it validates that a
//! manifest can address itself at all, refuses the ones that cannot, and turns a
//! question — this vertex type, these tiles, these orientations — into the set of
//! files that answers it and an honest account of what that set is *not* complete
//! for. [`resolve`] hands back a [`ReadPlan`], and a plan for reading is what a
//! caller holds.
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

/// The filename stem of **every projection's** tiles under `files` — the payload,
/// a level of it, an adjacency, a level of that. One spelling, because they are
/// one kind of thing.
const PROJECTION_STEM: &str = "chunk";

/// The filename stem of the identity index's tiles, and the one place a corpus
/// spells a tile differently — because the index is the one artefact that is not
/// a projection. It is a second ORDER over the same rows, so tile `k` here is the
/// `k`th slice of the sorted order and not a `dense_id` range at all.
const INDEX_STEM: &str = "tile";

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

/// Why an orientation is missing from an answer. The three reasons are honest;
/// they are not the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    /// The caller did not ask for this direction.
    NotRequested,
    /// The manifest does not publish an address for it, so no URL exists to ask for.
    NotDeclared,
    /// The relation's other end is a different vertex type, so its far ends are
    /// numbered in another `dense_id` space. A [`Window`] over the incident set
    /// wants that relation and never reports this; a [`Drawing`] cannot place
    /// one of its ends and always does.
    OtherSpace,
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
        tile_url_for(&self.prefix, INDEX_STEM, self.container, tile)
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

/// **One projection of a corpus's sequence, resolved into an address.**
///
/// A corpus is an order, a cut, and some projections of that sequence, and every
/// artefact of one except [`IndexAddress`] is one of these: the payload at
/// `scale: 1`, a vertex level at `scale: 4^k`, an orientation of an adjacency at
/// `scale: 1` with a [`Self::direction`], an edge level at `scale: 4^k` with
/// one. **The payload is not a special case** — it is the projection whose scale
/// is one, and this type does not know which of its instances is which.
///
/// # It reads the scale and never the exponent
///
/// Tile `j` covers `[j · chunk_size · scale, (j+1) · chunk_size · scale)`, so
/// [`Self::shift`] is the type's own plus `log2(scale)` and [`Self::rows`] is
/// `count.div_ceil(scale)`. There is no `4` and no `2k` on this side of the
/// document: `fossil_sinks::manifest::VertexLevels` is where a WRITER turns a
/// level into a scale, and what crosses into the manifest is the product. That
/// is the whole reason `scale` is the declared field rather than the level.
///
/// # What is declared, and what is not
///
/// The **scale and the path**, and neither is derivable. Which projections a
/// writer spent bytes on is a policy — a reader re-deriving the set from
/// `vertex_count` and `chunk_size` would reimplement the writer's plan and 404
/// the day the plan moved — and the path is the one part of a tile's URL nothing
/// computes, there being no directory to list over HTTP.
///
/// A projection nobody wrote is still ANSWERABLE: a level is the predicate
/// `dense_id % scale == 0` over the payload, and a written `l{k}/` is a cache of
/// it. So a corpus declaring one projection draws the identical picture as one
/// declaring five, and only reads more.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectionAddress {
    /// Where its tiles are, resolved against the corpus base and with a trailing
    /// separator. The type's own prefix joined to the manifest's `path`.
    pub prefix: String,
    /// **How many rows of the underlying sequence one row here stands for** —
    /// `1` for a payload or an adjacency, `4^k` for level `k`.
    pub scale: u64,
    /// Which endpoint column addresses these tiles, on an edge projection.
    /// `None` on a vertex one, whose address is its own `dense_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// The column tile `k` filters on — `src_dense`/`dst_dense` on an edge
    /// projection, `dense_id` on a vertex one.
    pub column: &'static str,
    /// Rows per tile: the cut, which does not change with the scale. On an edge
    /// projection this is the ALIGNED endpoint type's — a different space from
    /// the other endpoint's on a cross-type edge.
    pub chunk_size: u64,
    /// `log2(chunk_size) + log2(scale)` — the shift that names a tile, and never
    /// a division.
    pub shift: u32,
    /// How many rows of the sequence this projection addresses: the type's own
    /// `vertex_count` divided by the scale, or the aligned endpoint type's on an
    /// edge. `None` when no count is declared.
    ///
    /// **Not the edge's row count**, on an edge projection. An edge tile is
    /// addressed by a *vertex* tile, so `edge_count / chunk_size` is the wrong
    /// division and it is wrong quietly.
    pub rows: Option<u64>,
    /// `ceil(rows / chunk_size)`, or `None` when no count is declared.
    pub tiles: Option<u64>,
    /// Which container carries these tiles. The corpus's, never a second answer.
    pub container: Container,
    /// The manifest file this was read from, for the error messages that name it.
    #[serde(skip)]
    path: String,
    /// The vertex type whose declared count says how far this projection goes —
    /// the type itself on a vertex projection, the ALIGNED endpoint on an edge
    /// one. Named in the refusal, because `edge_count` would not have helped and
    /// a reader told otherwise debugs the wrong manifest.
    #[serde(skip)]
    counted_by: String,
}

impl ProjectionAddress {
    /// The tile holding `dense_id` in this projection.
    #[must_use]
    pub const fn tile_of(&self, dense_id: u64) -> u64 {
        tile_of(dense_id, self.shift)
    }

    /// The file tile `j` is in, spelled by the corpus's container.
    #[must_use]
    pub fn tile_url(&self, tile: u64) -> String {
        tile_url_for(&self.prefix, PROJECTION_STEM, self.container, tile)
    }

    /// Every file of this projection, in order and distinct.
    ///
    /// Refuses when no count is declared, because then there is no set to
    /// enumerate: that is the difference between addressing a tile somebody
    /// asked for and knowing how many there are.
    pub fn files(&self) -> Result<Vec<String>> {
        let tiles = self.tiles.ok_or_else(|| {
            invalid(format!(
                "{}: {} is addressed by {}, which declares no vertex_count, so how many tiles it \
                 has is not derivable — tiles are addressed and never listed, and HTTP gives no \
                 directory to fall back on",
                self.path, self.prefix, self.counted_by
            ))
        })?;
        Ok(distinct((0..tiles).map(|k| self.tile_url(k))))
    }
}

/// The projections a manifest wrote, listed for an error message that has to
/// name what a reader CAN ask for.
fn written_scales(projections: &[ProjectionAddress]) -> String {
    let mut scales: Vec<String> = projections.iter().map(|p| p.scale.to_string()).collect();
    scales.dedup();
    scales.join(", ")
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
    /// **Every projection of this type**, in manifest order: the payload at
    /// `scale: 1` and one per written level. See [`ProjectionAddress`].
    ///
    /// A type declaring only its payload is a legal corpus and the most legal of
    /// them: a level is a predicate, so every scale is answerable with or
    /// without a file for it, and what a written one changes is which bytes
    /// answer it.
    pub projections: Vec<ProjectionAddress>,
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
        tile_url_for(&self.prefix, PROJECTION_STEM, self.container, tile)
    }

    /// One projection of this type by its scale, or `None` when the manifest
    /// wrote none at that scale — which is a cost and not a refusal.
    #[must_use]
    pub fn projection(&self, scale: u64) -> Option<&ProjectionAddress> {
        self.projections.iter().find(|p| p.scale == scale)
    }

    /// Every payload FILE of this type, in order and distinct.
    pub fn files(&self) -> Result<Vec<String>> {
        self.projection_files(1)
    }

    /// Every file of the projection at `scale`, in order and distinct.
    ///
    /// Refuses a scale nobody wrote by naming the ones that were, because a URL
    /// under an unwritten `l{k}/` is the one failure a reader cannot tell from
    /// an empty level — and the predicate over the payload answers it anyway.
    pub fn projection_files(&self, scale: u64) -> Result<Vec<String>> {
        let projection = self.projection(scale).ok_or_else(|| {
            invalid(format!(
                "{} writes {} at scales {} and not {scale}, so its files are not addressable — \
                 the predicate over the payload is what answers that scale",
                self.path,
                self.vertex_type,
                written_scales(&self.projections)
            ))
        })?;
        projection.files()
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
    /// not publish. A projection the manifest omits, or declares without a
    /// `path`, is not here.
    pub directions: Vec<Direction>,
    /// **Every projection of this relation**: one per orientation at `scale: 1`
    /// — the adjacency — and one per written level, source-aligned and carrying
    /// both endpoints' coordinates.
    ///
    /// The same list a vertex type has, told apart by
    /// [`ProjectionAddress::direction`]. A relation declaring only its
    /// adjacencies is a corpus and not a gap: a reader draws the same edges out
    /// of the adjacency and the payload, and only reads more.
    pub projections: Vec<ProjectionAddress>,
}

impl EdgeAddress {
    /// The declared orientation's adjacency — its projection at `scale: 1` — or
    /// `None` when the corpus does not publish that half.
    #[must_use]
    pub fn adjacency(&self, direction: Direction) -> Option<&ProjectionAddress> {
        self.projection(1, direction)
    }

    /// One projection of this relation by scale and orientation. A level is
    /// always source-aligned: a level of a relation is *which vertices are in
    /// it*, and the source type's own pyramid is what says which.
    #[must_use]
    pub fn projection(&self, scale: u64, direction: Direction) -> Option<&ProjectionAddress> {
        self.projections
            .iter()
            .find(|p| p.scale == scale && p.direction == Some(direction))
    }

    /// Every file of the projection at `scale` in `direction`, in order and
    /// distinct.
    ///
    /// Refuses a scale nobody wrote by naming the ones that were: the adjacency
    /// and the payload are what answer it, and a URL under an unwritten `l{k}/`
    /// is the one failure a reader cannot tell from an empty level.
    pub fn projection_files(&self, scale: u64, direction: Direction) -> Result<Vec<String>> {
        let projection = self.projection(scale, direction).ok_or_else(|| {
            invalid(format!(
                "{} writes {} at scales {} and not {scale} aligned by {}, so its files are not \
                 addressable — the adjacency and the payload are what answer that scale",
                self.prefix,
                self.edge_type,
                written_scales(
                    &self
                        .projections
                        .iter()
                        .filter(|p| p.direction == Some(direction))
                        .cloned()
                        .collect::<Vec<_>>()
                ),
                direction.as_str()
            ))
        })?;
        projection.files()
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
///
/// **Which relations are drawable at all is [`Drawing`] and is not here.** CSR
/// being enough for the drawable ones says nothing about which ones those are,
/// and a window over `Author` legitimately addresses `Author authored Paper` —
/// those edges are incident to the windowed authors. Their far ends are Papers.
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

/// **Which relations a picture of one vertex type may draw** — and which of the
/// ones incident to it it may not, with the reason.
///
/// A drawing is one type's `dense_id` space: every mark is a row of that type's
/// payload, every far end has to be placed in the same numbering, and the two
/// endpoint columns of an edge row are compared against it. A relation that
/// LEAVES the type has its far ends numbered in another type's space, and the
/// two spaces are both dense from zero — so the comparison matches, silently,
/// and draws a line between two vertices with nothing between them.
///
/// **It is not a filter over a [`Window`], and that is the point.** A window is
/// the incident set and a cross-type relation belongs in it: `by_source` of
/// `Author authored Paper` is tiled by `Author`'s own `dense_id`, it holds
/// exactly the out-edges of the windowed authors, and a reader walking a
/// neighbourhood wants it. What it cannot do is place the far end, so the same
/// address that answers incidence correctly answers drawing wrongly, and the
/// two questions are asked with two calls rather than with one argument.
#[derive(Debug, Clone, Serialize)]
pub struct Drawing {
    /// The vertex type every mark, every far end and both ends of every line
    /// are numbered in.
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// Indices into [`ReadPlan::edges`] of the relations a picture of this type
    /// may draw, in declaration order. An index rather than a label, because a
    /// corpus may declare two relations sharing one label and an answer named by
    /// label could not be attributed to either.
    pub relations: Vec<usize>,
    /// The relations incident to this type that the picture leaves out, and why.
    /// Source-aligned, because the drawing read is: a level of a relation is
    /// which vertices are in it, and the source type's pyramid is what says
    /// which.
    pub undrawn: Vec<Gap>,
}

/// A corpus resolved into what a reader needs to read it: the addresses, and the
/// methods that turn a question into the files that answer it. Every one of them
/// is pure and synchronous.
#[derive(Debug, Clone, Serialize)]
pub struct ReadPlan {
    /// Where the corpus lives; prepended to every URL and nothing else.
    pub base: String,
    /// Which container the corpus declares. One answer for every payload set in it.
    pub container: Container,
    /// Every vertex type, in the order the index names them.
    pub types: Vec<VertexAddress>,
    /// Every edge type, in the order the index names them.
    pub edges: Vec<EdgeAddress>,
}

impl ReadPlan {
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

    /// **Every file the corpus can address**, distinct and in declaration
    /// order: each vertex type's projections (the payload and every written
    /// level) and its identity index, then each relation's projections (both
    /// orientations of the adjacency, and its written levels).
    ///
    /// It is the list a host that must grant access file by file — signing
    /// URLs, registering them with an engine — hands over, so it never composes
    /// one itself. What cannot be enumerated is left out rather than guessed
    /// at: a projection counted by a type whose manifest declares no
    /// `vertex_count` has no tile count, and [`ProjectionAddress::files`] is
    /// the question that refuses it by name.
    #[must_use]
    pub fn files(&self) -> Vec<String> {
        let vertex = self.types.iter().flat_map(|t| {
            t.projections
                .iter()
                .filter_map(|p| p.files().ok())
                .chain(t.index.as_ref().and_then(|i| i.files(&t.path, &t.vertex_type).ok()))
                .flatten()
        });
        let edge = self
            .edges
            .iter()
            .flat_map(|e| e.projections.iter().filter_map(|p| p.files().ok()).flatten());
        // A set beside the list: this one is every tile of the corpus, and the
        // linear `distinct` the per-projection lists use would be quadratic here.
        let mut seen = std::collections::HashSet::new();
        vertex.chain(edge).filter(|url| seen.insert(url.clone())).collect()
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

    /// Which relations a picture of one vertex type may draw, and why it leaves
    /// each of the others out. See [`Drawing`].
    ///
    /// **The rule is structural and not a check**: a relation is drawable when
    /// BOTH of its endpoints are the drawn type, so nothing numbered in another
    /// type's space is ever addressed and no comparison between two `dense_id`
    /// spaces can be reached. `fossil-layout` states the same rule on the
    /// writing side — a cell is an interval of ONE type's ids, and the placement
    /// it computes is over the relations whose two ends are both in it — and
    /// this is the reading side of it.
    ///
    /// # Errors
    ///
    /// [`GraphError::InvalidManifest`] when the index declares no such vertex
    /// type, or none at all.
    pub fn drawing(&self, vertex_type: Option<&str>) -> Result<Drawing> {
        let vertex = self.vertex_type(vertex_type)?;
        let mut relations = Vec::new();
        let mut undrawn = Vec::new();
        for (index, edge) in self.edges.iter().enumerate() {
            let incident =
                edge.src_type == vertex.vertex_type || edge.dst_type == vertex.vertex_type;
            if !incident {
                continue;
            }
            let reason =
                if edge.src_type != vertex.vertex_type || edge.dst_type != vertex.vertex_type {
                    GapReason::OtherSpace
                } else if edge.adjacency(Direction::Src).is_none() {
                    GapReason::NotDeclared
                } else {
                    relations.push(index);
                    continue;
                };
            undrawn.push(Gap {
                edge_type: edge.edge_type.clone(),
                direction: Direction::Src,
                reason,
            });
        }
        Ok(Drawing {
            vertex_type: vertex.vertex_type.clone(),
            relations,
            undrawn,
        })
    }

    /// The URLs a set of vertex tiles addresses.
    ///
    /// `directions` of `[Src]` fetches the out-edges of every vertex in the
    /// window and reports `complete: false` with a `not-requested` gap, because
    /// a window of drawn vertices has in-edges it did not ask for. Both
    /// orientations is the incident set.
    ///
    /// **`[Src]` is not «the drawing read», and it read that way here for as
    /// long as the bug lasted.** The out-edges of an `Author` include the ones
    /// that land on a `Paper`, which is what makes this answer right for
    /// incidence and unusable for a picture: those rows carry a `dst_dense` in
    /// `Paper`'s numbering. Which relations a picture may draw is
    /// [`ReadPlan::drawing`].
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

/// Resolve a corpus's manifest set into the [`ReadPlan`] a reader composes URLs from.
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
) -> Result<ReadPlan> {
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

    Ok(ReadPlan {
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

/// A sequence of small mappings — `projections`, and nothing else so far.
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

/// Where the tiles of one set are, in whichever container the corpus declares.
/// One function for every set in a corpus, because the only thing that ever
/// differs between them is the stem of the filename — and after the four
/// vocabularies became one there are two stems left, [`PROJECTION_STEM`] for
/// every projection and [`INDEX_STEM`] for the artefact that is not one. Under
/// `rowgroups` even that goes: a set is one file.
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

    let mut address = VertexAddress {
        vertex_type,
        prefix,
        chunk_size,
        shift,
        count,
        tiles,
        container,
        index,
        projections: Vec::new(),
        path: path.to_string(),
    };
    // Every projection of this type, addressed against the type's own cut and
    // its own count. A vertex projection has no `aligned_by`: its address IS a
    // `dense_id`, which is the rank in the one order a corpus has.
    for declared in declared_projections(path, doc)? {
        if declared.direction.is_some() {
            continue;
        }
        let resolved = resolve_projection(
            &address.prefix.clone(),
            path,
            &declared,
            "dense_id",
            &address,
            container,
        );
        address.projections.push(resolved);
    }
    // A type with no projection at scale one has no payload, and a manifest that
    // names none is not one this can invent: the count, the cut and the index all
    // describe rows nothing addresses.
    if !address.projections.iter().any(|p| p.scale == 1) {
        return Err(invalid(format!(
            "{path} declares no projection at scale 1, so {} has no payload to address",
            address.vertex_type
        )));
    }
    Ok(address)
}

/// What one `projections:` entry declares, before it is resolved against a type.
struct DeclaredProjection {
    path: String,
    scale: u64,
    direction: Option<Direction>,
}

/// The `projections:` entries of a manifest, in order.
///
/// **One function for the vertex list and the edge list**, because there is one
/// list: a projection is a `path`, a `scale` and the columns, and what differs
/// between the two manifests is only which type's cut addresses the result. That
/// stays with each caller and the reading does not get a second copy.
///
/// An entry that names no `path` is **dropped and not refused**, which is the
/// behaviour the corpus that publishes one orientation without a location needs:
/// there is no URL for it, so there is no address, and a reader reports the
/// orientation as one the manifest does not publish rather than one that 404s.
/// A `scale` no shift addresses IS refused, because a projection whose scale is
/// not a power of two forces a division where the whole format is a shift.
fn declared_projections(path: &str, doc: &Value) -> Result<Vec<DeclaredProjection>> {
    let mut out = Vec::new();
    for entry in sequence_of_mappings(doc, "projections") {
        // Present-but-empty is the payload, at the type's own prefix. Absent is
        // an entry with nowhere to be, and the two must not collapse.
        let Some(location) = scalar(entry, "path") else {
            continue;
        };
        let direction = match scalar(entry, "aligned_by") {
            None => None,
            Some(spelling) => match Direction::parse(&spelling) {
                Some(direction) => Some(direction),
                // Not an orientation, so nothing addresses it. Dropped for the
                // reason a missing `path` is: it names no tiles a reader can ask
                // for, and inventing one would compose a URL.
                None => continue,
            },
        };
        let raw = scalar(entry, "scale").unwrap_or_default();
        let scale = raw.parse::<u64>().unwrap_or(0);
        if shift_for(scale).is_none() {
            return Err(invalid(format!(
                "{path} declares a projection at scale {raw}, which no shift addresses"
            )));
        }
        out.push(DeclaredProjection {
            path: location,
            scale,
            direction,
        });
    }
    Ok(out)
}

/// One declared projection, resolved against the cut that addresses it.
///
/// The whole of the arithmetic, and there is no exponent in it: the shift is the
/// type's own plus `log2(scale)` and the rows are the count divided by `scale`.
/// `fossil_sinks::manifest::VertexLevels` is where a writer turns a level into a
/// scale; what reaches this side is the product.
fn resolve_projection(
    type_prefix: &str,
    path: &str,
    declared: &DeclaredProjection,
    column: &'static str,
    addressed_by: &VertexAddress,
    container: Container,
) -> ProjectionAddress {
    let (chunk_size, shift, count) = (
        addressed_by.chunk_size,
        addressed_by.shift,
        addressed_by.count,
    );
    let rows = count.map(|c| c.div_ceil(declared.scale));
    ProjectionAddress {
        prefix: prefix_of(&join(&[type_prefix, &declared.path])),
        scale: declared.scale,
        direction: declared.direction,
        column,
        chunk_size,
        // `saturating_add` because a scale wider than a `dense_id` is a whole
        // projection in one tile, which is the answer and not an overflow.
        shift: shift.saturating_add(declared.scale.trailing_zeros()),
        rows,
        tiles: rows.and_then(|r| tiles_of(r, chunk_size)),
        container,
        path: path.to_string(),
        counted_by: addressed_by.vertex_type.clone(),
    }
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

    // Every projection of this relation, adjacency and level alike: the aligned
    // endpoint's cut and count are what address it, so an edge level is counted
    // against the SOURCE type's `vertex_count` and never against `edge_count`.
    // A level tile here is a range of `src_dense`, and the relation's own row
    // count says nothing about how many of those ranges there are.
    let mut projections: Vec<ProjectionAddress> = Vec::new();
    for declared in declared_projections(path, doc)? {
        // An edge projection with no orientation names no column to filter on,
        // so nothing addresses it and it is not an address.
        let Some(direction) = declared.direction else {
            continue;
        };
        let vertex = if direction == Direction::Src {
            src
        } else {
            dst
        };
        projections.push(resolve_projection(
            &prefix,
            path,
            &declared,
            direction.column(),
            vertex,
            container,
        ));
    }
    // Finest first and `src` before `dst`, whatever order the manifest wrote
    // them in — the two readers this is diffed against both report the
    // orientations in that order.
    projections.sort_by_key(|p| (p.scale, p.direction));

    Ok(EdgeAddress {
        edge_type,
        src_type,
        dst_type,
        count: optional_count(doc, "edge_count"),
        prefix,
        directions: projections
            .iter()
            .filter(|p| p.scale == 1)
            .filter_map(|p| p.direction)
            .collect(),
        projections,
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
