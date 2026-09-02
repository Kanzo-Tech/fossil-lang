//! The corpus manifest structs + `serde_yaml_ng` emission.
//!
//! **The field vocabulary is `GraphAr` v1.0.0's. The conformance is not.** A
//! fossil corpus is not a valid `GraphAr` corpus: `fossil-df` declares
//! `dense_id` as `uint32`, and the reference C++ reader throws on it at the
//! first property of the first vertex type. `/docs/design/corpus` has the eight
//! divergences, what each was read out of, and what would reverse them.
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
use serde::{Deserialize, Serialize};

/// The `GraphAr` manifest format version string. Emitted as `version: gar/v1`.
pub const GRAPHAR_VERSION: &str = "gar/v1";

/// The payload file of a row-group container: one per set, its row groups the
/// tiles. `@fossil-lang/corpus` spells the same constant.
pub const TILES_FILE: &str = "tiles.parquet";

/// The **tile-code anchor** of one vertex type, under its own
/// [`VertexInfo::prefix`]. See [`VertexCodes`] for what is in it and why it is
/// beside the tiles rather than inside the manifest.
pub const TILE_CODES_FILE: &str = "codes.json";

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
    /// Property groups (column groupings → one file per group per chunk).
    pub property_groups: Vec<PropertyGroup>,
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
    /// **Where the Morton code of each tile's first and last row is published**
    /// — the one input the arithmetic address cannot derive. See [`VertexCodes`].
    ///
    /// `Option` for [`Self::index`]'s reason: a reader with a Parquet reader
    /// gets the same tiles out of the footers. The reader this field exists for
    /// is the one that has no Parquet reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<VertexCodes>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// Where a vertex type's identity index lives, and what it is ordered by.
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

/// Where a vertex type's **tile-code anchor** lives: the first and last Morton
/// code of every tile, plus the extent those codes were quantised against.
///
/// `dense_id` is a vertex's RANK by code and not its code, so arithmetic says
/// which tiles exist and cannot say which ones a window intersects; assuming
/// the ranks are uniform draws a wrong picture rather than a slow one.
///
/// A **path** here and not the numbers, for three reasons and the first is not
/// size: the manifest is the plan and the codes are an outcome of the layout
/// pass; there are two per tile, so inlining them makes a constant-size
/// document grow with the corpus, charged to every reader; and the point of an
/// arithmetic address is that a camera needs no Parquet reader, which a
/// Parquet-borne anchor puts back. `/docs/format/conventions/addressing` has the
/// measurements, and why JSON rather than a packed array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexCodes {
    /// Path to the anchor document, relative to the vertex type's own
    /// [`VertexInfo::prefix`] — [`TILE_CODES_FILE`] as the writer spells it.
    pub path: String,
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
    /// Adjacency-list orderings provided for this edge.
    pub adj_lists: Vec<AdjList>,
    /// Property groups carried on the edge.
    pub property_groups: Vec<PropertyGroup>,
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
    /// bytes rather than from `property_groups`: a field the reader never opens
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

/// A group of properties stored together in one file type per chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyGroup {
    /// Storage file type for this group, e.g. `"parquet"`.
    pub file_type: String,
    /// The properties (columns) in this group.
    pub properties: Vec<Property>,
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
}

/// An adjacency-list ordering descriptor for an edge.
///
/// The two orientations of one edge relation are two of these, and between
/// [`Self::aligned_by`] and [`Self::prefix`] they are **the whole of a reader's
/// hop**: `aligned_by` names the endpoint column that addresses the tiles, and
/// `prefix` says where they are. Nothing else has to be agreed out of band.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjList {
    /// Whether the adjacency list is sorted.
    pub ordered: bool,
    /// Which endpoint the list is aligned by, `"src"` or `"dst"`.
    ///
    /// Also which endpoint *addresses* it: `"src"` means tile `k` under
    /// [`Self::prefix`] holds the rows whose `src_dense >> shift` is `k`, with
    /// the shift taken from [`EdgeInfo::src_chunk_size`]; `"dst"` the same
    /// against `dst_dense` and [`EdgeInfo::dst_chunk_size`].
    pub aligned_by: String,
    /// Where this orientation's tiles are, relative to [`EdgeInfo::prefix`] and
    /// with the trailing separator — `"by_source/"`, `"by_target/"`.
    ///
    /// Declared rather than conventional because it is the one thing a reader
    /// cannot compute: `aligned_by` gives it the arithmetic and this gives it
    /// the URL, so `<edge prefix><adj prefix>tile{k}.parquet` is a complete
    /// address and no directory is ever listed.
    pub prefix: String,
    /// Storage file type, e.g. `"parquet"`.
    pub file_type: String,
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
        property_groups: Vec<PropertyGroup>,
    ) -> Self {
        Self {
            vertex_type: vertex_type.into(),
            iri: String::new(),
            vertex_count,
            chunk_size,
            prefix: prefix.into(),
            property_groups,
            // No index by default, and that is not a stub: writing one is a
            // second pass over the rows in a different order, which the caller
            // that HAS those rows decides to pay. `with_index` is how it says so.
            index: None,
            // Nor a code anchor: it is an outcome of the layout pass and the
            // caller that runs one declares it. See [`VertexCodes`].
            codes: None,
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// Declare that this type carries an identity index. See [`VertexIndex`].
    #[must_use]
    pub fn with_index(mut self, index: VertexIndex) -> Self {
        self.index = Some(index);
        self
    }

    /// Declare where this type's tile-code anchor is written. See [`VertexCodes`].
    #[must_use]
    pub fn with_codes(mut self, codes: VertexCodes) -> Self {
        self.codes = Some(codes);
        self
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
            vec![PropertyGroup {
                file_type: "parquet".to_string(),
                properties: vec![
                    Property {
                        name: "id".to_string(),
                        data_type: data_type_name(&DataType::Int64),
                        is_primary: true,
                        is_nullable: Some(false),
                    },
                    Property {
                        name: "name".to_string(),
                        data_type: data_type_name(&DataType::Utf8),
                        is_primary: false,
                        is_nullable: None,
                    },
                ],
            }],
        )
    }

    fn knows_edge() -> EdgeInfo {
        EdgeInfo {
            src_type: "Person".to_string(),
            edge_type: "knows".to_string(),
            iri: String::new(),
            dst_type: "Person".to_string(),
            edge_count: 19_998,
            chunk_size: DEFAULT_CHUNK_SIZE,
            src_chunk_size: DEFAULT_CHUNK_SIZE,
            dst_chunk_size: DEFAULT_CHUNK_SIZE,
            directed: true,
            prefix: "edge/person_knows_person/".to_string(),
            adj_lists: vec![
                AdjList {
                    ordered: true,
                    aligned_by: "src".to_string(),
                    prefix: "by_source/".to_string(),
                    file_type: "parquet".to_string(),
                },
                AdjList {
                    ordered: true,
                    aligned_by: "dst".to_string(),
                    prefix: "by_target/".to_string(),
                    file_type: "parquet".to_string(),
                },
            ],
            property_groups: vec![],
            version: GRAPHAR_VERSION.to_string(),
        }
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

    /// The `adj_lists` divergence, executed rather than asserted: `GraphAr`'s
    /// own edge-info example carries no `prefix` on any entry, and the prefix is
    /// the one part of a tile's URL a reader cannot compute — so the field has
    /// no `serde` default and a specification-shaped edge-info does not
    /// deserialize.
    #[test]
    fn adj_list_without_a_prefix_is_not_an_adj_list() {
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
adj_lists:
- ordered: true
  aligned_by: src
  file_type: parquet
property_groups: []
version: gar/v1
";
        let error = serde_yaml_ng::from_str::<EdgeInfo>(spec_shaped)
            .expect_err("an adj_list with no prefix has no address and must not deserialize");
        assert!(error.to_string().contains("prefix"), "{error}");
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
        assert!(yaml.contains("property_groups:"), "{yaml}");
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

    #[test]
    fn edge_yaml_carries_graphar_v1_field_names() {
        let yaml = knows_edge().to_yaml().expect("edge serialization");
        assert!(yaml.contains("src_type: Person"), "{yaml}");
        assert!(yaml.contains("dst_type: Person"), "{yaml}");
        assert!(yaml.contains("edge_type: knows"), "{yaml}");
        assert!(yaml.contains("edge_count: 19998"), "{yaml}");
        assert!(yaml.contains("adj_lists:"), "{yaml}");
        // Both orientations, each saying where its tiles are: a reader with only
        // `aligned_by` knows the arithmetic and not the address.
        assert!(yaml.contains("aligned_by: src"), "{yaml}");
        assert!(yaml.contains("prefix: by_source/"), "{yaml}");
        assert!(yaml.contains("aligned_by: dst"), "{yaml}");
        assert!(yaml.contains("prefix: by_target/"), "{yaml}");
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
}
