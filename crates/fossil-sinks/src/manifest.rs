//! The corpus manifest structs + `serde_yaml_ng` emission.
//!
//! **The field vocabulary is `GraphAr` v1.0.0's. The conformance is not.** These structs
//! serialize to the `GraphAr` vertex-info / edge-info / graph-info field names — `version:
//! gar/v1`, `type`, `chunk_size`, `prefix`, `property_groups`, and edge
//! `src_type`/`dst_type`/`adj_lists` — because a borrowed word is one fewer word a reader has to
//! learn, and because the hand-templated spelling this superseded (`graphar_version: 1.0.0`,
//! `vertex_types:`, `data_type: string`) was neither `GraphAr`'s nor anyone else's. **A fossil
//! corpus is not a valid `GraphAr` corpus, and this module used to end that sentence the other
//! way.** That is a description and not a policy: divergence 1 below is a `data_type` spelling
//! the reference C++ throws on, and it is on the first property of the first vertex type.
//!
//! Eight divergences, each read out of `docs/specification/format.md` in
//! `apache/incubator-graphar` and out of `cpp/src/graphar/` — not out of a summary of either.
//! **Nothing here was executed**: `GraphAr` was not built and no corpus was handed to it. Each of
//! these is what two sources say, and the fourth column of the honesty is saying which.
//!
//! 1. **The `data_type` spellings are not all `GraphAr`'s, and one of them stops its reader
//!    dead.** `types.cc`'s `DataType::TypeNameToDataType` accepts exactly `bool`, `int32`,
//!    `int64`, `float`, `double`, `string`, `date`, `timestamp` and five `list<…>` forms, and
//!    `throw`s `"Unsupported data type"` on anything else. `fossil-df` declares `dense_id` as
//!    `uint32`, and `cluster_id` the same — so the reference reader throws on the first property
//!    of the first vertex type it opens. **It is not the primary key, and this said it was**:
//!    `is_primary` marks `subject`, whose `string` that function does accept, and the two
//!    sentences that had it the other way round were written when `fossil-df` marked the address.
//!    Nothing about this divergence turns on the flag — the throw is on the spelling, and a
//!    reader reaches it whether the property is a key or not. The specification's own type
//!    list has no unsigned integer in it either. `time` and `binary` go out the same door: `time`
//!    is in the specification's list and has no arm in the C++, and `binary` is in neither — see
//!    [`data_type_name`], whose fallback it is.
//! 2. **[`VertexInfo::vertex_count`] and [`EdgeInfo::edge_count`] are ours.** The specification
//!    mentions no count of any kind: not a YAML field, not a file. The C++ writes one anyway —
//!    `<vertex prefix>vertex_count`, `<edge prefix><adj_list prefix>vertex_count`,
//!    `…edge_count{vertex_chunk_index}` — through `FileSystem::WriteValueToFile`, a template
//!    whose generic body is `ofstream->Write(&value, sizeof(T))`. It has one full specialisation,
//!    for `std::string`, which writes `value.size()` bytes; and one explicit instantiation,
//!    `<IdType>`, with `IdType = int64_t` in `fwd.h`. The count therefore goes out as **eight raw
//!    bytes in the writing machine's byte order**, in a file the specification never names, with
//!    no declared width and no declared endianness. That is a reference implementation normative
//!    by accident, which is the thing `/docs/characteristics/corpus-contract` exists to refuse.
//!    Ours is a declared decimal integer in the YAML, with a published vector table in
//!    `apps/corpus/guards/vectors.json` and a guard that reads the rows back off the disk.
//! 3. **`chunk_size` must be a power of two here.** A tile's address is [`tile_of`], a shift, and
//!    a shift is not a division. `GraphAr`'s own example vertex-info declares `chunk_size: 100`
//!    and its prose recommends 2^18 and 2^22 as *empirical* values; nothing in it requires a
//!    power of two. `fossil-layout`'s `shift_for` refuses anything else before a byte is written.
//! 4. **[`EdgeInfo::chunk_size`] denotes something else here** — see the field. In `GraphAr` it
//!    cuts a sub-logical table into edge chunks of that many rows; here it is not a row count at
//!    all.
//! 5. **One file per tile, carrying every column.** `GraphAr` gives each property group its own
//!    path prefix, so one chunk of one vertex type is several physical files. [`PropertyGroup`]
//!    carries no prefix, fossil emits one group, and a tile is one Parquet.
//! 6. **No offset table.** `GraphAr` requires one beside an `ordered_by_source`/`ordered_by_dest`
//!    adjacency, partitioned in alignment with the vertex chunking, to record where each
//!    vertex's edges start. Fossil writes none, and nothing under an edge prefix here is an
//!    `offset/`: a tile is sorted on the endpoint that addresses it, so a vertex's neighbours are
//!    a run of equal keys and a run is found by scanning the one tile that was fetched anyway.
//! 7. **[`AdjList::prefix`] is required here and absent from the specification's example.** All
//!    three `adj_lists` entries in `format.md` carry `ordered`, `aligned_by` and `file_type` and
//!    no `prefix`, though the prose beside them says an adjList includes "the prefix of file
//!    path". Here the prefix is the one part of a tile's URL a reader cannot compute, so an entry
//!    without one addresses nothing — and the field is a plain `String` with no `serde` default,
//!    so a specification-shaped edge-info does not even deserialize. That last half is the one
//!    claim in this list that IS executed: `adj_list_without_a_prefix_is_not_an_adj_list`.
//! 8. **The tile naming is ours, because the specification names no data file.** Here it is
//!    `<prefix>chunk{k}.parquet` for a vertex tile and
//!    `<edge prefix><adj_list prefix>tile{k}.parquet` for an edge one — `by_source/` and
//!    `by_target/`, one per declared [`AdjList`]. The C++ composes
//!    `<prefix><property group prefix>chunk{k}` and
//!    `<prefix>adj_list/part{vertex chunk}/chunk{k}`: extensionless, and two levels deep on the
//!    edge side because it partitions by source chunk and then by edge chunk.
//!
//! The structs are plain serializable data — no `Box<dyn Trait>`, safe to pass through Salsa
//! queries (CLAUDE.md hard rule). `data_type` strings are derived from [`arrow_schema::DataType`]
//! via [`data_type_name`], the single authority for the spec spellings (`int64`, `string`, ...).
//!
//! **Fossil byte-writes Parquet from Rust, and no engine is left on the path.** `fossil-df`'s
//! `files.rs` is the single Arrow→Parquet encoder, shared by the native sink and the browser
//! executor; the `DuckDB` `COPY` `GraphAr` writer was retired when both the `run` and `catalog`
//! paths moved to `fossil-df`. The layout post-pass in `fossil-layout/src/layout.rs`, which
//! re-tiles into the manifest-declared `prefix`, went the same way in `1e11a91` — it reads and
//! writes Parquet through `arrow-rs` and holds no connection, and this paragraph named a
//! `materialize.rs` that commit deleted. This module declares the tiling; it does not emit bytes,
//! and `enrich_layout` is the only thing that does — **what the emitter writes is what the
//! manifest says**, asserted on the artefact by `fossil-cli/tests/conformance.rs` rather than
//! agreed by convention. That gap stood open for a long time; it does not get to reopen.

use arrow_schema::DataType;
use serde::{Deserialize, Serialize};

/// The `GraphAr` manifest format version string. Emitted as `version: gar/v1`.
pub const GRAPHAR_VERSION: &str = "gar/v1";

/// The payload file of a row-group container: one per set, its row groups the
/// tiles. `@fossil-lang/corpus` spells the same constant.
pub const TILES_FILE: &str = "tiles.parquet";

/// Which container carries a corpus's tiles — one file per tile with the address
/// in the name, or one file per set with the address as the row-group ordinal.
///
/// **A reader cannot work this out, which is the whole reason it is declared.**
/// Working it out means listing a directory, and there is no listing over HTTP.
/// So it is a field of `graph.graph.yml` and it applies to every payload set in
/// the corpus at once — the vertex tiles, the identity index, and each
/// orientation of each edge type. `/docs/format/conventions/addressing` has the
/// measurement that chose fossil's: 5.6 range requests per window against 22.3,
/// and 496,373 B of footer in one piece against 1,150,490 B in 1,221.
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
/// # Why this is a declaration and not an enforcement
///
/// A corpus is files. A recipient holding them reads every column with any
/// Parquet reader, and `/docs/format/reading/without-fossil` documents exactly
/// that as the way to look at one. **There is no chokepoint, so there is no
/// read-time privacy control to build** — and the reference systems that do
/// have one do not gate SQL either: Snowflake's dynamic data masking and row
/// access policies, `BigQuery`'s column-level ACLs and row-level security and
/// Databricks' equivalent are all applied *inside the query planner*, whatever
/// SQL the caller wrote. Withholding a verb is not a privacy mechanism; it is
/// a second door on the same room.
///
/// So the protection is total at write time: **the bytes that would violate the
/// bound are never written**, and every reader preserves the bound for free
/// because you cannot extract what is not there. What travels with the artifact
/// is this — a declaration, plus a property a stranger can re-derive from the
/// files. `apps/corpus/guards/guards.mjs`'s `declared-privacy` is that stranger.
///
/// # This is the handed-over case, and it is not the only one
///
/// A corpus that is **handed over** has no privacy beyond what it carries
/// inside, which is what this field is for. A corpus that is **served** — the
/// operator keeps the files and the only access is through their engine — has a
/// chokepoint, and a chokepoint admits mechanisms this one cannot: query-time
/// perturbation, a differential-privacy budget that runs out. That is a
/// different product with a different trust model (a trusted operator, and an
/// answer that degrades with use), and nothing here is built for it. Nothing
/// here assumes it away either: this field says what the *bytes* guarantee, and
/// a served deployment is free to guarantee more on top.
///
/// # Absence, and why it is not `Option`
///
/// [`Self::Undeclared`] is a value a producer writes, and it means "this corpus
/// carries no privacy bound". A *missing* `privacy:` key means something else
/// entirely: "written before this field existed". Those two must not collapse,
/// and neither of them may be read as "public".
///
/// This is [`VertexInfo::vertex_count`]'s argument and not
/// [`VertexInfo::index`]'s. The distinction the two of them draw is whether
/// absence leaves a question **unanswerable** or merely **slower**, and this one
/// is unanswerable: `k` is derivable from the bytes by anyone, but the
/// **quasi-identifier set is not**, at all. Whether `birth_year` is a
/// quasi-identifier is a judgement about a jurisdiction and a deployment, not a
/// property of a column, and no amount of scanning recovers it. A reader handed
/// a corpus with no declared set cannot compute the bound, cannot approximate
/// it, and cannot tell that it was not computed.
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
    /// released population. Published beside [`Self::k`] rather than instead of
    /// it because the two answer different questions — what was required, and
    /// what there is — and because a reader that recomputes has something to
    /// check the producer's own arithmetic against, not merely a threshold to
    /// clear.
    pub reached: u64,
    /// How a `NULL` in a quasi-identifier column is read. **A parameter and not
    /// a default**, because both extremes are wrong and the field that settles
    /// it says so by exposing the choice as a number rather than making it —
    /// see [`AbsentQuasiIdentifier`].
    pub absent_quasi_identifier: AbsentQuasiIdentifier,
    /// The records the bound was measured over: every row of every tile of
    /// every type carrying a quasi-identifier, summed. Equal to the sum of
    /// those types' [`VertexInfo::vertex_count`], and that equality is the
    /// **scope assertion** — the equivalence class is the whole released
    /// population, and a corpus is tiles, so a check that silently ran
    /// per-tile is the easiest wrong answer to get here. Publishing the
    /// population is what makes running it per-tile detectable by somebody
    /// else.
    pub population: u64,
    /// Records excluded from certification and charged to the budget. Non-zero
    /// only under [`AbsentQuasiIdentifier::Suppress`]; zero by construction
    /// under the other two.
    pub suppressed: u64,
    /// The suppression allowance, in **parts per million of
    /// [`Self::population`]**. The bound requires
    /// `suppressed * 1_000_000 <= population * suppression_budget_ppm`.
    ///
    /// # Why parts per million and not a fraction
    ///
    /// k-anonymity in practice is generalisation *plus* a suppression limit, so
    /// a verification with no budget in it is verifying a different property
    /// from the one the literature means. ARX's `setSuppressionLimit` is that
    /// parameter, its library default is `0`, and the sweeps its own papers run
    /// are **0% against 10%** and **0% against 100%** (Lightning, *Transactions
    /// on Data Privacy* 9(2), §6.1) — not the 0/2/4 this line first claimed,
    /// which is the `0.02d` of ARX's API tutorial page mistaken for a benchmark.
    /// The GUI's own advice is that *"the recommended value for this parameter
    /// is 100%"*, which is worth knowing before treating a small budget as the
    /// conservative choice: a low limit does not make a release safer, it makes
    /// the search fail.
    ///
    /// It is an integer here because a manifest is a text
    /// document that four independent readers parse (`serde_yaml_ng`, two line
    /// scanners in JavaScript and TypeScript, and whatever a stranger brings),
    /// and a float is the one scalar where they can disagree about the same
    /// bytes. `20000` is 2%, the comparison above is exact integer arithmetic,
    /// and `BigInt` reproduces it without rounding.
    pub suppression_budget_ppm: u64,
    /// The quasi-identifier set, as `<Type>.<column>` names separated by single
    /// spaces — `Person.birth_year Person.postcode Person.sex`.
    ///
    /// **The one field a reader cannot derive and the one the whole bound turns
    /// on.** It is a flat scalar rather than a sequence because the manifest's
    /// grammar is a flat mapping of scalars, one sequence of paths and one
    /// sequence of small mappings; a nested sequence under a nested mapping is
    /// outside what `apps/corpus/guards/manifest.mjs` and
    /// `packages/corpus/src/manifest.ts` read, and both of them **skip** what
    /// they cannot see rather than failing on it. A quasi-identifier set that
    /// silently scans as absent is the worst available outcome, so the shape is
    /// chosen to be one those scanners already read.
    pub quasi_identifiers: String,
    /// **What the writer did to reach [`Self::k`]**, per generalised column.
    ///
    /// `none` when the policy declared no hierarchy and the quasi-identifiers
    /// were published as the program produced them — which is what every corpus
    /// written before the writer could derive anything says. Otherwise a
    /// space-separated token per generalised column, in the same
    /// `<Type>.<column>` spelling and the same flat-scalar grammar as
    /// [`Self::quasi_identifiers`], for the same line-scanner reason:
    ///
    /// ```text
    /// generalization: Person.birthYear@bucket Person.postcode@1-3/4
    /// ```
    ///
    /// `@bucket` is a numeric column published as its enclosing **declared**
    /// bucket. `@<coarsest>-<finest>/<declared>` is a levelled column — prefix
    /// or date — with the coarsest and finest hierarchy levels any published
    /// class sits at, over the levels the hierarchy declares. `finest ==
    /// declared` means the hierarchy ran out before `k` did.
    ///
    /// # Why this is worth writing when so much else was left out
    ///
    /// The rule the rest of this struct follows is that a fact a reader cannot
    /// check is not worth writing, and this field is the closest call in it.
    ///
    /// It earns its place because **`reached` means two different things
    /// without it.** A `reached: 20` over raw postcodes and a `reached: 20`
    /// over postcodes truncated to three characters are not the same release —
    /// the second traded resolution the first still has — and a recipient
    /// recomputing `k` from the Parquet gets 20 either way and cannot tell them
    /// apart. The one number the manifest exists to publish is ambiguous
    /// without this one beside it.
    ///
    /// And the levels **are** checkable, with one honest caveat. Every
    /// published quasi-identifier cell is a node of a declared hierarchy — the
    /// writer refuses to publish an observed range, precisely so that this
    /// stays true — so a reader can read a column, decide which level each cell
    /// sits at, and recompute the pair. What they need that the corpus does not
    /// carry is the hierarchy itself, which lives in the policy document that
    /// [`Self::policy`] names and does not locate. That is a weaker position
    /// than [`Self::quasi_identifiers`], which is self-contained, and it is
    /// stated rather than papered over: the field is checkable by the recipient
    /// who has the policy, and the recipient who has the policy is the one the
    /// field is for.
    ///
    /// # What it does not claim
    ///
    /// Not that the source data was any finer. A column already coarse in the
    /// source generalises to the same cells, and nothing in the released bytes
    /// can distinguish that — which is fine, because the claim here is about
    /// what a recipient holds, not about what a producer started from.
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
    /// The field `GraphAr` does not have, and the one thing a manifest could not
    /// say. Without it a reader has no way to know the corpus stops early: tiles
    /// are addressed and never listed, so a tree holding `chunk0..chunk16` is
    /// indistinguishable from a corpus that has seventeen tiles. A hole in the
    /// middle is caught by the addressing — tile `k` is not where tile `k+1`
    /// says it is — and **a missing tail is caught by nothing**, which is why
    /// `apps/corpus/conformance/writer.mjs` had to close it sideways through the
    /// edge endpoints. With this, `tiles = vertex_count.div_ceil(chunk_size)`,
    /// and a reader knows how far the corpus goes before it opens a file.
    ///
    /// `u64` and not `Option<u64>`: a count that may be absent is a count a
    /// reader may not depend on, and a reader that may not depend on it is
    /// exactly the reader that cannot detect the truncation. The optional
    /// spelling reproduces the gap it was added to close. A producer that does
    /// not know its own row count is not one this format has.
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
    /// `Option`, and it is worth saying why when [`Self::vertex_count`] argues
    /// at length that an optional count reproduces the gap it closes. The two
    /// are not the same kind of field. A missing count leaves a question
    /// **unanswerable** — a reader cannot tell a truncated corpus from a
    /// complete one, and no amount of work recovers the answer. A missing index
    /// leaves the same question answerable and **slower**: `subject = ?` over
    /// every tile returns exactly the row the index would have found. So its
    /// absence is a cost a reader can measure and report, which is what
    /// `openCorpus` does, rather than a fact it cannot obtain.
    ///
    /// Every corpus written before this field existed has none, and each of
    /// them stays readable. That is the other half of the same argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<VertexIndex>,
    /// `GraphAr` format version — always [`GRAPHAR_VERSION`] (`gar/v1`).
    pub version: String,
}

/// Where a vertex type's identity index lives, and what it is ordered by.
///
/// # Why a tiled sibling rather than one file
///
/// The payload is `vertex/<Type>/chunk{k}.parquet` in Morton order, because the
/// spatial order IS the id space and that is what makes a window a range. An
/// index has to be in a different order — by identity — so it cannot be a
/// column of that table: one table has one sort, and adding `subject_hash` as a
/// second key would break the order the window depends on.
///
/// So it is a second table, and it is **tiled like the first one**, with the
/// same `chunk_size` and the same `tile{k}` spelling under a prefix of its own.
/// A single file would be simpler to write and to read, and it is the container
/// this format refuses by name everywhere else: at five million vertices, cut at
/// the default `chunk_size`, the payload is **1,221** addressable objects and a
/// single-file index would be one of ~60 MB beside them. A reader that already
/// knows how to seek a tile needs nothing new to seek this.
///
/// **Both of those numbers were wrong when this paragraph was written**, and the
/// way they were wrong is worth keeping. It said *"a hundred and twenty-two"*
/// where `5,000,000 / 4,096` is 1,221 — a factor of ten, transcribed and never
/// divided. And it said *"one ~40 MB object"*, which is `subject` at 8.016
/// compressed bytes per row and nothing else: that is the size of the **scan**
/// this index exists to avoid, not of the index, which carries `subject` **and**
/// `dense_id` and is half again as large. A figure borrowed from the cost you are
/// arguing against is the easiest one to get wrong, because it is sitting right
/// there in the sentence above it.
///
/// # What a reader does with it
///
/// The tiles are ordered by [`Self::ordered_by`], so the footer statistics of
/// each one carry a disjoint `min`/`max` range of that column and a binary
/// search over the footers names the one tile that can hold a given value —
/// which is exactly what the payload's own footers cannot do for `subject`,
/// because Morton order and lexicographic order have nothing to do with each
/// other and every tile's range overlaps every other's.
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
    /// The sibling of [`VertexInfo::vertex_count`], and the same argument for
    /// being a required `u64`. It closes the same hole on the adjacency side: an
    /// edge tile with no rows is not written, so a reader that finds nothing at
    /// `tile{k}.parquet` learns "this tile has no edges" and cannot tell that
    /// from "this tile was never uploaded". Summing the tiles it did find
    /// against this number is what tells it apart.
    ///
    /// Not a tile count for the second time in this struct: [`Self::chunk_size`]
    /// is not a row count and this is not a size.
    pub edge_count: u64,
    /// The addressing unit of an edge tile, equal to [`Self::src_chunk_size`].
    ///
    /// **Not a row count**, and it never was one for edges: an edge lives in the
    /// tile of the endpoint its file is ordered by, so tile `k` under
    /// `<prefix>by_source/` holds every edge whose `src_dense` is in vertex tile
    /// `k` and its row count is the total degree of those 4,096 vertices. The
    /// alternative — the deepest tile containing both endpoints — was measured
    /// and is dominated on both curves: 2.29× → 15.86× over-read against CSR's
    /// flat 1.95× → 2.89×, and 2.5–3.5× the tiles.
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
    /// Which container carries every payload set of this corpus. See
    /// [`Container`]: it is here, once, because it is the one thing about a
    /// tile's URL a reader is told rather than derives, and because two payload
    /// sets in different containers would be two addressing schemes in one
    /// corpus.
    #[serde(default)]
    pub container: Container,
    /// The privacy bound this corpus declares. See [`Privacy`]: it is here,
    /// once, beside [`Self::container`], because the bound is a property of the
    /// **whole release** and not of a column — and because
    /// `packages/corpus/src/corpus.ts` deliberately does not read
    /// `property_groups`, taking the payload vocabulary from the bytes with one
    /// `DESCRIBE` per type. A field the reference reader never opens is not a
    /// policy.
    ///
    /// `#[serde(default)]` so that a corpus written before this field existed
    /// still deserialises, and [`Privacy::Undeclared`] so that what it
    /// deserialises to is "unknown" rather than "public".
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
    /// against `dst_dense` and [`EdgeInfo::dst_chunk_size`]. On a cross-type edge
    /// those are two different `dense_id` spaces, which is why the two sizes are
    /// declared separately rather than being one number that happens to agree.
    pub aligned_by: String,
    /// Where this orientation's tiles are, relative to [`EdgeInfo::prefix`] and
    /// with the trailing separator — `"by_source/"`, `"by_target/"`.
    ///
    /// Declared rather than conventional because it is the one thing a reader
    /// cannot compute: `aligned_by` gives it the arithmetic and the tile number,
    /// and this gives it the URL. `<edge prefix><adj prefix>tile{k}.parquet` is a
    /// complete address, and a reader that has the manifest has never needed to
    /// list a directory. A tile with no rows is not written, so a 404 is the
    /// answer "this vertex has no edges in this direction" and costs nothing to
    /// give.
    pub prefix: String,
    /// Storage file type, e.g. `"parquet"`.
    pub file_type: String,
}

/// How many bits a `dense_id` is shifted right by to name the tile holding it.
///
/// Published as arithmetic and not as prose, because that is the difference
/// between an implementation somebody can copy and one they have to re-derive
/// (Iceberg publishes Murmur3 with a
/// vector table and every port agrees; `PMTiles` links Wikipedia for its Hilbert
/// curve and every port differs).
///
/// **The operands, spelled out.** The input is an unsigned 64-bit `dense_id`,
/// the shift is logical, and the result is an unsigned 64-bit tile number. Not
/// pedantry: `>>` is arithmetic on a signed type in Rust, and in JavaScript it
/// truncates to 32 bits before shifting, so the same three characters mean
/// three different things across the layers that have to agree. The border
/// vectors a re-implementation is checked against are in this module's tests.
///
/// **Sixty-four here and thirty-two in the column, deliberately.** A vertex
/// manifest declares `dense_id` as `uint32`, so the stored ceiling is
/// 4,294,967,295 per vertex type — and the arithmetic every reader implements is
/// wider than the column on purpose, so that widening the column later moves no
/// reader. The published vectors carry 2³¹, 2³² − 1 and 2⁵³ for that reason:
/// they are checked past a ceiling the storage has not reached. Which ceiling
/// binds, and why it is not this one, is on `/docs/format/conventions/identity`.
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
/// **A tile is a fixed 4,096-row `dense_id` range**, and both lines of reasoning
/// that reach that number arrived independently.
/// Measured on the five-million corpus served over a plain HTTP origin, counting
/// every request that answers, against an ideal payload of 0.6–0.9 MB per window
/// that is flat in N:
///
/// | `chunk_size` | requests | bytes | vs ideal |
/// |---|---|---|---|
/// | 1,024 | 178 | 1.11 MB | 1.73× |
/// | **4,096** | **78** | **1.48 MB** | **2.31×** |
/// | 8,192 | 47 | 1.89 MB | 2.95× |
/// | 32,768 | 25 | 4.61 MB | 7.20× |
/// | 122,880 | 35 | 11.73 MB | 18.3× |
///
/// The two curves have no common optimum — bytes bottom out at 1,024–2,048 and
/// requests fall monotonically — so what chooses is `λ·β`, the bytes a link
/// moves in the latency of one request: 32,768 in series, 8,192 with six in
/// flight, **4,096 fully multiplexed**, which is what an addressed reader is.
///
/// **And the byte curve is flat from 1,024 to 8,192, so the emitter is not tuned
/// inside that band.** A change there is not an improvement, it is noise with a
/// `git blame` on it.
///
/// It was 122,880 — `DuckDB`'s default `ROW_GROUP_SIZE`, chosen when a chunk was
/// thought to be a row group and measured only in milliseconds over localhost,
/// where a request costs nothing. It is **Pareto-dominated by 32,768 on both
/// curves at once** (more requests *and* four times the bytes, because a
/// 122,880-row edge tile crosses several row groups and costs 9.4 requests), and
/// it is not a power of two, which forces a division where a shift does.
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
            version: GRAPHAR_VERSION.to_string(),
        }
    }

    /// Declare that this type carries an identity index. See [`VertexIndex`].
    #[must_use]
    pub fn with_index(mut self, index: VertexIndex) -> Self {
        self.index = Some(index);
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
/// hand-rolled. Unhandled arrow types fall back to `binary`, which is **fossil's
/// fallback and not a `GraphAr` spelling**: `binary` is in neither the
/// specification's type list nor `types.cc`, and `TypeNameToDataType` throws
/// `"Unsupported data type"` on it. This sentence used to call it "the `GraphAr`
/// catch-all for opaque columns", attributing to `GraphAr` a spelling `GraphAr`
/// rejects. `time` is the same shape of problem from the other side — it is in
/// the specification's list and has no arm in the C++ — and both are named in
/// this module's header beside the `uint32` that is the real one.
///
/// **This said the declared types "must match what `DuckDB` COPY actually
/// writes", and both halves of that were wrong.** `DuckDB` COPY is not the
/// writer any more for a payload — the module header above says so: `fossil-df`'s
/// `files.rs` encodes Arrow→Parquet, and COPY survives only in the layout
/// post-pass. And "must match" was a `must` nothing enforces.
///
/// **Half of that gap is closed, and the half that is closed is the half the
/// addressing rests on.** `apps/corpus/guards/guards.mjs`'s
/// `addressing-is-unsigned` opens the payload and requires `dense_id`,
/// `src_dense` and `dst_dense` to hold an unsigned integer, because those are
/// the three names every convention *shifts* — a `dense_id` stored as a string
/// opens, describes and addresses until a reader shifts it, and one stored
/// signed shifts arithmetically instead of logically and reaches a tile that
/// does not exist. What is still open is the *rest* of the property list: the
/// checker reads the manifest with a line scanner whose grammar is a flat
/// mapping and one sequence of small mappings, and a `property_groups` entry's
/// properties are one level deeper than that goes.
///
/// Neither half could be closed here. This crate declares the tiling and emits
/// no bytes, and it holds `arrow-schema` ALONE on purpose (see `Cargo.toml`);
/// comparing a declaration to a file needs a Parquet reader and a written
/// artefact, so the guard belongs beside the one that already opens them.
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

    /// The border vectors of [`tile_of`], which are the deliverable.
    ///
    /// A second implementation — the wasm reader, the `TypeScript`/`DuckDB` path,
    /// or a stranger's — is checked against this table and not against a
    /// sentence. Every value here is a border: the first id, the last id of tile
    /// 0, the first of tile 1, and the four places where a 32-bit reading of the
    /// shift diverges from a 64-bit one. `2^31`, `2^32 − 1` and `2^53` exceed the
    /// `u32` a `dense_id` column holds *today*, and they are here precisely for
    /// that reason: they are where a port that took the shift as signed, or that
    /// ran it through a JavaScript `number`, gives a different answer.
    ///
    /// `2^32 − 1` is the largest id the declared `uint32` can carry, and it is a
    /// vector rather than a sentence because the arithmetic is 64-bit and the
    /// column is not — so the day the column widens, every reader already agrees
    /// about the values past this one and none of them has to be moved.
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

    /// How many tiles a declared count implies, at the borders that matter.
    ///
    /// The same table as the `declared_count` section of
    /// `apps/corpus/guards/vectors.json`, which is the published half — this is
    /// the Rust half executing it. The two borders worth the name are a count
    /// that exactly fills a tile (one tile, not two) and a count one row over
    /// (two, the second holding one row), because the tail tile is the one a
    /// truncated corpus loses and the one no other check can see.
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

    /// The count is required, and a manifest without one does not deserialize.
    ///
    /// This is the `u64`-not-`Option<u64>` decision as a test. An optional count
    /// is a count a reader may skip, and a reader that skips it is the reader
    /// that cannot tell a truncated corpus from a whole one — the exact hole the
    /// field was added to close. If this test is ever made to pass by adding a
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

    /// Divergence 7 in this module's header, executed rather than asserted.
    ///
    /// `format.md`'s edge-info example gives all three of its `adj_lists`
    /// entries as `ordered` + `aligned_by` + `file_type`, with no `prefix`. Here
    /// the prefix is the one part of a tile's URL a reader cannot compute, so
    /// the field has no `serde` default and a specification-shaped edge-info
    /// does not deserialize. Everything else in that list is what two documents
    /// say; this is what this crate does.
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
        // Field-name guard: spec spellings present, NOT the old hand-rolled template.
        assert!(yaml.contains("version: gar/v1"), "{yaml}");
        assert!(yaml.contains("type: Person"), "{yaml}");
        // The count, and its position: it sits before the tiling, because
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
        // The old hand-rolled spelling must NOT appear.
        assert!(!yaml.contains("graphar_version"), "{yaml}");
        assert!(!yaml.contains("vertex_types"), "{yaml}");
    }

    #[test]
    fn vertex_yaml_skips_none_nullable_but_keeps_some() {
        let yaml = person_vertex().to_yaml().expect("vertex serialization");
        // id has Some(false) → emitted; name has None → skipped.
        assert!(yaml.contains("is_nullable: false"), "{yaml}");
        // Exactly one occurrence (only id), proving skip_serializing_if works for name.
        assert_eq!(yaml.matches("is_nullable").count(), 1, "{yaml}");
    }

    #[test]
    fn vertex_info_round_trips_through_yaml() {
        // The query side (fossil-graph) deserialises the same structs the
        // writer serialises — single source, no parallel reader structs.
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
        // Both orientations, each saying where its tiles are — the two lines a
        // reader needs to turn a `dense_id` into the URL of its edges in one
        // direction. A reader that has only `aligned_by` knows the arithmetic and
        // not the address.
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
        // Mandatory on write. A producer that has no bound says so, because the
        // reader has to be able to tell "no bound" from "written before the
        // field existed" — and neither of them from "public".
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
        // Ignorable on read. This is the `graph.graph.yml` of every corpus
        // written before this field, byte for byte, and it must deserialise —
        // to `Undeclared`, which is «unknown», not to a bound and not to
        // «public».
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

    /// The manifest is read by four things and only one of them is `serde`.
    ///
    /// `apps/corpus/guards/manifest.mjs` and `packages/corpus/src/manifest.ts` are
    /// line scanners over «a flat mapping of scalars, one sequence of paths and
    /// one sequence of small mappings», and what they do with a shape outside
    /// that grammar is **skip it silently**. So the emitted `privacy:` block has
    /// to stay inside it, and this is the test that says so in the crate that
    /// emits it rather than in the two that read it.
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
        // And it bites: 1,401 of 70,000 is 20,014 ppm, which is over. Read off a
        // value rather than written as a constant expression, which the
        // compiler folds away and clippy rightly calls an assertion about
        // nothing.
        let over = KAnonymity {
            suppressed: 1_401,
            ..bound
        };
        assert!(over.suppressed * 1_000_000 > over.population * over.suppression_budget_ppm);
    }
}
