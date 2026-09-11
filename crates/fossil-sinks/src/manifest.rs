//! The corpus manifest structs + `serde_yaml_ng` emission.
//!
//! # A corpus is a sequence, a cut, and some projections of that sequence
//!
//! **An order** — Morton over the positions, and `dense_id` *is* the rank in it.
//! **A cut** — fixed runs of `chunk_size`, and that, and only that, is a tile:
//! arithmetic rather than an artefact. And **projections of that sequence**,
//! each of them a [`Projection`]: a `scale` and the columns it carries. This
//! file said the same rule four times — `property_groups`, a vertex's `levels`,
//! `adj_lists`, and an edge's own `levels` — and says it once. **The payload is
//! the projection at `scale: 1`, not a special case, and that is the claim.**
//!
//! [`VertexIndex`] is the one artefact that is NOT a projection: it is a second
//! ORDER over the same rows, so the Morton cut does not address it.
//!
//! `path` and `scale` are `OME-NGFF`'s own spellings — a `multiscales` object
//! there lists `datasets`, each with a `path` and a `coordinateTransformations`
//! entry of `{"type": "scale", "scale": [...]}`, ordered finest first as these
//! are. Borrowed for recognition and not for novelty.
//!
//! **The rest of the field vocabulary is `GraphAr` v1.0.0's. The conformance is
//! not**, and `projections` is a divergence larger than the eight: a `GraphAr`
//! reader looks for `property_groups` and `adj_lists` and finds neither. A
//! fossil corpus was already not a valid `GraphAr` corpus — `fossil-df` declares
//! `dense_id` as `uint32` and the reference C++ reader throws on it at the first
//! property of the first vertex type. `/docs/design/corpus` has the divergences,
//! what each was read out of, and what would reverse them.
//!
//! The structs are plain serializable data — no `Box<dyn Trait>`, safe to pass
//! through Salsa queries (CLAUDE.md hard rule). `data_type` spellings come from
//! [`arrow_schema::DataType`] via [`data_type_name`].
//!
//! **This module declares the tiling and emits no bytes.** `fossil-df`'s
//! `files.rs` is the single Arrow→Parquet encoder and `fossil-layout`'s
//! `enrich_layout` re-tiles into the declared `prefix`; that what the emitter
//! writes is what the manifest says is asserted on the artefact by
//! `fossil-cli/tests/conformance.rs`, not agreed by convention.

use arrow_schema::DataType;
pub use fossil_graph_schema::Cardinality;
use serde::{Deserialize, Serialize};

/// The `GraphAr` manifest format version string. Emitted as `version: gar/v1`.
pub const GRAPHAR_VERSION: &str = "gar/v1";

/// The payload file of a row-group container: one per set, its row groups the
/// tiles. `@fossil-lang/corpus` spells the same constant.
pub const TILES_FILE: &str = "tiles.parquet";

/// The filename stem of a **level set**, under a vertex type's own
/// [`VertexInfo::prefix`]: level `k` lives under `<prefix>l{k}/`, and inside it
/// the container rules apply unchanged. See [`VertexLevels`].
pub const LEVEL_PREFIX_STEM: &str = "l";

/// Which container carries a corpus's tiles — one file per tile with the address
/// in the name, or one file per set with the address as the row-group ordinal.
///
/// **A reader cannot work this out**: working it out means listing a directory,
/// and there is no listing over HTTP. So it is declared once, on
/// `graph.graph.yml`, and applies to every payload set in the corpus at once.
/// `/docs/format/conventions/addressing` has the measurement that chose
/// fossil's.
///
/// Absence is [`Self::Files`] and not an error, because a corpus written before
/// the field existed is that one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// One Parquet per tile, the address in the filename —
    /// `<prefix>chunk{k}.parquet`, `<prefix>tile{k}.parquet`.
    #[default]
    Files,
    /// One Parquet per payload set, the address the row-group ordinal, named by
    /// [`TILES_FILE`] under the set's own prefix. What fossil writes.
    RowGroups,
}

/// The privacy bound a corpus declares, and the population it was measured over.
///
/// **A declaration, not an enforcement.** A corpus is files and there is no
/// chokepoint, so the protection is total at write time: the bytes that would
/// violate the bound are never written. What travels with the artifact is this
/// declaration plus a property a stranger can re-derive from the files —
/// `apps/corpus/guards/guards.mjs`'s `declared-privacy` is that stranger.
/// `/docs/format/conventions/privacy` has the argument, and what a *served*
/// corpus can guarantee on top of it.
///
/// Not `Option`: [`Self::Undeclared`] is a value a producer writes and means
/// "this corpus carries no bound", while a missing `privacy:` key means
/// "written before this field existed". Neither may be read as "public". This
/// is [`VertexInfo::vertex_count`]'s argument and not [`VertexInfo::index`]'s —
/// the quasi-identifier set is a judgement about a jurisdiction, so no amount
/// of scanning recovers it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "bound", rename_all = "kebab-case")]
pub enum Privacy {
    /// No bound. The corpus may hold anything, and a reader is told so rather
    /// than left to infer it from a missing key.
    #[default]
    Undeclared,
    /// Every equivalence class over the declared quasi-identifiers holds at
    /// least [`KAnonymity::k`] records of the released population.
    KAnonymity(KAnonymity),
}

/// What a verified k-anonymity bound records, and what a third party needs to
/// re-derive it from the files alone.
///
/// Every field here is either a parameter of the check or an outcome of it.
/// Nothing is a summary: a reader that recomputes from the Parquet must reach
/// these numbers exactly, and `declared-privacy` fails when it does not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KAnonymity {
    /// The bound the policy asked for. The claim is `reached >= k`.
    pub k: u64,
    /// The smallest equivalence class actually measured, over the whole
    /// released population. Published beside [`Self::k`] because the two answer
    /// different questions — what was required, and what there is.
    pub reached: u64,
    /// How a `NULL` in a quasi-identifier column is read. **A parameter and not
    /// a default**, because both extremes are wrong and the field that settles
    /// it says so by exposing the choice as a number rather than making it —
    /// see [`AbsentQuasiIdentifier`].
    pub absent_quasi_identifier: AbsentQuasiIdentifier,
    /// The records the bound was measured over: every row of every tile of
    /// every type carrying a quasi-identifier, summed — so equal to the sum of
    /// those types' [`VertexInfo::vertex_count`]. That equality is the **scope
    /// assertion**: a corpus is tiles, so a check that silently ran per-tile is
    /// the easiest wrong answer available, and publishing the population is what
    /// makes it detectable by somebody else.
    pub population: u64,
    /// Records excluded from certification and charged to the budget. Non-zero
    /// only under [`AbsentQuasiIdentifier::Suppress`]; zero by construction
    /// under the other two.
    pub suppressed: u64,
    /// The suppression allowance, in **parts per million of
    /// [`Self::population`]**. The bound requires
    /// `suppressed * 1_000_000 <= population * suppression_budget_ppm`.
    ///
    /// An integer because a manifest is a text document four independent
    /// readers parse (`serde_yaml_ng`, two line scanners in JavaScript and
    /// TypeScript, and whatever a stranger brings), and a float is the one
    /// scalar where they can disagree about the same bytes. `20000` is 2%, and
    /// `BigInt` reproduces the comparison without rounding. Why a budget is
    /// part of the bound at all, and where ARX's own sweeps land, is on
    /// `/docs/format/conventions/privacy` and `/docs/design/prior-art`.
    pub suppression_budget_ppm: u64,
    /// The quasi-identifier set, as `<Type>.<column>` names separated by single
    /// spaces — `Person.birth_year Person.postcode Person.sex`.
    ///
    /// **The one field a reader cannot derive and the one the whole bound turns
    /// on.** A flat scalar and not a sequence: `apps/corpus/guards/manifest.mjs`
    /// and `packages/corpus/src/manifest.ts` are line scanners over a flat
    /// mapping, and they **skip** what they cannot see rather than failing on
    /// it — so a set emitted as a YAML sequence scans as absent.
    pub quasi_identifiers: String,
    /// **What the writer did to reach [`Self::k`]**, per generalised column.
    ///
    /// `none` when the policy declared no hierarchy and the quasi-identifiers
    /// were published as the program produced them. Otherwise a space-separated
    /// token per generalised column, in the same `<Type>.<column>` spelling and
    /// the same flat-scalar grammar as [`Self::quasi_identifiers`]:
    ///
    /// ```text
    /// generalization: Person.birthYear@bucket Person.postcode@1-3/4
    /// ```
    ///
    /// `@bucket` is a numeric column published as its enclosing **declared**
    /// bucket. `@<coarsest>-<finest>/<declared>` is a levelled column — prefix
    /// or date — with the coarsest and finest hierarchy levels any published
    /// class sits at, over the levels the hierarchy declares.
    ///
    /// It earns its place because **`reached` means two different things
    /// without it**: a `reached: 20` over raw postcodes and one over postcodes
    /// truncated to three characters are not the same release, and a recipient
    /// recomputing `k` gets 20 either way. It is checkable only by a recipient
    /// who also has the hierarchy, which lives in the policy document
    /// [`Self::policy`] names — `/docs/format/conventions/privacy` states that
    /// caveat rather than papering over it.
    #[serde(default)]
    pub generalization: String,
    /// The `odrl:uid` of the policy document this corpus was verified against.
    /// A name, not a location: the document is not in the corpus, and a corpus
    /// that carried its own policy would be a corpus that can be handed on with
    /// the policy rewritten.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy: String,
    /// The `odrl:profile` IRI the policy declared, so a reader knows which
    /// vocabulary the `leftOperand`s came from before it tries to read them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub profile: String,
}

/// The declared reading of an absent quasi-identifier, re-exported from the
/// crate that owns it.
///
/// It lives in `fossil-policy` and not here because it is a **policy
/// parameter**: the producer chooses it in the document, and the manifest
/// records the choice. The dependency runs manifest → policy vocabulary →
/// nothing, which is the direction that makes sense — a record of a check names
/// the vocabulary of the check, and a policy document knows nothing about
/// `GraphAr`. Re-exported rather than mirrored so that there is exactly one
/// spelling of `value` / `wildcard` / `suppress` in the tree; two enums with a
/// conversion between them is how the two halves of a manifest start
/// disagreeing.
pub use fossil_policy::AbsentQuasiIdentifier;

/// `GraphAr` vertex-info manifest (one per vertex/shape type).
///
/// Serializes with the spec field names; `vertex_type` renames to `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexInfo {
    /// Shape label, e.g. `"Person"`. Emitted as the spec key `type`.
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// Full RDF type IRI (empty for non-RDF graphs). Carried into the manifest
    /// so the query side's schema verbs surface it without a separate registry.
    /// Omitted from YAML when empty, so non-RDF graphs keep the canonical
    /// `GraphAr` shape.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub iri: String,
    /// **How many rows this vertex type has** — every row of every tile under
    /// [`Self::prefix`], summed. Equivalently the count of distinct `dense_id`s,
    /// and, because they are a gapless `0..n−1`, one more than the largest of
    /// them. It is not a tile count and it is not the largest id.
    ///
    /// The field `GraphAr` does not have. Tiles are addressed and never listed,
    /// so a hole in the middle is caught by the addressing — tile `k` is not
    /// where tile `k+1` says it is — and **a missing tail is caught by
    /// nothing**. With this, `tiles = vertex_count.div_ceil(chunk_size)`, and a
    /// reader knows how far the corpus goes before it opens a file.
    ///
    /// `u64` and not `Option<u64>`: a count a reader may skip is a count the
    /// reader that cannot detect the truncation skips, which reproduces the gap
    /// it was added to close.
    pub vertex_count: u64,
    /// Rows per tile (configurable; default [`DEFAULT_CHUNK_SIZE`]). Tile `k` is
    /// the `dense_id` range `[k·chunk_size, (k+1)·chunk_size)` and a power of
    /// two, so a reader addresses it with [`tile_of`] rather than a division.
    pub chunk_size: u64,
    /// Output path prefix for this vertex's tiles, e.g. `"vertex/person/"`.
    pub prefix: String,
    /// **Every projection of this type's sequence**, coarsest last: the payload
    /// at `scale: 1` and one entry per written level. See [`Projection`].
    ///
    /// Not `Option` and not skipped when empty: a type with no projection has no
    /// bytes, and the difference between that and a type whose payload the
    /// manifest forgot to name is the difference between a corpus and a 404.
    pub projections: Vec<Projection>,
    /// **A second copy of this type, ordered by identity instead of by
    /// position** — the index that turns a lookup by subject IRI from a scan
    /// into a seek. See [`VertexIndex`].
    ///
    /// `Option` where [`Self::vertex_count`] is not, because a missing count
    /// leaves a question **unanswerable** and a missing index leaves it
    /// answerable and **slower**: `subject = ?` over every tile returns exactly
    /// the row the index would have found, and `openCorpus` reports the cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<VertexIndex>,
    /// **Which coordinate systems this type's rows carry, and where each one
    /// came from.** See [`CoordinateSystem`].
    ///
    /// A far view of a graph is worth looking at or worth nothing depending on
    /// this and nothing else: if a position is a latitude or a learned
    /// embedding then the plane is a space and its density is information; if a
    /// drawing algorithm chose it because a picture needed coordinates, the
    /// density is about the algorithm. Both are two `float32` columns, so
    /// nothing but a declaration can tell them apart.
    ///
    /// **A list, not a value**, and not for symmetry: a corpus can legitimately
    /// carry a measured position *and* a derived one, and which of them gets
    /// drawn is a reader's question rather than a writer's. It is the shape
    /// OME-NGFF and `SpatialData` reached for the same reason.
    ///
    /// **`None` is a statement about the writer, not about the data** — the
    /// same rule [`Property::cardinality`] is written under, and for the same
    /// reason. A corpus written before this field existed has no declaration to
    /// record, and it reads back as "not declared" rather than as "derived",
    /// which is the answer a default would have invented. `/docs/design/position`
    /// is the argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<Vec<CoordinateSystem>>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// One coordinate system over a vertex type's rows: which two columns hold it,
/// and **where the numbers in them came from**.
///
/// The cost of carrying a second system is a permutation of 4 bytes per row per
/// axis — a column, not a corpus — which is what makes declaring both of them
/// cheaper than making a reader guess between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinateSystem {
    /// What this system is called, so a reader can ask for one by name — `geo`,
    /// `layout`. Names are the writer's; nothing here reserves any.
    pub name: String,
    /// The column holding the first axis.
    pub x: String,
    /// The column holding the second axis.
    pub y: String,
    /// Where the positions came from. See [`Provenance`].
    pub provenance: Provenance,
    /// What derived them, for a [`Provenance::Derived`] system — a free string,
    /// because the set of things that can lay out a graph is not closed.
    ///
    /// `None` on a derived system says the writer did not record it, and that
    /// is weaker than a name rather than equivalent to one: re-deriving a
    /// position is how a reader checks that a holon's referent has not moved,
    /// and it cannot re-run what nothing names. It is `None` rather than
    /// required because a measured system has nothing to put here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_by: Option<String>,
}

/// Where a coordinate system's numbers came from — the one fact that decides
/// whether a far view of this type means anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// A coordinate reference system outside the corpus — a latitude and
    /// longitude, a projected CRS. The plane is the world and its density is
    /// information about the world.
    Geographic,
    /// A learned or fitted space — an embedding, an MDS solution. The plane is
    /// a space and its density is information about that space.
    Embedded,
    /// A drawing algorithm chose it because a picture needed coordinates.
    /// **The density is about the algorithm**, and a heat map of it is a
    /// picture of the layout's own regularities.
    Derived,
}

impl CoordinateSystem {
    /// A system whose positions a drawing algorithm chose, naming what chose
    /// them.
    #[must_use]
    pub fn derived(
        name: impl Into<String>,
        x: impl Into<String>,
        y: impl Into<String>,
        derived_by: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            x: x.into(),
            y: y.into(),
            provenance: Provenance::Derived,
            derived_by: Some(derived_by.into()),
        }
    }

    /// A system whose positions are data — measured or fitted outside the
    /// renderer. `derived_by` is `None` because there is nothing to re-run.
    #[must_use]
    pub fn measured(
        name: impl Into<String>,
        x: impl Into<String>,
        y: impl Into<String>,
        provenance: Provenance,
    ) -> Self {
        Self {
            name: name.into(),
            x: x.into(),
            y: y.into(),
            provenance,
            derived_by: None,
        }
    }

    /// Whether this system's density is information about something other than
    /// the algorithm that drew it — which is the question a far view turns on.
    #[must_use]
    pub const fn is_data(&self) -> bool {
        matches!(
            self.provenance,
            Provenance::Geographic | Provenance::Embedded
        )
    }
}

/// Where a vertex type's identity index lives, and what it is ordered by.
///
/// **This is the one artefact of a corpus that is not a [`Projection`]: it is a
/// second ORDER over the same rows, so the Morton cut does not address it.**
/// Everything else — payload, levels, adjacency, edge levels — is the same
/// sequence read at some `scale`, and tile `k` of it is a `dense_id` range.
/// Tile `k` here is the `k`th slice of the SORTED order, which is why it
/// carries a [`Self::chunk_size`] of its own and no `scale`.
///
/// The payload is in Morton order, because the spatial order IS the id space
/// and that is what makes a window a range. A lookup by identity needs the
/// other order and a table has one sort, so this is a **second table**, tiled
/// like the first: the index tiles' footers carry disjoint `subject` ranges, so
/// a binary search over them names the one tile a value can be in — which the
/// payload's own footers cannot do, every range there spanning the type.
/// `/docs/format/conventions/identity` has the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexIndex {
    /// Path prefix for the index tiles, e.g. `"index/"`, relative to the vertex
    /// type's own [`VertexInfo::prefix`]. Tile `k` is `<prefix>tile{k}.parquet`.
    pub prefix: String,
    /// The column the tiles are sorted by, and the one a lookup is keyed on.
    /// `"subject"` today, and named rather than assumed because a corpus whose
    /// identity is some other column would index that column instead.
    pub ordered_by: String,
    /// Rows per index tile. The same as [`VertexInfo::chunk_size`] when the
    /// writer has no reason to differ, and declared separately because tile `k`
    /// of the index holds the `k`th slice **of the sorted order**, which has
    /// nothing to do with the `dense_id` range tile `k` of the payload holds.
    /// Reusing the payload's number would read as an alignment that does not
    /// exist.
    pub chunk_size: u64,
}

/// **The pyramid a type gets** — the plan, and the one home of the exponent.
///
/// This is not a manifest block and does not serialise: what a manifest carries
/// is a [`Projection`] per level, and this is what CHOOSES them. The writer
/// calls [`Self::planned`] once and turns the answer into projections with
/// [`Self::projections`], so the manifest cannot name a level nobody wrote.
///
/// **Level `k` is the vertices whose `dense_id` is a multiple of `4^k`.** Over
/// a Morton-ordered `dense_id` that is one vertex per quadtree cell of depth
/// `k`, and level `k+1` is a strict subset of level `k` — the nesting is by
/// construction rather than by a writer's care, which is why zooming in only
/// ever ADDS. A level is not an aggregation: nothing here is a synthetic
/// centroid, every row is a real vertex at the position the payload gives it,
/// and `cluster_id` is a colour rather than a level of anything.
///
/// **Quarters and not halves, and the pyramid is COMPLETE.** A camera's zoom
/// step doubles the linear scale, which quadruples the area and so the points,
/// so a level per quarter is a level per zoom step. In halves one step crossed
/// two levels, and a writer needed a window constant to bound what it wrote —
/// the exponent was wrong and the constant was the patch. In quarters the cost
/// is the series `1/4 + 1/16 + …`, a third of the type whatever `V` is, so
/// there is nothing left for a window or a floor to bound: every level from 1
/// down to the one that fits a single tile is written.
///
/// **The exponent never leaves this type**: what a manifest carries is `4^k` as
/// a [`Projection::scale`], and [`Projection`] is where the addressing that
/// follows from it is written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexLevels {
    /// Filename stem of a level's path, relative to the vertex type's own
    /// [`VertexInfo::prefix`] — [`LEVEL_PREFIX_STEM`] as the writer spells it.
    /// Level `k`'s [`Projection::path`] is `<stem>{k}/`, and inside it the
    /// corpus's [`Container`] decides the filenames exactly as it does for the
    /// payload, because a level is not a different kind of thing from one.
    pub prefix: String,
    /// The levels planned, finest first, and **complete** — `1..=coarsest`,
    /// where the coarsest is the level that fits a single tile. Level `k` holds
    /// `ceil(vertex_count / 4^k)` rows.
    pub levels: Vec<u32>,
    /// Rows per tile — the type's own [`VertexInfo::chunk_size`]. The cut does
    /// not change with the scale: a projection is a run of `chunk_size` rows of
    /// its own sequence, whatever that sequence samples.
    pub chunk_size: u64,
}

/// **How many bits of `dense_id` one level drops — and the only place the
/// pyramid's base is written down.**
///
/// A level is a quarter of the one below it, so `stride(k) = 4^k` and a level
/// tile's address is the payload's shift plus `2k`. That `2` is the whole of
/// the pyramid's arithmetic, and it lives here because it was spelled fourteen
/// times across four implementations when it did not: every writer, reader and
/// guard reaches it through [`VertexLevels::stride`] or
/// [`VertexLevels::stride_bits`], and a second spelling of it is the bug this
/// constant exists to prevent.
///
/// It replaces two constants rather than joining them: one bounded how many
/// levels were written and the other which corpora got any, and both capped a
/// cost that halving left unbounded. In quarters the complete pyramid costs
/// `1/4 + 1/16 + … = 1/3` of the type whatever `V` is — the same third an
/// OME-Zarr pyramid pays, and for the same reason — so there is nothing to cap.
const STRIDE_BITS: u32 = 2;

impl VertexLevels {
    /// How many `dense_id`s one row of level `k` stands for — `4^k`.
    ///
    /// Saturating rather than panicking at the top of the range: a level whose
    /// stride does not fit a `u64` holds one row, which is the arithmetically
    /// correct answer and the one that keeps [`Self::planned`]'s search total.
    #[must_use]
    pub const fn stride(level: u32) -> u64 {
        match 1u64.checked_shl(Self::stride_bits(level)) {
            Some(step) => step,
            None => u64::MAX,
        }
    }

    /// How many bits of `dense_id` level `k` drops — `2k`, and the number a
    /// reader ADDS to its payload tile shift to address a level tile.
    #[must_use]
    pub const fn stride_bits(level: u32) -> u32 {
        level.saturating_mul(STRIDE_BITS)
    }

    /// The pyramid a type of `vertex_count` rows at `chunk_size` rows per tile
    /// gets, or `None` where it gets none.
    ///
    /// **One function, two callers**: the layout pass writes exactly these
    /// levels and the manifest declares exactly these levels, so the two cannot
    /// drift into a manifest naming a file nobody wrote. A second copy of this
    /// rule anywhere is the bug it exists to prevent.
    ///
    /// The list is **complete**: every level from 1 to the coarsest, which is
    /// the finest `k` whose level fits in **one tile** — coarser than that buys
    /// nothing, because one tile is already one range request and the whole
    /// level is the minimum read. There is no floor and no window. The one size
    /// that gets no pyramid is the one a single range request already answers.
    ///
    /// # Panics
    /// Never: `chunk_size` of zero returns `None` before it is divided by, and
    /// the search terminates because [`Self::stride`] saturates.
    #[must_use]
    pub fn planned(vertex_count: u64, chunk_size: u64) -> Option<Self> {
        if chunk_size == 0 || vertex_count <= chunk_size {
            return None;
        }
        // The coarsest: the smallest `k` with `ceil(V / 4^k) <= chunk_size`.
        // Searched rather than derived from a logarithm, because the answer has
        // to be the same integer in every language that reads this corpus.
        let mut coarsest = 1u32;
        while Self::rows_at(vertex_count, coarsest) > chunk_size {
            coarsest += 1;
        }
        Some(Self {
            prefix: LEVEL_PREFIX_STEM.to_string(),
            levels: (1..=coarsest).collect(),
            chunk_size,
        })
    }

    /// The prefix level `k`'s tiles live under, relative to the vertex type's
    /// own [`VertexInfo::prefix`] — `<stem>{k}/`.
    #[must_use]
    pub fn level_prefix(&self, level: u32) -> String {
        format!("{}{level}/", self.prefix)
    }

    /// How many rows level `k` of a type of `vertex_count` rows holds.
    #[must_use]
    pub const fn rows_at(vertex_count: u64, level: u32) -> u64 {
        vertex_count.div_ceil(Self::stride(level))
    }

    /// **This plan, as the projections a manifest carries** — one per level,
    /// finest first, each at `path: <stem>{k}/` and `scale: 4^k`.
    ///
    /// The only bridge between the exponent and the document: a caller writes
    /// no `4` and no shift, it writes `properties` and gets the scales back.
    #[must_use]
    pub fn projections(&self, file_type: &str, properties: &[Property]) -> Vec<Projection> {
        self.levels
            .iter()
            .map(|&level| Projection {
                path: self.level_prefix(level),
                scale: Self::stride(level),
                aligned_by: None,
                ordered: None,
                file_type: file_type.to_string(),
                properties: properties.to_vec(),
            })
            .collect()
    }
}

/// `GraphAr` edge-info manifest (one per `(src_type, edge_type, dst_type)` triple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeInfo {
    /// Source vertex type label.
    pub src_type: String,
    /// Edge type label (the relationship name).
    pub edge_type: String,
    /// Full predicate IRI (empty for non-RDF graphs). Omitted from YAML when
    /// empty. See [`VertexInfo::iri`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub iri: String,
    /// Destination vertex type label.
    pub dst_type: String,
    /// **How many of these edges one source vertex may have**, or `None` where
    /// nothing declared it.
    ///
    /// [`Cardinality::Single`] is the interesting one, and it is a fact about
    /// the relation that no count can stand in for: an edge type whose every
    /// source happens to have one destination *today* is not a functional
    /// relation, and a reader may not treat it as one. A shape saying `{1,1}`
    /// is what makes it safe to.
    ///
    /// It is written here because the compiler knew it and the corpus did not.
    /// `fossil_graph_schema::EdgeType` has carried the field since the shape
    /// reader was written, `Occurs::collapse` is the only way into it, and
    /// `edge_info` took the whole `EdgeType` and never read it — so a `{1,1}`
    /// edge and a `*` edge produced byte-identical manifests, and every
    /// consumer downstream had to re-derive from the data what the shape had
    /// already said.
    ///
    /// **`directed` is not this.** That says whether the relation has an
    /// orientation; this says how many times it may fire from one end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<Cardinality>,
    /// **How many edges this relation has** — the rows of one orientation, not
    /// of both. The two orientations are one relation stored twice, so a single
    /// number covers them and each of them separately has to add up to it.
    ///
    /// The sibling of [`VertexInfo::vertex_count`], required for the same
    /// reason: an edge tile with no rows is not written, so a 404 reads as
    /// "this tile has no edges" and as "this tile was never uploaded" alike.
    /// Summing the tiles a reader did find against this number tells them
    /// apart. Not a tile count — [`Self::chunk_size`] is not a row count and
    /// this is not a size.
    pub edge_count: u64,
    /// The addressing unit of an edge tile, equal to [`Self::src_chunk_size`].
    ///
    /// **Not a row count**, and it never was one for edges: an edge lives in the
    /// tile of the endpoint its file is ordered by, so tile `k` under
    /// `<prefix>by_source/` holds every edge whose `src_dense` is in vertex tile
    /// `k`, and its row count is the total degree of that tile's vertices. The
    /// alternative — the deepest tile containing both endpoints — is dominated
    /// on both curves; `/docs/format/conventions/adjacency` has the measurement
    /// and `/docs/design/discarded` what would bring it back.
    pub chunk_size: u64,
    /// Source-vertex tile size, and the shift that addresses the `aligned_by:
    /// src` tiles. Must equal the source [`VertexInfo::chunk_size`] — an edge
    /// tile is addressed by a vertex tile, so a different number here would
    /// address nothing.
    pub src_chunk_size: u64,
    /// Destination-vertex tile size, and the shift that addresses the
    /// `aligned_by: dst` tiles. Must equal the destination
    /// [`VertexInfo::chunk_size`] for the same reason [`Self::src_chunk_size`]
    /// must equal the source's — the two are separate fields because on a
    /// cross-type edge they are separate `dense_id` spaces, and only a
    /// same-type edge makes them look like one number.
    pub dst_chunk_size: u64,
    /// Whether the edge is directed.
    pub directed: bool,
    /// Output path prefix, e.g. `"edge/person_knows_person/"`.
    pub prefix: String,
    /// **Every projection of this relation's sequence**: one per orientation at
    /// `scale: 1` — the adjacency — and one per written level, source-aligned.
    ///
    /// The rule [`VertexInfo::projections`] states, applied to edges. What an
    /// edge projection has that a vertex one does not is
    /// [`Projection::aligned_by`], which names the endpoint column that
    /// addresses it. See [`Projection`], and its `# An edge level` section for
    /// why a level here carries four more columns than the adjacency does.
    pub projections: Vec<Projection>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// `GraphAr` top-level **graph info** (`<name>.graph.yml`) — the aggregate
/// index that references every vertex-info and edge-info file in the graph.
///
/// Required for serverless consumption: an httpfs reader (fossil-graph,
/// DuckDB-WASM) cannot list a directory over HTTP, so the graph info is the
/// single entry point a binding fetches to discover all types and their
/// per-type YAML paths. This supersedes keasy's server-built `DataManifest`
/// (the query side now reads the same artifact the writer emits — single
/// source).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphInfo {
    /// Graph label, e.g. `"graph"`. Emitted as the spec key `name`.
    pub name: String,
    /// Prefix the `vertices`/`edges` entries are relative to. Usually `""`
    /// — the entries are already `<dest>`-relative `rel_path`s.
    pub prefix: String,
    /// Which container carries every payload set of this corpus — here once,
    /// because two payload sets in different containers would be two addressing
    /// schemes in one corpus. See [`Container`].
    #[serde(default)]
    pub container: Container,
    /// The privacy bound this corpus declares — here once, because the bound is
    /// a property of the **whole release** and not of a column, and because
    /// `packages/corpus/src/corpus.ts` takes the payload vocabulary from the
    /// bytes rather than from a projection's `properties`: a field the reader never opens
    /// is not a policy. See [`Privacy`].
    ///
    /// `#[serde(default)]` so a corpus written before this field existed still
    /// deserialises, to "unknown" rather than to "public".
    #[serde(default)]
    pub privacy: Privacy,
    /// Relative paths to each vertex-info YAML, e.g. `vertex/Person.vertex.yml`.
    pub vertices: Vec<String>,
    /// Relative paths to each edge-info YAML, e.g.
    /// `edge/Person_knows_Person/Person_knows_Person.edge.yml`.
    pub edges: Vec<String>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// **One projection of a corpus's sequence: a scale, a path, and the columns.**
///
/// A corpus is an order (Morton over the positions, and `dense_id` is the rank
/// in it), a cut (fixed runs of `chunk_size`), and some projections of that
/// sequence. This is one of them, and the four artefacts of a corpus that used
/// to be four vocabularies are four of these:
///
/// | artefact | is |
/// |---|---|
/// | payload | `scale: 1`, the drawing columns plus identity |
/// | vertex level | `scale: 4^k`, the same columns over one row in `4^k` |
/// | adjacency | `scale: 1`, `aligned_by` an endpoint, the endpoint columns |
/// | edge level | `scale: 4^k`, `aligned_by: src`, both endpoints' coordinates |
///
/// The payload is not a special case — it is the projection whose scale is one.
///
/// # Addressing one needs the scale and nothing else
///
/// Tile `j` covers the `dense_id` range
/// `[j · chunk_size · scale, (j+1) · chunk_size · scale)`, so a reader's shift
/// is the type's own plus `log2(scale)` and its row count is
/// `count.div_ceil(scale)`. **No reader needs the exponent**: `4` and `2k` live
/// in [`VertexLevels`] on the writer's side, and what crosses into the document
/// is the product. Under [`Container::Files`] tile `j` is
/// `<type prefix><path>chunk{j}.parquet`, and under [`Container::RowGroups`] it
/// is `<type prefix><path>tiles.parquet` with the footer saying which row group
/// — the same two spellings whatever the scale, because a level is not a
/// different kind of thing from a payload. [`VertexIndex`] is the artefact that
/// spells its files `tile{k}` instead, and it is the one that is not a
/// projection.
///
/// # An edge level
///
/// **Level `k` of a relation is the edges incident to a level-`k` vertex** — in
/// either orientation, `src_dense % scale == 0 OR dst_dense % scale == 0` — and
/// each row carries **both endpoints' coordinates**. The positions are the whole
/// point: a camera keeps an edge with ONE end drawn, and a vertex level at the
/// app's three-pixel floor can position **0.79%** of the edges the same view
/// draws, so a pyramid of vertices alone answers a view with links by opening
/// the payload. `src_x`/`src_y`/`dst_x`/`dst_y` make the projection
/// **self-drawing** — the lines and their far ends come out of one file.
///
/// It decimates and never aggregates, and [`VertexLevels`] is where that
/// argument lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    /// Where this projection's tiles are, relative to the type's own `prefix`,
    /// with the trailing separator — `""` for the payload, `"l1/"` for a level,
    /// `"by_source/"` for an orientation.
    ///
    /// `OME-NGFF`'s own spelling: a `multiscales` dataset entry carries a
    /// `path`, and this is that field doing that job. Declared rather than
    /// conventional because it is the one part of a tile's URL a reader cannot
    /// compute, and there is no directory to list over HTTP.
    pub path: String,
    /// **How many `dense_id`s one row of this projection stands for** — `1` for
    /// the payload and the adjacency, `4^k` for level `k`.
    ///
    /// `OME-NGFF` spells a resolution's downsampling factor `scale`, inside a
    /// `coordinateTransformations` entry of type `scale`; this is the same
    /// number doing the same job, flattened to the one axis a sequence has.
    /// [`VertexLevels::stride`] is where it comes from and the only place the
    /// pyramid's base is written down.
    pub scale: u64,
    /// Which endpoint column addresses these tiles — `"src"` or `"dst"` — on an
    /// edge projection, and `None` on a vertex one.
    ///
    /// It is what makes an edge projection addressable at all: `src` means tile
    /// `k` holds the rows whose `src_dense >> shift` is `k`, with the shift
    /// taken from [`EdgeInfo::src_chunk_size`]; `dst` the same against
    /// `dst_dense` and [`EdgeInfo::dst_chunk_size`]. A level of a relation is
    /// always `src`: a level is *which vertices are in it*, and the source
    /// type's own pyramid is what says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aligned_by: Option<String>,
    /// Whether the rows are sorted by [`Self::aligned_by`]'s column. `None` on
    /// a vertex projection, whose order is the corpus's one order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordered: Option<bool>,
    /// Storage file type, e.g. `"parquet"`.
    pub file_type: String,
    /// The columns this projection carries, which is the other half of what a
    /// projection *is*. A level of a vertex type carries the payload's columns
    /// over a quarter of the rows; a level of a relation carries four more than
    /// the adjacency beside it.
    pub properties: Vec<Property>,
}

impl Projection {
    /// A projection at `scale: 1` — a payload, or one orientation's adjacency.
    #[must_use]
    pub fn payload(path: impl Into<String>, properties: Vec<Property>) -> Self {
        Self {
            path: path.into(),
            scale: 1,
            aligned_by: None,
            ordered: None,
            file_type: "parquet".to_string(),
            properties,
        }
    }

    /// Declare which endpoint addresses this projection, and whether its rows
    /// are sorted by that column. Only an edge projection has one.
    #[must_use]
    pub fn aligned_by(mut self, endpoint: impl Into<String>, ordered: bool) -> Self {
        self.aligned_by = Some(endpoint.into());
        self.ordered = Some(ordered);
        self
    }
}

/// A single property (column) of a vertex or edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    /// Property (column) name.
    pub name: String,
    /// `GraphAr` data-type spelling (`int64`, `string`, `double`, `bool`, ...), derived from
    /// an [`arrow_schema::DataType`] via [`data_type_name`].
    pub data_type: String,
    /// Whether this property is (part of) the primary key.
    pub is_primary: bool,
    /// Nullability, omitted from the YAML when `None` (`GraphAr` treats it as optional).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_nullable: Option<bool>,
    /// **How many values the shape declared this predicate may carry**, or
    /// `None` where nothing declared it.
    ///
    /// `is_nullable` is the neighbouring fact and not this one: it answers
    /// whether a value may be absent, and this answers whether there may be
    /// more than one. `?` is nullable and single; `+` is non-nullable and
    /// multi; the two axes are independent and the manifest carried only one.
    ///
    /// **`None` is a statement about the writer, not about the data.** A corpus
    /// whose types were inferred rather than declared has no cardinality to
    /// record, and so does one written before this field existed; both read
    /// back as "not declared" rather than as "single", which is the answer a
    /// default would have invented.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cardinality: Option<Cardinality>,
}

/// How many bits a `dense_id` is shifted right by to name the tile holding it.
///
/// **The operands, spelled out**, because the three characters mean three
/// different things across the layers that have to agree: the input is an
/// unsigned 64-bit `dense_id`, the shift is logical, and the result is an
/// unsigned 64-bit tile number. `>>` is arithmetic on a signed type in Rust,
/// and in JavaScript it truncates to 32 bits before shifting. The border
/// vectors a re-implementation is checked against are in this module's tests,
/// and they run past the `uint32` the column declares so that widening it later
/// moves no reader. Which ceiling binds is on
/// `/docs/format/conventions/identity`.
pub const TILE_SHIFT: u32 = 12;

/// The tile a `dense_id` lives in — the whole of the addressing scheme.
///
/// There is no tile tree and nothing to discover: tile `i` **is** the range
/// `[i·4096, (i+1)·4096)`, its parent is a further shift, and the lowest common
/// ancestor of two vertices is the common prefix of their ids. A reader computes
/// every URL it wants before it emits the first request, which is the whole
/// content of *the camera is addressed, not queried*.
#[must_use]
pub const fn tile_of(dense_id: u64) -> u64 {
    dense_id >> TILE_SHIFT
}

/// Rows per tile when a mapping does not override it — `1 << TILE_SHIFT`.
///
/// **A tile is a fixed 4,096-row `dense_id` range.** The measurement that chose
/// it — requests against bytes over five million vertices, and why the two
/// curves have no common optimum — is on
/// `/docs/format/conventions/addressing`. Two things worth knowing before
/// touching this: the byte curve is flat from 1,024 to 8,192, so a change
/// inside that band is noise with a `git blame` on it; and the value must stay
/// a power of two, because a tile's address is [`tile_of`], a shift, and a
/// shift is not a division.
pub const DEFAULT_CHUNK_SIZE: u64 = 1 << TILE_SHIFT;

impl VertexInfo {
    /// Construct a `VertexInfo` with the [`GRAPHAR_VERSION`] preset.
    #[must_use]
    pub fn new(
        vertex_type: impl Into<String>,
        vertex_count: u64,
        chunk_size: u64,
        prefix: impl Into<String>,
        projections: Vec<Projection>,
    ) -> Self {
        Self {
            vertex_type: vertex_type.into(),
            iri: String::new(),
            vertex_count,
            chunk_size,
            prefix: prefix.into(),
            projections,
            // No index by default, and that is not a stub: writing one is a
            // second pass over the rows in a different order, which the caller
            // that HAS those rows decides to pay. `with_index` is how it says so.
            index: None,
            // Not declared, which is neither "derived" nor "none". A writer that
            // knows where its positions came from says so with
            // `with_coordinates`; a writer that does not must not be made to
            // look as though it did.
            coordinates: None,
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// Declare that this type carries an identity index. See [`VertexIndex`].
    #[must_use]
    pub fn with_index(mut self, index: VertexIndex) -> Self {
        self.index = Some(index);
        self
    }

    /// Declare the coordinate systems this type carries. See
    /// [`CoordinateSystem`].
    ///
    /// Replaces rather than appends: the list **is** the declaration, and a
    /// writer that would rather add one reads the current list and hands back a
    /// longer one. An empty `systems` declares that the type carries none,
    /// which is a different answer from not calling this at all.
    #[must_use]
    pub fn with_coordinates(mut self, systems: Vec<CoordinateSystem>) -> Self {
        self.coordinates = Some(systems);
        self
    }

    /// The declared systems whose positions are data rather than a drawing
    /// algorithm's choice — empty where none are, and empty where nothing was
    /// declared, which a caller that needs to tell those apart separates with
    /// [`Self::coordinates`] itself.
    #[must_use]
    pub fn measured_coordinates(&self) -> Vec<&CoordinateSystem> {
        self.coordinates
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|system| system.is_data())
            .collect()
    }

    /// Append the projections of a level plan, each carrying the payload's own
    /// columns — which is what the layout pass writes into them.
    #[must_use]
    pub fn with_levels(mut self, plan: &VertexLevels) -> Self {
        let (file_type, properties) = self.payload().map_or_else(
            || ("parquet".to_string(), Vec::new()),
            |p| (p.file_type.clone(), p.properties.clone()),
        );
        self.projections
            .extend(plan.projections(&file_type, &properties));
        self
    }

    /// This type's payload — the projection at `scale: 1`.
    ///
    /// `Option` because a manifest is a document somebody else may have
    /// written, and one that names no payload is one this cannot invent.
    #[must_use]
    pub fn payload(&self) -> Option<&Projection> {
        self.projections.iter().find(|p| p.scale == 1)
    }

    /// The payload's columns, in manifest order — the field list every schema
    /// verb reads. Empty when the manifest names no payload.
    #[must_use]
    pub fn properties(&self) -> &[Property] {
        self.payload().map_or(&[], |p| p.properties.as_slice())
    }

    /// Serialize this vertex-info to `GraphAr` v1.0.0 YAML.
    ///
    /// # Errors
    /// Returns the underlying `serde_yaml_ng` error if serialization fails (it cannot, for
    /// these plain structs, but the signature is honest).
    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }
}

impl EdgeInfo {
    /// Append the projections of the SOURCE type's level plan, source-aligned
    /// and carrying `properties` — both endpoints' ids and their coordinates.
    ///
    /// **One plan, two artefacts.** The levels are the source type's, not a
    /// second choice made here: a level of a relation is *which vertices are in
    /// it*, so a relation whose source type writes 1..=4 writes 1..=4 or it
    /// writes nothing. [`VertexLevels::planned`] stays the one place the
    /// numbers are chosen.
    #[must_use]
    pub fn with_levels(mut self, plan: &VertexLevels, properties: &[Property]) -> Self {
        self.projections.extend(
            plan.projections("parquet", properties)
                .into_iter()
                .map(|p| p.aligned_by("src", true)),
        );
        self
    }

    /// The adjacency for one orientation — the projection at `scale: 1` aligned
    /// by `endpoint`. `None` when the corpus does not publish that half.
    #[must_use]
    pub fn adjacency(&self, endpoint: &str) -> Option<&Projection> {
        self.projections
            .iter()
            .find(|p| p.scale == 1 && p.aligned_by.as_deref() == Some(endpoint))
    }

    /// Serialize this edge-info to `GraphAr` v1.0.0 YAML.
    ///
    /// # Errors
    /// Returns the underlying `serde_yaml_ng` error if serialization fails.
    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }
}

impl GraphInfo {
    /// Construct a `GraphInfo` with the [`GRAPHAR_VERSION`] preset.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        prefix: impl Into<String>,
        container: Container,
        vertices: Vec<String>,
        edges: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            prefix: prefix.into(),
            container,
            // [`Privacy::Undeclared`] and not a parameter, because the great
            // majority of corpora carry no bound and a constructor argument
            // every caller passes the same value to is a way of getting it
            // wrong once. The bound is attached by [`Self::with_privacy`],
            // which only the verifier calls.
            privacy: Privacy::Undeclared,
            vertices,
            edges,
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// Attach a verified privacy bound.
    ///
    /// **Only a verifier calls this**, and it is a separate method rather than
    /// a `new` parameter for that reason: a bound that could be set by the
    /// builder is a bound that can be declared without being measured, which is
    /// the one failure mode this whole field exists to make impossible.
    #[must_use]
    pub fn with_privacy(mut self, privacy: Privacy) -> Self {
        self.privacy = privacy;
        self
    }

    /// Serialize this graph-info to `GraphAr` v1.0.0 YAML.
    ///
    /// # Errors
    /// Returns the underlying `serde_yaml_ng` error if serialization fails.
    pub fn to_yaml(&self) -> Result<String, serde_yaml_ng::Error> {
        serde_yaml_ng::to_string(self)
    }
}

/// Map an [`arrow_schema::DataType`] to its `GraphAr` `data_type` string spelling.
///
/// `arrow-schema` is the single authority for the spellings — never
/// hand-rolled. Unhandled arrow types fall back to `binary`, which is
/// **fossil's fallback and not a `GraphAr` spelling**; `time` is the same
/// problem from the other side, in the specification's list with no arm in the
/// C++. Both are divergences, and `/docs/design/corpus` has them beside the
/// `uint32` that is the real one.
///
/// **Nothing enforces that the declared type matches the column the writer
/// emits**, except for the three names every convention *shifts*:
/// `apps/corpus/guards/guards.mjs`'s `addressing-is-unsigned` opens the payload
/// and requires `dense_id`, `src_dense` and `dst_dense` to hold an unsigned
/// integer, because one stored signed shifts arithmetically and reaches a tile
/// that does not exist. The rest of the property list is open, and
/// `fossil-cli/tests/declared_type_reaches_the_column.rs` is where the gap is
/// measured. Neither half could be closed here: this crate emits no bytes and
/// holds `arrow-schema` alone on purpose (see `Cargo.toml`).
#[must_use]
pub fn data_type_name(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "bool",
        DataType::Int8 | DataType::Int16 | DataType::Int32 => "int32",
        DataType::Int64 => "int64",
        DataType::Float16 | DataType::Float32 => "float",
        DataType::Float64 => "double",
        DataType::Utf8 | DataType::LargeUtf8 => "string",
        DataType::Date32 | DataType::Date64 => "date",
        DataType::Timestamp(_, _) => "timestamp",
        DataType::Time32(_) | DataType::Time64(_) => "time",
        _ => "binary",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person_vertex() -> VertexInfo {
        VertexInfo::new(
            "Person",
            10_000,
            DEFAULT_CHUNK_SIZE,
            "vertex/person/",
            vec![Projection::payload(
                "",
                vec![
                    Property {
                        name: "id".to_string(),
                        data_type: data_type_name(&DataType::Int64),
                        is_primary: true,
                        is_nullable: Some(false),
                        cardinality: Some(Cardinality::Single),
                    },
                    Property {
                        name: "name".to_string(),
                        data_type: data_type_name(&DataType::Utf8),
                        is_primary: false,
                        is_nullable: None,
                        cardinality: None,
                    },
                ],
            )],
        )
    }

    fn knows_edge() -> EdgeInfo {
        EdgeInfo {
            src_type: "Person".to_string(),
            edge_type: "knows".to_string(),
            iri: String::new(),
            dst_type: "Person".to_string(),
            cardinality: Some(Cardinality::Multi),
            edge_count: 19_998,
            chunk_size: DEFAULT_CHUNK_SIZE,
            src_chunk_size: DEFAULT_CHUNK_SIZE,
            dst_chunk_size: DEFAULT_CHUNK_SIZE,
            directed: true,
            prefix: "edge/person_knows_person/".to_string(),
            projections: vec![
                Projection::payload("by_source/", endpoint_columns()).aligned_by("src", true),
                Projection::payload("by_target/", endpoint_columns()).aligned_by("dst", true),
            ],
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// The two columns every adjacency tile carries — the pair that IS the edge.
    fn endpoint_columns() -> Vec<Property> {
        ["src_dense", "dst_dense"]
            .into_iter()
            .map(|name| Property {
                name: name.to_string(),
                data_type: "uint32".to_string(),
                is_primary: false,
                is_nullable: Some(false),
                cardinality: Some(Cardinality::Single),
            })
            .collect()
    }

    /// The border vectors of [`tile_of`], which are the deliverable: a second
    /// implementation is checked against this table and not against a sentence.
    /// The values past `2^32 − 1` exceed the `uint32` a `dense_id` column holds
    /// today, and are here because they are where a port that took the shift as
    /// signed, or ran it through a JavaScript `number`, answers differently.
    #[test]
    fn tile_of_border_vectors() {
        for (dense_id, tile) in [
            (0u64, 0u64),
            (4_095, 0),
            (4_096, 1),
            (8_191, 1),
            (2_147_483_647, 524_287),                   // 2^31 − 1
            (2_147_483_648, 524_288),                   // 2^31
            (4_294_967_295, 1_048_575),                 // 2^32 − 1, the last id a `uint32` holds
            (9_007_199_254_740_992, 2_199_023_255_552), // 2^53
        ] {
            assert_eq!(tile_of(dense_id), tile, "tile_of({dense_id})");
        }
        // The shift and the row count are one statement, not two that agree.
        assert_eq!(DEFAULT_CHUNK_SIZE, 4_096);
        assert_eq!(tile_of(DEFAULT_CHUNK_SIZE - 1), 0);
        assert_eq!(tile_of(DEFAULT_CHUNK_SIZE), 1);
    }

    /// How many tiles a declared count implies — the Rust half of the
    /// `declared_count` section of `apps/corpus/guards/vectors.json`. The
    /// borders are a count that exactly fills a tile and one a row over, the
    /// tail tile being what a truncated corpus loses and no other check sees.
    #[test]
    fn a_count_implies_a_tile_count() {
        for (count, chunk_size, tiles, last) in [
            (0u64, 4_096u64, 0u64, 0u64),
            (1, 4_096, 1, 1),
            (4_095, 4_096, 1, 4_095),
            (4_096, 4_096, 1, 4_096),
            (4_097, 4_096, 2, 1),
            (300, 64, 5, 44),
            (9_007_199_254_740_993, 4_096, 2_199_023_255_553, 1),
        ] {
            assert_eq!(count.div_ceil(chunk_size), tiles, "tiles of {count}");
            let tail = count - tiles.saturating_sub(1) * chunk_size;
            assert_eq!(if tiles == 0 { 0 } else { tail }, last, "tail of {count}");
        }
    }

    /// The count is required, and a manifest without one does not deserialize —
    /// the `u64`-not-`Option<u64>` decision as a test. Made to pass by adding a
    /// `#[serde(default)]`, the field has been turned back into a hint.
    #[test]
    fn a_manifest_without_a_count_is_not_a_manifest() {
        let yaml = person_vertex().to_yaml().expect("serialize");
        let without = yaml
            .lines()
            .filter(|line| !line.starts_with("vertex_count:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(yaml.contains("vertex_count: 10000"), "{yaml}");
        let error = serde_yaml_ng::from_str::<VertexInfo>(&without)
            .expect_err("a vertex-info with no count must not deserialize");
        assert!(error.to_string().contains("vertex_count"), "{error}");
    }

    /// The divergence `GraphAr`'s own edge-info example carries, executed rather
    /// than asserted: its `adj_lists` entries name no location at all, and a
    /// location is the one part of a tile's URL a reader cannot compute — so
    /// [`Projection::path`] has no `serde` default and a specification-shaped
    /// edge-info does not deserialize.
    #[test]
    fn a_projection_without_a_path_is_not_a_projection() {
        let spec_shaped = "\
src_type: person
edge_type: knows
dst_type: person
edge_count: 1024
chunk_size: 1024
src_chunk_size: 100
dst_chunk_size: 100
directed: false
prefix: edge/person_knows_person/
projections:
- scale: 1
  aligned_by: src
  ordered: true
  file_type: parquet
  properties: []
version: gar/v1
";
        let error = serde_yaml_ng::from_str::<EdgeInfo>(spec_shaped)
            .expect_err("a projection with no path has no address and must not deserialize");
        assert!(error.to_string().contains("path"), "{error}");
    }

    /// **The payload is a projection and not a special case** — the claim, as a
    /// test. One vocabulary describes the two halves of a vertex type and the
    /// two halves of a relation, and the only thing that separates a payload
    /// from a level of it is a number.
    #[test]
    fn the_payload_is_the_projection_whose_scale_is_one() {
        let plan = VertexLevels::planned(1_000_000, DEFAULT_CHUNK_SIZE).expect("over one tile");
        let vertex = person_vertex().with_levels(&plan);
        let scales: Vec<u64> = vertex.projections.iter().map(|p| p.scale).collect();
        assert_eq!(scales, vec![1, 4, 16, 64, 256]);
        // Every level carries what the payload carries: the layout pass writes
        // the payload's own schema into them, one row in `scale`.
        let payload = vertex.payload().expect("a payload");
        assert_eq!(payload.path, "");
        for level in vertex.projections.iter().filter(|p| p.scale > 1) {
            assert_eq!(level.properties, payload.properties, "{}", level.path);
            assert_eq!(level.aligned_by, None);
        }
        // And on a relation the same list holds both orientations and the
        // pyramid, told apart by `aligned_by` and by the scale.
        let edge = knows_edge().with_levels(&plan, &endpoint_columns());
        assert_eq!(edge.adjacency("src").expect("CSR").path, "by_source/");
        assert_eq!(edge.adjacency("dst").expect("CSC").path, "by_target/");
        assert_eq!(edge.adjacency("nowhere"), None);
        let levelled: Vec<&str> = edge
            .projections
            .iter()
            .filter(|p| p.scale > 1)
            .map(|p| p.path.as_str())
            .collect();
        assert_eq!(levelled, vec!["l1/", "l2/", "l3/", "l4/"]);
        // A level of a relation is which vertices are in it, so it is always
        // source-aligned — there is no second choice made on the edge side.
        assert!(
            edge.projections
                .iter()
                .filter(|p| p.scale > 1)
                .all(|p| p.aligned_by.as_deref() == Some("src"))
        );
    }

    /// The scale a projection declares is `VertexLevels::stride` and never a
    /// second spelling of it — asserted against the algebra, not a literal.
    #[test]
    fn a_projections_scale_is_the_strides_own_number() {
        let plan = VertexLevels::planned(1_000_000, DEFAULT_CHUNK_SIZE).expect("over one tile");
        for (projection, &level) in plan
            .projections("parquet", &[])
            .iter()
            .zip(plan.levels.iter())
        {
            assert_eq!(projection.scale, VertexLevels::stride(level));
            assert_eq!(projection.path, plan.level_prefix(level));
            // What a reader spends it on, without ever seeing the exponent: the
            // rows are a division and the shift is a count of trailing zeros.
            assert_eq!(
                1_000_000u64.div_ceil(projection.scale),
                VertexLevels::rows_at(1_000_000, level)
            );
            assert_eq!(
                projection.scale.trailing_zeros(),
                VertexLevels::stride_bits(level)
            );
        }
    }

    #[test]
    fn data_type_name_uses_spec_spellings() {
        assert_eq!(data_type_name(&DataType::Int64), "int64");
        assert_eq!(data_type_name(&DataType::Utf8), "string");
        assert_eq!(data_type_name(&DataType::Float64), "double");
        assert_eq!(data_type_name(&DataType::Boolean), "bool");
    }

    #[test]
    fn vertex_yaml_carries_graphar_v1_field_names() {
        let yaml = person_vertex().to_yaml().expect("vertex serialization");
        assert!(yaml.contains("version: gar/v1"), "{yaml}");
        assert!(yaml.contains("type: Person"), "{yaml}");
        // The count sits before the tiling, because
        // `tiles = vertex_count.div_ceil(chunk_size)` reads in that order.
        let count_at = yaml.find("vertex_count: 10000").expect(&yaml);
        let chunk_at = yaml.find("chunk_size:").expect(&yaml);
        assert!(count_at < chunk_at, "{yaml}");
        // Asserted against the constant, not a literal: the value is a measured trade-off
        // (see DEFAULT_CHUNK_SIZE) and this test is about the spec *spelling* of the key.
        assert!(
            yaml.contains(&format!("chunk_size: {DEFAULT_CHUNK_SIZE}")),
            "{yaml}"
        );
        assert!(yaml.contains("prefix: vertex/person/"), "{yaml}");
        assert!(yaml.contains("projections:"), "{yaml}");
        assert!(!yaml.contains("property_groups"), "{yaml}");
        assert!(yaml.contains("data_type: int64"), "{yaml}");
        assert!(yaml.contains("is_primary: true"), "{yaml}");
        assert!(!yaml.contains("graphar_version"), "{yaml}");
        assert!(!yaml.contains("vertex_types"), "{yaml}");
    }

    #[test]
    fn vertex_yaml_skips_none_nullable_but_keeps_some() {
        let yaml = person_vertex().to_yaml().expect("vertex serialization");
        assert!(yaml.contains("is_nullable: false"), "{yaml}");
        assert_eq!(yaml.matches("is_nullable").count(), 1, "{yaml}");
    }

    #[test]
    fn vertex_info_round_trips_through_yaml() {
        let original = person_vertex();
        let yaml = original.to_yaml().expect("serialize");
        let parsed: VertexInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(original, parsed);
    }

    #[test]
    fn edge_info_round_trips_through_yaml() {
        let original = knows_edge();
        let yaml = original.to_yaml().expect("serialize");
        let parsed: EdgeInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(original, parsed);
    }

    /// The property the whole field is written under: a corpus from before it
    /// existed reads back as **not declared**, never as `derived`. A default
    /// here would invent the answer for every corpus already on disk, and
    /// `derived` is the answer a reader would then act on — which is the defect
    /// the field exists to remove rather than relocate.
    #[test]
    fn a_vertex_written_before_the_field_existed_declares_nothing() {
        let old = "type: Person\nvertex_count: 5\nchunk_size: 4096\n\
                   prefix: vertex/Person/\nprojections: []\nversion: gar/v1\n";
        let parsed: VertexInfo = serde_yaml_ng::from_str(old).expect("deserialize");
        assert_eq!(parsed.coordinates, None);
        assert!(parsed.measured_coordinates().is_empty());
    }

    /// Undeclared is absent, not `coordinates: null` — the same shape
    /// `is_nullable` and `cardinality` keep, so a corpus that declares nothing
    /// is byte-for-byte the corpus it was before the field landed.
    #[test]
    fn an_undeclared_system_is_absent_from_the_yaml() {
        let yaml = person_vertex().to_yaml().expect("serialize");
        assert!(!yaml.contains("coordinates"), "{yaml}");
    }

    /// Declaring an empty list says «this type carries no coordinate system»,
    /// which is an answer. Not calling `with_coordinates` says nothing. The two
    /// must not collapse, because only the first licenses a reader to conclude.
    #[test]
    fn declaring_no_systems_is_not_declaring_nothing() {
        let silent = person_vertex();
        let explicit = person_vertex().with_coordinates(Vec::new());
        assert_eq!(silent.coordinates, None);
        assert_eq!(explicit.coordinates, Some(Vec::new()));
        assert_ne!(silent, explicit);

        let yaml = explicit.to_yaml().expect("serialize");
        let parsed: VertexInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(parsed.coordinates, Some(Vec::new()));
    }

    /// Two systems over one type, which is the case the field is a LIST for:
    /// authors have affiliations with real locations *and* a community layout,
    /// and which one is drawn is the reader's question.
    #[test]
    fn two_systems_round_trip_with_their_provenance() {
        let original = person_vertex().with_coordinates(vec![
            CoordinateSystem::measured("geo", "lon", "lat", Provenance::Geographic),
            CoordinateSystem::derived("layout", "x", "y", "louvain+phyllotaxis"),
        ]);
        let yaml = original.to_yaml().expect("serialize");
        assert!(yaml.contains("provenance: geographic"), "{yaml}");
        assert!(yaml.contains("provenance: derived"), "{yaml}");
        assert!(yaml.contains("derived_by: louvain+phyllotaxis"), "{yaml}");

        let parsed: VertexInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(original, parsed);
    }

    /// A measured system carries no `derived_by` and the key stays out of the
    /// document, rather than appearing as a null a reader has to interpret.
    #[test]
    fn a_measured_system_writes_no_deriver() {
        let yaml = person_vertex()
            .with_coordinates(vec![CoordinateSystem::measured(
                "geo",
                "lon",
                "lat",
                Provenance::Geographic,
            )])
            .to_yaml()
            .expect("serialize");
        assert!(!yaml.contains("derived_by"), "{yaml}");
    }

    /// The one question a far view turns on, as a predicate: a position an
    /// algorithm chose is not data, and the other two are.
    #[test]
    fn only_a_derived_position_fails_to_be_data() {
        let info = person_vertex().with_coordinates(vec![
            CoordinateSystem::measured("geo", "lon", "lat", Provenance::Geographic),
            CoordinateSystem::measured("umap", "u0", "u1", Provenance::Embedded),
            CoordinateSystem::derived("layout", "x", "y", "louvain+phyllotaxis"),
        ]);
        let measured: Vec<&str> = info
            .measured_coordinates()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(measured, ["geo", "umap"]);
    }

    #[test]
    fn edge_yaml_carries_graphar_v1_field_names() {
        let yaml = knows_edge().to_yaml().expect("edge serialization");
        assert!(yaml.contains("src_type: Person"), "{yaml}");
        assert!(yaml.contains("dst_type: Person"), "{yaml}");
        assert!(yaml.contains("edge_type: knows"), "{yaml}");
        assert!(yaml.contains("edge_count: 19998"), "{yaml}");
        assert!(yaml.contains("projections:"), "{yaml}");
        assert!(!yaml.contains("adj_lists"), "{yaml}");
        // Both orientations, each saying where its tiles are: a reader with only
        // `aligned_by` knows the arithmetic and not the address.
        assert!(yaml.contains("aligned_by: src"), "{yaml}");
        assert!(yaml.contains("path: by_source/"), "{yaml}");
        assert!(yaml.contains("aligned_by: dst"), "{yaml}");
        assert!(yaml.contains("path: by_target/"), "{yaml}");
        assert!(yaml.contains("directed: true"), "{yaml}");
        assert!(yaml.contains("version: gar/v1"), "{yaml}");
    }

    fn bounded_graph() -> GraphInfo {
        GraphInfo::new(
            "graph",
            "",
            Container::RowGroups,
            vec!["vertex/Person.vertex.yml".to_string()],
            vec![],
        )
        .with_privacy(Privacy::KAnonymity(KAnonymity {
            k: 5,
            reached: 12,
            absent_quasi_identifier: AbsentQuasiIdentifier::Suppress,
            population: 70_000,
            suppressed: 143,
            suppression_budget_ppm: 20_000,
            quasi_identifiers: "Person.birth_year Person.postcode Person.sex".to_string(),
            generalization: "Person.birth_year@bucket Person.postcode@1-3/4".to_string(),
            policy: "https://example.org/policies/persons-v1".to_string(),
            profile: "https://fossil-lang.org/ns/privacy/v1".to_string(),
        }))
    }

    #[test]
    fn an_undeclared_bound_is_written_down_rather_than_left_out() {
        // Mandatory on write: a producer with no bound says so, because "no
        // bound" and "written before the field existed" must not collapse.
        let yaml = GraphInfo::new("graph", "", Container::RowGroups, vec![], vec![])
            .to_yaml()
            .expect("serialize");
        assert!(yaml.contains("privacy:"), "{yaml}");
        assert!(yaml.contains("bound: undeclared"), "{yaml}");
        assert!(
            !yaml.contains("k:"),
            "an undeclared bound declares no k\n{yaml}"
        );
    }

    #[test]
    fn a_manifest_written_before_the_field_existed_still_reads() {
        // Ignorable on read: the `graph.graph.yml` of every corpus written
        // before the field, deserialising to «unknown» and not to «public».
        let old = "name: graph\nprefix: ''\ncontainer: rowgroups\n\
                   vertices:\n- vertex/Person.vertex.yml\nedges: []\nversion: gar/v1\n";
        let parsed: GraphInfo = serde_yaml_ng::from_str(old).expect("deserialize");
        assert_eq!(parsed.privacy, Privacy::Undeclared);
    }

    #[test]
    fn graph_info_round_trips_a_bound_through_yaml() {
        let original = bounded_graph();
        let yaml = original.to_yaml().expect("serialize");
        let parsed: GraphInfo = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(original, parsed);
    }

    /// `apps/corpus/guards/manifest.mjs` and `packages/corpus/src/manifest.ts`
    /// are line scanners that **skip silently** what falls outside a flat
    /// mapping, so the emitted `privacy:` block has to stay inside one — said
    /// here, in the crate that emits it, rather than in the two that read it.
    #[test]
    fn the_privacy_block_stays_inside_the_grammar_the_line_scanners_read() {
        let yaml = bounded_graph().to_yaml().expect("serialize");
        let lines: Vec<&str> = yaml.lines().collect();
        let start = lines
            .iter()
            .position(|l| *l == "privacy:")
            .expect("a privacy block");
        let block: Vec<&str> = lines[start + 1..]
            .iter()
            .take_while(|l| l.starts_with("  "))
            .copied()
            .collect();
        assert!(!block.is_empty(), "{yaml}");
        for line in &block {
            // Exactly two spaces of indent and a `key: scalar` — one level, no
            // sequence, no third level. A `- ` item or a four-space line here is
            // what the scanners drop on the floor.
            assert!(
                line.starts_with("  ") && !line.starts_with("   "),
                "`{line}` is deeper than one level\n{yaml}"
            );
            let rest = &line[2..];
            assert!(
                !rest.starts_with("- "),
                "`{line}` is a sequence item\n{yaml}"
            );
            let (_, value) = rest.split_once(": ").unwrap_or_else(|| {
                panic!("`{line}` is not `key: scalar`\n{yaml}");
            });
            assert!(
                !value.starts_with('[') && !value.starts_with('{'),
                "`{line}` is flow style, which both scanners REFUSE\n{yaml}"
            );
        }
        // And the set is one scalar, which is the whole reason it is spelled
        // with spaces instead of as a list.
        assert!(
            block.contains(&"  quasi_identifiers: Person.birth_year Person.postcode Person.sex"),
            "{yaml}"
        );
    }

    /// The budget is integer arithmetic, and this is the comparison a reader
    /// reproduces. 143 suppressed of 70,000 is 2,042.8 ppm, under a 20,000 ppm
    /// (2%) allowance — and the point is that no float appears on either side.
    #[test]
    fn the_suppression_budget_compares_without_a_float() {
        let Privacy::KAnonymity(bound) = bounded_graph().privacy else {
            panic!("a bound");
        };
        assert!(bound.suppressed * 1_000_000 <= bound.population * bound.suppression_budget_ppm);
        // And it bites: 1,401 of 70,000 is 20,014 ppm, which is over. Read off
        // a value, not written as a constant expression the compiler folds.
        let over = KAnonymity {
            suppressed: 1_401,
            ..bound
        };
        assert!(over.suppressed * 1_000_000 > over.population * over.suppression_budget_ppm);
    }

    /// The corpus the encargo is about: a million vertices at 4,096 rows a
    /// tile is 245 tiles, and the pyramid over it is four levels — one per zoom
    /// step, complete down to the level a single range request answers.
    #[test]
    fn planned_levels_over_a_million_vertices() {
        let plan = VertexLevels::planned(1_000_000, DEFAULT_CHUNK_SIZE).expect("over one tile");
        assert_eq!(plan.levels, vec![1, 2, 3, 4]);
        let rows: Vec<u64> = plan
            .levels
            .iter()
            .map(|&k| VertexLevels::rows_at(1_000_000, k))
            .collect();
        assert_eq!(rows, vec![250_000, 62_500, 15_625, 3_907]);
        // The coarsest fits one tile and the one below it does not, which is
        // the whole definition of where the pyramid stops.
        assert!(rows[3] <= DEFAULT_CHUNK_SIZE);
        assert!(VertexLevels::rows_at(1_000_000, 3) > DEFAULT_CHUNK_SIZE);
        // A third of the type, which is what a complete pyramid in quarters
        // costs. The five-level window it replaces wrote 121,095 — cheaper,
        // and it answered one of the four rectangles a camera path asked for.
        assert_eq!(rows.iter().sum::<u64>(), 332_032);
    }

    /// The cost is a FRACTION of the type, not a constant number of tiles —
    /// which is what retired the window. A complete pyramid in quarters is
    /// `1/4 + 1/16 + …`, bounded by a third whatever `V` is, so there is no
    /// unbounded cost left for a constant to cap.
    #[test]
    fn the_pyramid_costs_a_third_of_the_type() {
        for &v in &[300_000u64, 1_000_000, 5_000_000, 10_000_000] {
            let plan = VertexLevels::planned(v, DEFAULT_CHUNK_SIZE).expect("over one tile");
            let levels = plan.levels.len() as u64;
            let rows: u64 = plan
                .levels
                .iter()
                .map(|&k| VertexLevels::rows_at(v, k))
                .sum();
            // Under a third, and the slack is one row per level: every term is
            // a `ceil`, and there is one term per level.
            assert!(rows <= v / 3 + levels, "{v}: {rows} rows is over a third");
            // And over a quarter, because level 1 alone is a quarter. A pyramid
            // that came in under it would be one with a level missing.
            assert!(rows * 4 >= v, "{v}: {rows} rows is under a quarter");
        }
    }

    /// **There is no floor**, and its absence is the change. The only size
    /// that gets no pyramid is the one a single range request already answers,
    /// which is not a policy but the definition of the coarsest level. The
    /// floor in tiles existed to cap the window's cost, and it is what kept the
    /// conformance corpus — 300 vertices in 5 tiles — from ever having one.
    #[test]
    fn a_type_that_fits_one_tile_gets_no_pyramid() {
        assert!(VertexLevels::planned(DEFAULT_CHUNK_SIZE, DEFAULT_CHUNK_SIZE).is_none());
        assert!(VertexLevels::planned(DEFAULT_CHUNK_SIZE + 1, DEFAULT_CHUNK_SIZE).is_some());
        // The conformance corpus, which the old floor put three orders out of
        // reach: 300 rows at 64 to a tile is levels 1 and 2.
        let small = VertexLevels::planned(300, 64).expect("over one tile");
        assert_eq!(small.levels, vec![1, 2]);
        // The walking skeleton's five `Person` vertices fit a tile many times
        // over, and a corpus that gets no pyramid is not one that lost it.
        assert!(VertexLevels::planned(5, DEFAULT_CHUNK_SIZE).is_none());
        assert!(VertexLevels::planned(1_000_000, 0).is_none());
    }

    /// `k+1` is a strict subset of `k`, which is what makes zooming in ADD
    /// rather than replace. Asserted over the predicate itself, because the
    /// predicate is the definition and the files are the optimisation.
    #[test]
    fn levels_nest_by_construction() {
        for k in 0..6u32 {
            let coarse: Vec<u64> = (0..4_096u64)
                .filter(|d| d % VertexLevels::stride(k + 1) == 0)
                .collect();
            let fine: Vec<u64> = (0..4_096u64)
                .filter(|d| d % VertexLevels::stride(k) == 0)
                .collect();
            assert!(coarse.iter().all(|d| fine.contains(d)), "level {k}");
            assert!(coarse.len() < fine.len() || fine.len() <= 1);
        }
    }

    /// A level set's prefix is a stem plus the number, the shape
    /// `chunk{k}.parquet` already has.
    #[test]
    fn a_level_prefix_is_the_stem_and_the_number() {
        let plan = VertexLevels::planned(1_000_000, DEFAULT_CHUNK_SIZE).expect("over one tile");
        assert_eq!(plan.level_prefix(3), "l3/");
        assert_eq!(plan.prefix, LEVEL_PREFIX_STEM);
    }

    /// A type with no pyramid has one projection, and a type with one has more
    /// — there is no key that appears and disappears, because a level was never
    /// a different kind of thing from the payload beside it.
    #[test]
    fn a_type_without_levels_is_a_type_with_one_projection() {
        let info = person_vertex();
        assert_eq!(info.projections.len(), 1);
        let with = info.with_levels(&VertexLevels::planned(1_000_000, DEFAULT_CHUNK_SIZE).unwrap());
        assert_eq!(with.projections.len(), 5);
        let yaml = with.to_yaml().expect("serialise");
        let back: VertexInfo = serde_yaml_ng::from_str(&yaml).expect("round trip");
        assert_eq!(back, with);
    }

    /// **The bytes the hand-written line scanners have to read**, pinned here
    /// rather than assumed there.
    ///
    /// `apps/corpus/guards/manifest.mjs` is a line scanner and not a YAML
    /// parser, deliberately and for the reason its own header states. What it
    /// has to see is one sequence of mappings — `projections:` — each with a
    /// nested `properties:` sequence at the item's OWN indentation, which is the
    /// grammar `property_groups:` already had. Collapsing four blocks into this
    /// one is what let the scanner lose a level of nesting rather than grow one.
    ///
    /// The failure this exists to prevent is silent: a scanner that cannot see a
    /// projection reads the type as having fewer, which is indistinguishable
    /// from a corpus that wrote fewer. So the emitter is what is asserted, and a
    /// serde version that re-indents sequences turns this red instead of turning
    /// a reader blind.
    #[test]
    fn the_projections_block_is_emitted_in_the_shape_the_line_scanners_read() {
        let plan = VertexLevels::planned(1_000_000, DEFAULT_CHUNK_SIZE).unwrap();
        let yaml = person_vertex().with_levels(&plan).to_yaml().expect("yaml");
        let lines: Vec<&str> = yaml.lines().collect();
        let start = lines
            .iter()
            .position(|l| *l == "projections:")
            .expect("a projections block");
        let block: Vec<&str> = lines[start + 1..]
            .iter()
            .take_while(|l| l.starts_with("- ") || l.starts_with("  "))
            .copied()
            .collect();
        // One item per projection, each opening at column zero.
        assert_eq!(block.iter().filter(|l| l.starts_with("- ")).count(), 5);
        // The payload first, at scale one and no path, then the pyramid.
        assert!(yaml.contains("- path: ''\n  scale: 1\n"), "{yaml}");
        assert!(yaml.contains("- path: l4/\n  scale: 256\n"), "{yaml}");
        // And the nested sequence sits at the item's own indentation — two
        // spaces, not four, which is the whole of what the scanner assumes.
        assert!(yaml.contains("  properties:\n  - name: id\n"), "{yaml}");
    }
}
