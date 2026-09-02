/**
 * A corpus, opened from a URL — **the door**, with the addressing underneath and invisible.
 *
 * There were three entry points over one manifest and no rule for choosing between them, and this
 * is the one door now: `createGraphClient` is the transport it dispatches through, and
 * `resolveCorpus` stays published on its own subpath because it is the arithmetic a third-party
 * reader would otherwise re-derive — synchronous, WASM-free, and provably separable.
 * `/docs/design/one-door` has what the removal settled, and why the camera grew this object rather
 * than opening a fourth beside it.
 *
 * **The object has two halves and the line between them is not a spelling.** `extent`, `window`,
 * `node` and `neighbours` compute which FILES to open and open those; `schema`, `read`, `expand`,
 * `path`, `aggregate` and `executeSql` name a relation and let the engine decide. The camera is
 * addressed, not queried — an LOD is a different relation and not a filter — and `fossil-graph`'s
 * own crate doc states the same rule from the other side: *pruning is which bytes are read, and
 * that is the tiles' job, not a verb's*. See {@link Corpus.neighbours} for the one place the two
 * halves answer questions that look identical and are not.
 *
 * `resolveCorpus` is not this. It is the addressing layer: it returns URLs and leaves the consumer
 * knowing what a tile is, which container carries one, how to ask for footers and how to join CSR
 * with CSC. That is exactly the knowledge the handover asked not to need. Here there are no tiles,
 * no `dense_id`, no Morton, no `by_source`, no prefixes and no footers in the caller's face:
 *
 * ```ts
 * const corpus = await openCorpus(url, { query, wasmUrl });
 * corpus.types                                  // what is inside
 * await corpus.window({ x, y, w, h })           // vertices + edges, and whether that is all of them
 * await corpus.node(iri)
 * await corpus.neighbours([iri], { depth: 2 })
 * await corpus.schema({ vertex_type: 'Person' })
 * await corpus.aggregate({ vertex_type: 'Person', group_by: 'age', agg: 'count', bins: 20 })
 * ```
 *
 * **The one thing a host brings is an engine.** See `./query.ts` for why the capability is a single
 * `query` callback and not a bundled Parquet decoder: this package still has zero runtime
 * dependencies, and the engine is the one the host already has.
 *
 * That sentence used to end «and DuckDB's own footer pruning is the half of a windowed read nobody
 * has to write», which is true of DuckDB and false as a reason to prefer it. Measured on a
 * million-vertex corpus over HTTP: on the windowed read both DuckDB v1.5.3 and DataFusion 54 open
 * exactly **5 of 245** row groups, and DataFusion reads **2.6× fewer bytes** doing it, because
 * DuckDB's httpfs floors every footer read at 16 KiB. The pruning is not a differentiator. What
 * this package actually buys by taking a callback is that it links no engine at all.
 *
 * **Five things a consumer used to supply out of its own head.** Four are absorbed and the fifth is
 * declared, and each is argued where it bites rather than here:
 *
 * - **How many tiles** — from the manifest. `vertex_count` and `chunk_size` are both required
 *   fields, so `ceil(count / chunk_size)` settles it in one `read_text`. Before that there was no
 *   way: HTTP gives no directory, and the written alternative was to probe with `HEAD` until a 404.
 *   A corpus that declares no count is the one this refuses to open.
 * - **The payload vocabulary** — from the bytes, with one `DESCRIBE` per vertex type, and
 *   deliberately not from the manifest's `property_groups`. See {@link openCorpus} for the count
 *   that decided it.
 * - **The `x`/`y` boxes** — from the Parquet footers, as {@link Corpus.extent}, which is a fifth
 *   member on a surface that names four because without it a caller holding only a URL has no
 *   coordinates to put in a window.
 * - **Corpus identity** — the subject IRI, and this was the one with no owner. See
 *   {@link Corpus.node}: what it costs, what the tree contradicts itself about, and what would
 *   change it.
 * - **Which container** — from the manifest's `container`, because a reader over HTTP has no
 *   directory to list. Both are read; neither is globbed. See {@link openCorpus}.
 *
 * And one shape of corpus it refuses rather than guesses at: **an edge label incident twice to one
 * vertex type**, which the addressing's `tilesFor` cannot name unambiguously. See {@link Corpus.window}.
 */

import {
  type AdjacencyAddress,
  CorpusManifestError,
  type Direction,
  type EdgeTiles,
  type Gap,
  GRAPH_INFO_PATH,
  levelFor as levelForBox,
  mortonTilesFor,
  parseTileCodes,
  resolveCorpus,
  strideOf,
  type CorpusAddressing,
  type TileCodesDocument,
  type VertexAddress,
} from './address.js';
import { createGraphClient, type GraphClient } from './client.js';
import type {
  AggregateParams,
  AggregateResult,
  ExecuteSqlParams,
  ExecuteSqlResult,
  ExpandParams,
  ExpandResult,
  PathParams,
  PathResult,
  ReadParams,
  ReadResult,
  SchemaParams,
  SchemaResult,
} from './generated.js';
import { initFossilGraphWasm } from './load.js';
import { join, paths, scan } from './manifest.js';
import type { QueryFn, QueryRow } from './query.js';

export { CorpusManifestError } from './address.js';
export type { Direction, Gap, GapReason } from './address.js';

/**
 * Raised when the bytes disagree with what the manifest promised, or when the corpus is shaped in a
 * way this API cannot answer. Distinct from {@link CorpusManifestError}, which is the manifest
 * failing to address itself before a single byte of payload has been read — a caller can retry one
 * of those against a different corpus and never the other.
 */
export class CorpusReadError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CorpusReadError';
  }
}

/** One column of a vertex payload, as the bytes declare it. */
export interface CorpusField {
  readonly name: string;
  /** The engine's spelling of the Parquet type — `UINTEGER`, `VARCHAR`, `FLOAT`. */
  readonly type: string;
}

/** What one vertex type is: how much of it there is, and what each row carries. */
export interface CorpusVertexType {
  readonly type: string;
  /** Rows, from the manifest's `vertex_count`. */
  readonly count: bigint;
  readonly fields: readonly CorpusField[];
  /** The column an identity is read from, or `null` when the payload carries none. */
  readonly identity: string | null;
  /** Whether the payload carries `x` and `y`, without which no box can be answered. */
  readonly geometry: boolean;
  /**
   * Whether a lookup by identity on this type is a **seek** or a **scan**.
   *
   * `true` when the corpus publishes an identity index: {@link Corpus.node} and
   * {@link Corpus.neighbours}'s seed resolution read one index tile and then the payload tiles
   * those addresses name. `false` when it does not: the same call reads the identity column of
   * every tile of the type, which at five million vertices is about 40 MB.
   *
   * It is a property of the TYPE and not of a call, because that is the shape of the fact: a
   * consumer needs to know once whether its bookmarks are cheap, not to be told again on every
   * click. Both answers are correct; only one of them is fast, and a caller with no way to tell
   * them apart discovers the difference by measuring.
   */
  readonly indexed: boolean;
}

/** What one edge type is. `count` covers both orientations: they are one relation stored twice. */
export interface CorpusEdgeType {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  readonly count: bigint | null;
  /** The orientations the corpus publishes an address for — never one it does not. */
  readonly directions: readonly Direction[];
}

/** What is inside a corpus. */
export interface CorpusTypes {
  readonly vertices: readonly CorpusVertexType[];
  readonly edges: readonly CorpusEdgeType[];
}

/** The bounding box of one vertex type's positions. */
export interface Extent {
  readonly minX: number;
  readonly maxX: number;
  readonly minY: number;
  readonly maxY: number;
}

/**
 * One vertex, **placed** — an address, a position, and every column the row carries.
 *
 * **`GraphVertex` is the other one, and neither is a lossy spelling of the other.** That is what
 * `expand` and `path` answer with: an IRI, a label, a vertex type and a hop count, generated from
 * the Rust structs and single-source with them. It carries no `x`, no `y` and no `dense_id`,
 * because a verb reads a relation and those are the tiles' columns; this carries all three,
 * because a camera reads tiles and a canvas cannot draw an identity.
 *
 * They were both called something ending in `Vertex` with a prefix that named where they came
 * from rather than what they hold, which is how one becomes a candidate for deleting the other.
 * The name says what it holds now.
 */
export interface PlacedVertex {
  readonly type: string;
  /** The subject IRI — the identity — or `null` when this payload does not carry one. */
  readonly id: string | null;
  /** The address. It is a `BigInt`, and it does not survive a re-layout. */
  readonly denseId: bigint;
  readonly x: number;
  readonly y: number;
  /** Every column that is not `dense_id`, `subject`, `x` or `y`. */
  readonly fields: QueryRow;
}

/**
 * One edge, **once**, however many orientations found it.
 *
 * The corpus stores one relation twice — `by_source` is CSR and `by_target` is CSC — so an answer
 * that reads both and concatenates returns every edge of a self-type relation twice. It draws
 * identically and counts wrongly, which is the failure mode worth naming: on the conformance corpus
 * that is 1,192 edges against the 596 the manifest declares.
 *
 * **Both endpoints are addresses and neither is an identity**, which is what an adjacency tile
 * holds: two `dense_id` columns and nothing else. They are the one address that survives into an
 * answer, and they are here because the alternative is worse — naming the far endpoint of every
 * edge means reading the tile it lives in, and a window's edges point outside the window by
 * construction. Join them against {@link PlacedVertex.denseId}, which is in the same space, and
 * never store one: {@link Corpus.node} says why.
 */
export interface PlacedEdge {
  /** `GraphEdge` is the verbs' answer: a predicate IRI and two subject IRIs. This is two addresses. */
  readonly edgeType: string;
  readonly src: bigint;
  readonly dst: bigint;
}

/** A rectangle in the corpus's own coordinates. `w` and `h` extend from `x`/`y`. */
export interface Box {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** What {@link Corpus.window} takes. */
export interface WindowParams extends Box {
  /** The vertex type, defaulting to the first the index names. */
  type?: string;
  /**
   * Which orientations to read. Defaults to **both**, which is the answer that is complete for
   * incidence — `resolveCorpus`'s `tilesFor` defaults to `['src']` instead, because that is the drawing
   * read and a drawing read pays for nothing it cannot paint. The reference API's default is the
   * honest one and the drawing path opts down to it.
   */
  directions?: readonly Direction[];
}

/**
 * An answer that knows what it is missing.
 *
 * `complete` means one thing in both members that carry it: **every edge incident to a vertex in
 * this answer is in this answer**. It is not "the answer is large" or "nothing was truncated" —
 * neither member truncates.
 *
 * Two things can make it false, and they are reported separately because they are different facts.
 * {@link Answer.gaps} names an orientation that was not read, either because the caller did not ask
 * for it or because the corpus publishes no address for it. {@link Neighbourhood.frontier} names
 * the vertices a depth bound stopped at, whose edges were never opened.
 */
export interface Answer {
  /** `true` when there are no gaps and nothing was cut off. */
  readonly complete: boolean;
  readonly gaps: readonly Gap[];
}

/** What a window read. */
export interface WindowAnswer extends Answer {
  readonly type: string;
  readonly box: Box;
  /** The tiles the answer's vertices turned out to live in. Reported, never asked for. */
  readonly tiles: readonly bigint[];
  readonly vertices: readonly PlacedVertex[];
  readonly edges: readonly PlacedEdge[];
}

/**
 * One level of the pyramid — Zarr's `multiscales` entry, spelled for this corpus.
 *
 * A reader asks the store which levels exist and then decides; it does not hand a budget over and
 * hope. {@link Corpus.levels} is the list, {@link Corpus.levelFor} is the arithmetic that turns a
 * budget into one of these, and {@link Corpus.view} answers for the one it is given.
 */
export interface LevelInfo {
  /** *k*. Level 0 is every vertex. */
  readonly level: number;
  /** `2^k` — one `dense_id` in this many survives. */
  readonly stride: number;
  /** How many vertices the whole type has at this level: `ceil(count / stride)`. */
  readonly count: number;
  /**
   * Whether a written `vertex/<Type>/l{k}/` answers this level, or the full tiles are strided.
   *
   * **The two select the same rows** — `dense_id % 2^k == 0` is the definition and a level file is
   * a cache of it. What a written level changes is the byte count; see {@link ViewCost.read} and
   * `/docs/design/one-door` for why that is a cost and not a second contract.
   */
  readonly written: boolean;
}

/** What {@link Corpus.levelFor} takes: a rectangle and what it may cost. */
export interface LevelBudget extends Box {
  type?: string;
  /** How many vertices the caller is willing to be handed. */
  budget: number;
}

/** What {@link Corpus.view} takes — a rectangle, a level, and nothing about the session. */
export interface ViewParams extends Box {
  /** The vertex type, defaulting to the first the index names. */
  type?: string;
  /**
   * Which level of detail. **The caller's, always** — see {@link Corpus.levelFor} for why this is
   * not derived here.
   */
  level: number;
  /** Which column carries the categorical the caller will colour by. Defaults to `cluster_id`. */
  fill?: string;
  /**
   * Addresses that ride whatever the rectangle and the level select — a pinned vertex.
   *
   * They are exempt from the level, because a pin is one `dense_id` and an odd one is a multiple of
   * no stride above 1. Their tiles are counted on {@link ViewCost.tiles}, so a pin's fetch is on the
   * ledger rather than hidden in it.
   */
  pinned?: readonly (number | bigint)[];
  /** Whether to answer with the edges among the drawn set. Defaults to `true`. */
  links?: boolean;
  /**
   * The shortest edge worth a row, **in the corpus's own units**.
   *
   * A pixel floor is the caller's, and it converts: a renderer with `p` corpus units per pixel and a
   * three-pixel floor passes `3p`. Nothing here knows what a pixel is.
   */
  minLinkLength?: number;
}

/** What one answer cost, in the terms a reader can check against a network tab. */
export interface ViewCost {
  /** Tiles the rectangle and the pins selected. */
  readonly tiles: number;
  /** Tiles the type has. */
  readonly ofTiles: number;
  /** Maximal runs of adjacent tiles — what those tiles cost in `Range` requests. */
  readonly runs: number;
  /** Compressed bytes those runs hold, from the Parquet footer. */
  readonly bytes: number;
  /**
   * Which artefact answered the level: `strided` means the full tiles were opened and the
   * predicate strided them — **the same rows, more bytes** — and `level` means a written `l{k}/`
   * answered. A caller watching this is watching the pyramid arrive without the contract changing;
   * `/docs/design/one-door` is why that is reported here rather than being a second call.
   */
  readonly read: 'strided' | 'level';
  /**
   * How the tiles were chosen: the published code anchor (`vertex/<Type>/codes.json`, arithmetic,
   * no Parquet reader) or the per-tile `x`/`y` boxes in the footers.
   */
  readonly addressed: 'anchor' | 'footer';
  /** Wall clock for the queries this answer issued. */
  readonly ms: number;
}

/**
 * A rectangle at a level, drawn — **and it is a function of its arguments and nothing else.**
 *
 * No cap that moves with what the window holds, no stride derived from a count, no state between
 * calls. `/docs/design/one-door` argues why that purity is the specification rather than a
 * property this happens to have, and `/docs/design/camera` measures what it buys.
 *
 * Parallel arrays rather than objects, because the consumer is a renderer that uploads them: at
 * twenty thousand marks a `PlacedVertex[]` is twenty thousand objects built to be read four fields
 * at a time and thrown away. {@link Corpus.window} is the one that answers with rows.
 */
export interface View {
  readonly type: string;
  readonly box: Box;
  readonly level: number;
  /** `2^level`. */
  readonly stride: number;
  /**
   * How many vertices the rectangle holds at **level 0** — the denominator, and the one number that
   * says whether a coarse view is a picture of the whole rectangle or a picture of part of it.
   */
  readonly matched: number;
  /**
   * `dense_id`s, marks first and then anchors. In this type's own numbering: a `dense_id` is unique
   * within one vertex type and repeats across a union of two, so a caller drawing more than one
   * type pairs these with {@link View.type} itself.
   */
  readonly denseIds: BigUint64Array;
  /** `x, y` per row, marks first and then anchors. */
  readonly positions: Float32Array;
  /** The `fill` column per row, as written. Only the first {@link View.marks} are meaningful. */
  readonly categories: Uint32Array;
  /**
   * How many of the rows are **drawn**. A prefix length rather than a count: the rows past it are
   * anchors — far ends the links need, at their real positions, out of tiles already opened.
   */
  readonly marks: number;
  /** Pairs of row indices into {@link View.positions}. At least one end of each is a mark. */
  readonly links: Uint32Array;
  readonly cost: ViewCost;
}

/** What {@link Corpus.neighbours} took and what it reached. */
export interface Neighbourhood extends Answer {
  readonly type: string;
  readonly depth: number;
  /** The identities that resolved to a vertex, and the ones that did not. */
  readonly seeds: readonly string[];
  readonly missing: readonly string[];
  /** Every vertex reached, the seeds included. */
  readonly vertices: readonly PlacedVertex[];
  readonly edges: readonly PlacedEdge[];
  /**
   * The vertices the last hop reached and whose own edges were never opened — the boundary the
   * depth bound cut, as addresses in this answer's own `dense_id` space.
   *
   * Empty means the walk exhausted the component and {@link Answer.complete} can be `true`. It is a
   * field rather than a sentence because the alternative is an answer that looks whole: the edges
   * between two frontier vertices are missing, so a count over `edges` under-reports and nothing in
   * a `depth: 1` result says which vertices the shortfall is around.
   */
  readonly frontier: readonly bigint[];
}

/** What {@link Corpus.node} takes beyond the identity. */
export interface NodeParams {
  /** Narrow the search to one vertex type. Without it every type carrying an identity is searched. */
  type?: string;
}

/** What {@link Corpus.neighbours} takes beyond the identities. */
export interface NeighboursParams {
  /** Hops. Defaults to 1. */
  depth?: number;
  /** Which orientations to walk. Defaults to both — an undirected neighbourhood. */
  directions?: readonly Direction[];
}

/** A corpus, open. */
export interface Corpus {
  /** Where it lives, as {@link openCorpus} was given it. */
  readonly url: string;
  /**
   * What is inside: the vertex and edge types, their counts, and every column a row carries.
   *
   * **Not {@link Corpus.schema}, and the difference is which artefact each one believes.** This is
   * read once while the corpus is opening — counts from the manifest's `vertex_count`, columns
   * from one `DESCRIBE` per type over the bytes — so it is a property rather than a call, it costs
   * nothing to read again, and it cannot go stale within an open corpus. `schema()` is a verb: it
   * asks the engine, counts with `count(*)`, takes its column list from the manifest's
   * `property_groups`, and adds per-field cardinality and role for a type the call names.
   *
   * They disagree, and on the conformance corpus they disagree loudly: `property_groups` declares
   * THREE properties against seven columns on disk, so `schema()` speaks of `subject`,
   * `birth_year` and `postcode` while this reports those plus `dense_id`, `x`, `y` and
   * `cluster_id` — the four the writer puts there and the vocabulary does not name. That is not
   * one of them being wrong. A verb composes
   * SQL before it has seen a byte and can only read the declaration; this had a round trip to
   * spend and spent it on the artefact. Where the two must agree is a corpus guard's job —
   * `apps/corpus`'s `declared-count` is the one that catches a manifest lying about its rows.
   *
   * So: **this for what a row carries, `schema()` for what a field looks like.**
   */
  readonly types: CorpusTypes;
  /**
   * The addressing underneath, for a caller that has outgrown this surface — a drawing path that
   * wants tile URLs to fetch itself, for instance. Nothing here needs it.
   */
  readonly addressing: CorpusAddressing;
  /**
   * The bounding box of one vertex type's positions, or `null` when it has no geometry.
   *
   * **It comes from the Parquet footers**, which is one request per tile and then nothing: the same
   * row-group statistics that let the engine skip tiles a window misses. It is therefore *believed*
   * rather than verified — a box wider than its rows costs a read, a box narrower than its rows
   * loses vertices, and only re-reading every page tells them apart. Cached after the first call.
   *
   * It is a fifth member on a specification that names four, and it is here because without it
   * {@link Corpus.window} cannot be called: a caller with a URL and nothing else has no coordinates
   * to put in the box.
   */
  extent(type?: string): Promise<Extent | null>;
  /** The vertices in a rectangle and the edges among them. */
  window(params: WindowParams): Promise<WindowAnswer>;
  /**
   * Which levels of detail exist — the multiscale metadata, and the first half of the Zarr shape.
   *
   * Every level from 0 to the one holding a single vertex is listed, because every one of them is
   * answerable: level *k* is the predicate `dense_id % 2^k == 0`, which needs no bytes on disk.
   * {@link LevelInfo.written} is the only thing a pyramid on disk changes, and it changes a cost
   * rather than an answer.
   */
  levels(type?: string): readonly LevelInfo[];
  /**
   * The coarsest level whose population fits a budget — **pure, synchronous, and separate.**
   *
   * A second function rather than an argument to {@link Corpus.view}; `levelFor` in `./address.ts`
   * is the arithmetic, and `/docs/design/one-door` is why the level is the caller's.
   */
  levelFor(params: LevelBudget): number;
  /**
   * A rectangle at a level of detail, ready to draw — **the door a camera goes through.**
   *
   * **This and {@link Corpus.window} both take a rectangle and they are not the same question.**
   * `window` answers *what is here*: every row, every column, complete for incidence, no bound.
   * This answers *what to draw at this resolution*: a decimation, positions and one categorical,
   * bounded by the level rather than by a cap. What keeps them one contract rather than two is
   * that **`view` at level 0 selects the same vertices `window` does over the same rectangle**,
   * which `tests/view.test.ts` asserts against the fixture.
   */
  view(params: ViewParams): Promise<View>;
  /** One vertex by identity, or `null`. */
  node(id: string, params?: NodeParams): Promise<PlacedVertex | null>;
  /**
   * Everything within `depth` hops of a set of identities.
   *
   * **{@link Corpus.expand} is not this, and neither one is a spelling of the other.** They were
   * going to be merged, on the reading that a neighbourhood asked twice is a neighbourhood asked
   * twice. Three differences say otherwise, and each is the same difference:
   *
   * - **How it walks.** This turns the frontier into tile numbers by shifting the addresses it
   *   already holds, opens those adjacency tiles and no others, and does it once per hop.
   *   `expand` is a recursive CTE over the whole edge relation — the query this member exists
   *   because of, and which did not return in 45 seconds at a million vertices.
   * - **What it answers with.** Placements: an address, an `x`, a `y`, and every payload column.
   *   `expand` answers with IRIs and hop counts, which a canvas cannot draw without reading the
   *   tiles again. The corpus stores the adjacency as two `dense_id` columns and nothing else, so
   *   naming the far endpoint of every edge means opening the tile it lives in — and a
   *   neighbourhood's edges point outward by construction.
   * - **What it admits.** {@link Answer.complete}, {@link Answer.gaps} and
   *   {@link Neighbourhood.frontier}: the orientations not read and the boundary the depth bound
   *   cut. `ExpandResult` is two lists. An answer whose outermost ring is missing its own edges
   *   looks whole in a count, which is why the boundary is a field here.
   *
   * `expand` also walks the source-ordered relation only, so it is directed where this defaults to
   * both orientations. That is the smallest of the three and the easiest to mistake for the whole
   * of it.
   */
  neighbours(ids: Iterable<string>, params?: NeighboursParams): Promise<Neighbourhood>;

  // ── The verbs ─────────────────────────────────────────────────────────────────────────────
  //
  // Six methods whose SQL is written in Rust — `fossil-graph`, single-source with the native
  // runtime and with `fossil-mcp`'s server-side surface — and dispatched here through
  // `fossil-graph-wasm`. They were `createGraphClient`, a second entry point with no rule for
  // choosing between it and this one; it is the transport now, and this is the door.
  //
  // **They read a relation, where everything above reads tiles**, and that is the line between
  // the two halves of this object rather than a spelling difference. A verb's SQL names a table
  // and lets the engine decide which bytes to open; `window`, `extent` and `neighbours` compute
  // which files to open and open those. `crates/fossil-graph/src/lib.rs` states the same rule
  // from the other side — *pruning is which bytes are read, and that is the tiles' job, not a
  // verb's*.
  //
  // **What a verb sees is the manifest's vocabulary, not the payload's.** {@link openCorpus}
  // refuses to take the column list off `property_groups` and reads the bytes instead, with the
  // count that decided it; the verbs have no bytes to read at the time they compose SQL, so they
  // take the manifest at its word. On the conformance corpus that is three declared properties
  // against seven columns on disk, so `read` answers with `subject`, `birth_year` and `postcode`
  // while `types` above reports all seven. Neither is wrong and they are not the same question —
  // see {@link Corpus.types}.

  /**
   * The type lists, and per-field statistics for a named `vertex_type`.
   *
   * See {@link Corpus.types} for which of the two believes the manifest and which believes the
   * bytes; they are not the same question and they disagree on this corpus.
   *
   * **`FieldStat.role === 'identifier'` is a chart-axis heuristic and nothing else.** The merge
   * puts it one call away from {@link CorpusVertexType.identity}, which is the column a subject
   * IRI is read from and the thing a bookmark keys on, and the two are unrelated: `role` guesses
   * what a column looks like so an axis can default sensibly, and it will call an integer `id`
   * column an identifier whether or not anything identifies anything with it. It is not a
   * governance classification, it does not mark a quasi-identifier, and nothing may gate a
   * disclosure decision on it. `identity` is the one that names an identity.
   */
  schema(params?: SchemaParams): Promise<SchemaResult>;
  /**
   * Rows of one vertex type under a `where` predicate, an order and a limit.
   *
   * `where` is SQL and carries the same authority as {@link Corpus.executeSql}: a host that gates
   * one behind a permission MUST gate the other with it.
   */
  read(params: ReadParams): Promise<ReadResult>;
  /**
   * The neighbourhood of a set of vertices as IRIs — `all` walks outward up to `depth`, `into`
   * keeps only the edges whose both ends are in the set.
   *
   * **Not the same call as {@link Corpus.neighbours}**, which is why both are here. This one is a
   * recursive CTE over the whole edge relation and answers in identities; that one is tile
   * arithmetic over the adjacency and answers in placements. See {@link Corpus.neighbours}.
   */
  expand(params: ExpandParams): Promise<ExpandResult>;
  /** The shortest route between two vertices. */
  path(params: PathParams): Promise<PathResult>;
  /** One grouping, over values or — with `bins` — over equal-width ranges. */
  aggregate(params: AggregateParams): Promise<AggregateResult>;
  /** The escape hatch, for the question the other five cannot shape. */
  executeSql(params: ExecuteSqlParams): Promise<ExecuteSqlResult>;
}

/** What {@link openCorpus} takes. */
export interface OpenCorpusOptions {
  /** The host's engine. One method, and see `./query.ts` for why it is the only one. */
  query: QueryFn;
  /**
   * Where `fossil_graph_wasm_bg.wasm` is, for the verbs.
   *
   * Optional, and only the verbs need it: `types`, `extent`, `window`, `node` and `neighbours`
   * write their own SQL and load nothing. Given, it is booted on the first verb call and not at
   * open — a caller that only draws never instantiates it. Omitted, the caller is taken to have
   * called `initFossilGraphWasm` itself, which is the same memoised boot; a verb called before
   * either rejects with what the WASM says.
   */
  wasmUrl?: string | URL | Request | Response;
}

/** The four columns an answer reads by name; everything else is payload. */
const RESERVED = new Set(['dense_id', 'subject', 'x', 'y']);

/** The column the identity is read from. See {@link Corpus.node} for why it is not `dense_id`. */
const IDENTITY = 'subject';

/** A single-quoted SQL string literal. Every URL in this module reaches SQL through here. */
function lit(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

/** A DuckDB list of paths, which is how one query spans a set of tiles. */
function list(urls: readonly string[]): string {
  return `[${urls.map(lit).join(', ')}]`;
}

/**
 * Distinct, in order — what a set of tiles is a set of FILES.
 *
 * A no-op under the file-per-tile container and load-bearing under the other, where every tile of a
 * type names one file: `read_parquet` over a list scans each element, so naming it once per tile
 * would return every row once per tile.
 */
function distinct(urls: readonly string[]): string[] {
  return [...new Set(urls)];
}

/** A quoted SQL identifier. Column names come from the payload, so they are not interpolated raw. */
function ident(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}

/**
 * A key as the bytes a Parquet statistic is compared in.
 *
 * **`<` on two JavaScript strings is the wrong comparison here, and silently so.** Parquet orders a
 * string column by unsigned UTF-8 bytes, and so does DuckDB's default collation, so that is the
 * order an index tile is sorted in and the order its footer's min/max bound. JavaScript compares
 * UTF-16 code units, which agrees with UTF-8 order for everything below U+E000 and disagrees above
 * it: a surrogate pair's code units sort below the private-use area and its bytes sort above it. An
 * IRI that reached there would be pruned out of the one tile holding it and reported as absent,
 * which is the failure a lookup cannot notice. So the comparison is on bytes.
 */
const UTF8 = new TextEncoder();
function utf8(value: string): Uint8Array {
  return UTF8.encode(value);
}

/** Unsigned byte order, which is Parquet's order for a string column. */
function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i += 1) {
    if (a[i] !== b[i]) return a[i]! - b[i]!;
  }
  return a.length - b.length;
}

/**
 * Whether a tile bounded by `[lo, hi]` can hold `key` — and it answers `true` when it cannot tell.
 *
 * The upper clause is not just `key <= hi`, because a Parquet writer may TRUNCATE a long string
 * statistic rather than store it whole. parquet-rs truncates a max upward — it increments the last
 * byte, so the stored bound is still at least every value in the tile — and a min downward, which
 * makes the plain comparison safe against the writer this corpus has. A writer that truncated a max
 * WITHOUT incrementing would store a prefix of the real one, and every key extending that prefix
 * would be excluded from the only tile that holds it. The prefix test costs one comparison and
 * removes the whole class.
 */
function couldHold(key: Uint8Array, lo: Uint8Array, hi: Uint8Array): boolean {
  if (compareBytes(key, lo) < 0) return false;
  if (compareBytes(key, hi) <= 0) return true;
  return key.length > hi.length && compareBytes(key.subarray(0, hi.length), hi) === 0;
}

/**
 * A 64-bit id out of an untyped column.
 *
 * The corpus contract's second obligation is that a 64-bit id is a `BigInt` at the TypeScript
 * boundary, and it records exactly this hole: *"an id read out of a Parquet column into an untyped
 * value is outside it."* The two hosts disagree about what they hand back — DuckDB-WASM gives a
 * `UINTEGER` as a `Number` and a `UBIGINT` as a `BigInt`, and `ConnectionExecutor` turns both into
 * JSON numbers — so this is where the width is enforced instead of assumed. A `Number` that is not
 * a safe integer is refused rather than rounded, because rounding a `dense_id` addresses a
 * different vertex and nothing downstream can tell.
 */
function idOf(value: unknown, what: string): bigint {
  if (typeof value === 'bigint') return value;
  if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) return BigInt(value);
  if (typeof value === 'string' && /^\d+$/.test(value)) return BigInt(value);
  throw new CorpusReadError(
    `${what} came back as ${typeof value} ${String(value)}, which cannot hold a 64-bit id exactly`,
  );
}

function floatOf(value: unknown, what: string): number {
  const n = typeof value === 'string' ? Number(value) : value;
  if (typeof n !== 'number' || !Number.isFinite(n)) {
    throw new CorpusReadError(`${what} came back as ${String(value)}, which is not a coordinate`);
  }
  return n;
}

function text(row: QueryRow, column: string): string {
  const value = row[column];
  if (typeof value !== 'string') {
    throw new CorpusReadError(`expected ${column} to be text; got ${typeof value}`);
  }
  return value;
}

/**
 * Open a corpus from its URL.
 *
 * One argument is the corpus and the other is the engine. Everything else — which files exist, how
 * many tiles there are, what a row carries — is read from the artefact:
 *
 * 1. `graph.graph.yml` names the per-type manifests, and they are fetched with `read_text` through
 *    the same `query` the payload goes through. There is no separate `fetch` capability because
 *    there is nothing a separate one could reach that the engine cannot: it has to see the tiles.
 * 2. `vertex_count` and `chunk_size` give the tile set by arithmetic. Before those became required
 *    fields there was no way to know it — HTTP has no directory listing, and the alternative
 *    written down at the time was to probe with `HEAD` until a 404.
 * 3. One `DESCRIBE` per vertex type gives the payload vocabulary.
 *
 * **Why step 3 is not read off the manifest.** `property_groups` carries names *and* types, so this
 * looked free. It is not: on the conformance corpus the manifest declares **one** property
 * (`subject`) against **five** columns on disk (`dense_id`, `subject`, `x`, `y`, `cluster_id`), and
 * `packages/corpus`'s own test fixture declares three of which one is `dense_id`. The manifest's
 * property list is a promise; the payload is the artefact, and this reads the artefact. The cost is
 * one round trip per vertex type at open.
 *
 * **Which container, and it is read off the manifest.** A tile is a range of rows; whether it is a
 * file (`chunk{k}.parquet`) or a row group inside one file (`tiles.parquet`) is a second question,
 * and `graph.graph.yml`'s `container` is what answers it — a reader over HTTP has no directory to
 * list, so this is not something to work out. Both are read here and neither is globbed: a reader
 * that globbed would pick up a staged single-file copy beside the tiles and count every row twice.
 * The row-group one is the measured winner (5.6 requests per window against 22.3, and 496 kB of
 * footer against 1.15 MB, at five million vertices) and it is the container fossil does not write
 * yet, which is why both are read and not one.
 *
 * @throws {CorpusManifestError} when the manifest cannot address itself, or declares no row count.
 */
export async function openCorpus(url: string, options: OpenCorpusOptions): Promise<Corpus> {
  const { query } = options;
  if (typeof query !== 'function') {
    throw new TypeError('openCorpus needs a query capability: the host brings the engine');
  }

  const readText = async (relative: readonly string[]): Promise<Record<string, string>> => {
    if (relative.length === 0) return {};
    const urls = relative.map((path) => join(url, path));
    const rows = await query(`SELECT filename, content FROM read_text(${list(urls)})`);
    const byUrl = new Map(rows.map((row) => [text(row, 'filename'), text(row, 'content')]));
    const out: Record<string, string> = {};
    for (const [index, path] of relative.entries()) {
      const content = byUrl.get(urls[index]!);
      if (content === undefined) {
        throw new CorpusManifestError(`${urls[index]!} is named by the manifest and did not read`);
      }
      out[path] = content;
    }
    return out;
  };

  const manifestFiles = await readText([GRAPH_INFO_PATH]);
  const index = scan(GRAPH_INFO_PATH, manifestFiles[GRAPH_INFO_PATH]!);
  Object.assign(
    manifestFiles,
    await readText([...paths(index, 'vertices'), ...paths(index, 'edges')]),
  );

  const addressing = resolveCorpus({ manifestFiles, base: url });

  // Every vertex type's payload files, derived once. `files()` throws when the manifest declares no
  // count, which is the corpus this API cannot open and the addressing layer still can.
  const payloadFiles = new Map<string, readonly string[]>();
  const columns = new Map<string, readonly CorpusField[]>();
  for (const type of addressing.types) {
    payloadFiles.set(type.type, type.files());
    const first = payloadFiles.get(type.type)![0];
    let described: QueryRow[] = [];
    if (first !== undefined) {
      try {
        described = await query(`DESCRIBE SELECT * FROM read_parquet(${lit(first)})`);
      } catch (cause) {
        // This does not diagnose the failure — a truncated corpus reaches here too — it states what
        // was addressed and why nothing else was tried, because the tempting recovery is a glob and
        // a glob is wrong: it would pick up a staged single-file copy beside the tiles and count
        // every row twice.
        throw new CorpusReadError(
          `${first} is the first payload file of ${type.type} and it did not open. The manifest ` +
            `declares the ${addressing.container} container, so that is what was addressed, and ` +
            `this neither falls back to the other nor globs. (${(cause as Error).message})`,
        );
      }
    }
    columns.set(
      type.type,
      described.map((row) => ({
        name: text(row, 'column_name'),
        type: text(row, 'column_type'),
      })),
    );
  }

  const fieldsOf = (type: string): readonly CorpusField[] => columns.get(type) ?? [];
  const has = (type: string, column: string): boolean =>
    fieldsOf(type).some((f) => f.name === column);

  const types: CorpusTypes = {
    vertices: addressing.types.map((type) => ({
      type: type.type,
      // `files()` above threw if this were absent: a corpus whose extent is not derivable is the
      // one this API refuses to open, and it is also the only thing the manifest's count is for.
      count: type.count!,
      fields: fieldsOf(type.type),
      identity: has(type.type, IDENTITY) ? IDENTITY : null,
      geometry: has(type.type, 'x') && has(type.type, 'y'),
      indexed: type.index !== null,
    })),
    edges: addressing.edges.map((edge) => ({
      edgeType: edge.edgeType,
      srcType: edge.srcType,
      dstType: edge.dstType,
      count: edge.count,
      directions: edge.directions,
    })),
  };

  /**
   * The tile-code anchors, read **while the corpus is opening** — one small JSON per vertex type.
   *
   * It is not lazy, and the reason is {@link Corpus.levelFor}: a camera asks which level to draw
   * before it asks for anything, and a level that arrived as a promise would be a request between
   * the camera moving and a URL being computable, which is the `viewport` verb this format deleted.
   * So the anchor is bought with the manifests. It is 5,498 B at a million vertices — two orders of
   * magnitude under the footer it replaces, and it replaces it: with an anchor, which tiles a
   * rectangle touches is arithmetic and no Parquet reader is on the path.
   *
   * A type that declares no `codes:` is a legal corpus and simply has no entry here; `view` falls
   * back to the footer boxes and says so on {@link ViewCost.addressed}.
   */
  const anchors = new Map<string, TileCodesDocument>();
  {
    const declared = addressing.types.filter((type) => type.codesUrl !== null);
    if (declared.length > 0) {
      const rows = await query(
        `SELECT filename, content FROM read_text(${list(declared.map((t) => t.codesUrl!))})`,
      );
      const byUrl = new Map(rows.map((row) => [text(row, 'filename'), text(row, 'content')]));
      for (const type of declared) {
        const content = byUrl.get(type.codesUrl!);
        // A `codes:` the manifest names and the store does not carry is a corpus that lies about
        // its own addressing, and the honest response is the slower path rather than an exception:
        // the footers answer the same question, and `ViewCost.addressed` reports which one ran.
        if (content !== undefined) {
          anchors.set(type.type, parseTileCodes(content, `${type.type}'s tile-code anchor`));
        }
      }
    }
  }

  const extents = new Map<string, Extent | null>();

  const vertexOf = (type: string, row: QueryRow): PlacedVertex => {
    const fields: QueryRow = {};
    for (const [key, value] of Object.entries(row)) {
      if (!RESERVED.has(key)) fields[key] = value;
    }
    return {
      type,
      id: typeof row[IDENTITY] === 'string' ? (row[IDENTITY] as string) : null,
      denseId: idOf(row['dense_id'], `${type}.dense_id`),
      x: floatOf(row['x'] ?? 0, `${type}.x`),
      y: floatOf(row['y'] ?? 0, `${type}.y`),
      fields,
    };
  };

  /**
   * The adjacency this window reads for one edge label, or the reason it does not.
   *
   * The addressing's `tilesFor` names an orientation by edge *label*, so a corpus where two edge
   * types incident to the same vertex type share one label has an answer that cannot be attributed.
   * Refused rather than guessed: the alternative is edges filed under the wrong relation, which
   * draws correctly and counts wrongly.
   */
  const adjacencyFor = (vertexType: string, edgeType: string, direction: Direction) => {
    const candidates = addressing
      .incident(vertexType)
      .filter((edge) => edge.edgeType === edgeType && edge.adjacency(direction) !== null);
    if (candidates.length > 1) {
      throw new CorpusReadError(
        `two edge types incident to ${vertexType} are both labelled ${edgeType}, so an answer ` +
          `cannot say which relation an edge belongs to`,
      );
    }
    return candidates[0]?.adjacency(direction) ?? null;
  };

  /**
   * Where each tile is, read from the footers **once per corpus**.
   *
   * The arithmetic says which tiles exist; only the boxes say which ones a rectangle intersects,
   * and the boxes are Parquet statistics. Without them the only correct read is every tile with a
   * `WHERE`, and DuckDB prunes the rows but still opens each footer — `ceil(vertex_count /
   * chunk_size)` of them per window, which is 2,442 at ten million and made a window that answers
   * 3,152 vertices take 4.7 s. Measured from kanzo-ui, 2026-08-25: the same one-percent rectangle
   * cost 75.6 ms at a million and 1,000.8 at five, out of one tile either way.
   *
   * So it is read once and kept: a few thousand rows of metadata, no column data, and every window
   * after it opens only the tiles it names. `Corpus.extent` still scans every tile because its
   * answer is about every tile.
   */
  interface TileBox {
    readonly tile: bigint;
    readonly x0: number;
    readonly x1: number;
    readonly y0: number;
    readonly y1: number;
    /**
     * Where the tile's bytes begin in its file, and how far they run.
     *
     * Off the same footer read as the box, because it is the same row of `parquet_metadata` — and
     * needed for the same reason the box is: {@link Corpus.view} reports what an answer cost, and a
     * cost report whose byte figure is a guess is not one. `min(coalesce(dictionary_page_offset,
     * data_page_offset))` is where a row group starts — the dictionary page comes first when there
     * is one, and `file_offset` is not reliably populated by every writer.
     */
    readonly start: number;
    readonly bytes: number;
  }

  const boxes = new Map<string, Promise<readonly TileBox[]>>();

  const tileBoxes = (type: string): Promise<readonly TileBox[]> => {
    const cached = boxes.get(type);
    if (cached) return cached;
    const address = addressing.vertexType(type);
    const urls = payloadFiles.get(type)!;
    // Which tile a footer row is about. Under `files` it is the file — one per tile, and the row
    // groups inside it are one tile's worth however many there are. Under `rowgroups` it is the
    // ordinal, and this is the one place in this file where that ordinal is the address: a vertex
    // tile is exactly `chunk_size` gapless rows, so row group `k` IS tile `k`.
    const index = new Map(urls.map((url, k) => [url, BigInt(k)]));
    const perGroup = address.container === 'rowgroups';
    const loading = (async (): Promise<readonly TileBox[]> => {
      // `min_value`/`max_value`, never `min`/`max`: Parquet's original statistics fields compare
      // bytes as signed, which is meaningless for an unsigned column, so a writer that gets it
      // right leaves them empty and a reader that only knows the deprecated pair concludes the
      // footer carries no box at all. `coalesce` reads either.
      // No `WHERE path_in_schema IN ('x','y')`, and that is deliberate rather than sloppy: the box
      // still comes only from those two columns, through the CASE arms, and the two sums below want
      // EVERY column of the row group. One footer read answers both, which is what keeps this "read
      // once and kept" rather than "read twice and kept".
      const rows = await query(
        `SELECT file_name AS file, row_group_id AS rg, ` +
          `min(coalesce(dictionary_page_offset, data_page_offset)) AS start, ` +
          `sum(total_compressed_size) AS bytes, ` +
          `min(CASE WHEN path_in_schema = 'x' THEN coalesce(stats_min_value, stats_min)::DOUBLE END) AS x0, ` +
          `max(CASE WHEN path_in_schema = 'x' THEN coalesce(stats_max_value, stats_max)::DOUBLE END) AS x1, ` +
          `min(CASE WHEN path_in_schema = 'y' THEN coalesce(stats_min_value, stats_min)::DOUBLE END) AS y0, ` +
          `max(CASE WHEN path_in_schema = 'y' THEN coalesce(stats_max_value, stats_max)::DOUBLE END) AS y1 ` +
          `FROM parquet_metadata(${list(urls)}) GROUP BY 1, 2`,
      );
      const merged = new Map<bigint, TileBox>();
      for (const row of rows) {
        const tile = perGroup ? BigInt(String(row['rg'])) : index.get(String(row['file']));
        const [x0, x1, y0, y1] = ['x0', 'x1', 'y0', 'y1'].map((k) => Number(row[k]));
        const start = Number(row['start'] ?? 0);
        const bytes = Number(row['bytes'] ?? 0);
        // A tile whose name did not come back verbatim, or whose footer carries no statistics for
        // x or y, has no box — and a tile with no box is one this cannot exclude. Keeping it is
        // the conservative answer: the read stays correct and only loses the pruning.
        if (tile === undefined || ![x0, x1, y0, y1].every(Number.isFinite)) continue;
        const held = merged.get(tile);
        merged.set(
          tile,
          held === undefined
            ? { tile, x0: x0!, x1: x1!, y0: y0!, y1: y1!, start, bytes }
            : {
                tile,
                x0: Math.min(held.x0, x0!),
                x1: Math.max(held.x1, x1!),
                y0: Math.min(held.y0, y0!),
                y1: Math.max(held.y1, y1!),
                start: Math.min(held.start, start),
                bytes: held.bytes + bytes,
              },
        );
      }
      return [...merged.values()];
    })();
    boxes.set(type, loading);
    return loading;
  };

  /**
   * The tiles a rectangle can touch. Pure, and the box is half-open on the far edge exactly as
   * {@link boxOf} is, so a tile the `WHERE` would empty is never opened.
   */
  const intersecting = (all: readonly TileBox[], { x, y, w, h }: Box): readonly TileBox[] =>
    all.filter((b) => b.x1 >= x && b.x0 < x + w && b.y1 >= y && b.y0 < y + h);

  /**
   * Selected tiles collapsed into maximal runs, each a single byte interval — what a rectangle
   * costs in `Range` requests rather than in tiles.
   *
   * Two tiles join a run when they are consecutive ordinals **and their bytes actually abut**. The
   * second condition is not paranoia about the format: Parquet does not promise that row groups are
   * written back to back, and a run whose members are not contiguous names an interval containing
   * bytes belonging to nobody. Against the corpora this repo writes every boundary abuts — measured,
   * 0 gaps in 244 — so the O(√n)-runs claim holds as O(√n) requests, and it is checked rather than
   * assumed. A tile with no footer entry contributes a run of its own and no bytes.
   */
  interface TileRun {
    first: number;
    last: number;
    bytes: number;
  }

  const runsOf = (tiles: readonly number[], weights: Map<number, TileBox>): TileRun[] => {
    const runs: TileRun[] = [];
    let end: number | null = null;
    for (const tile of tiles) {
      const box = weights.get(tile);
      const open = runs[runs.length - 1];
      if (open !== undefined && box !== undefined && tile === open.last + 1 && end === box.start) {
        open.last = tile;
        open.bytes += box.bytes;
      } else {
        runs.push({ first: tile, last: tile, bytes: box?.bytes ?? 0 });
      }
      end = box === undefined ? null : box.start + box.bytes;
    }
    return runs;
  };

  /**
   * The same rectangle in the addressing layer's spelling — `{x, y, w, h}` against `{xlo, xhi,
   * ylo, yhi}`, and the far edge is **inclusive** there where {@link boxOf} is half-open here.
   *
   * The mismatch is one grid cell wide and it errs the safe way: `mortonTilesFor` may name one more
   * tile than the `WHERE` will keep a row from, which costs bytes. Erring the other way would lose
   * the vertices on the seam, which costs a wrong picture.
   */
  const gridBox = ({ x, y, w, h }: Box) => ({ xlo: x, xhi: x + w, ylo: y, yhi: y + h });

  const boxOf = ({ x, y, w, h }: Box): string =>
    `x >= ${x} AND x < ${x + w} AND y >= ${y} AND y < ${y + h}`;

  const vertexType = (name?: string): VertexAddress => addressing.vertexType(name);

  const ascending = (a: bigint, b: bigint): number => (a < b ? -1 : a > b ? 1 : 0);

  /** The vertices a set of dense ids names, read out of the tiles those ids address. */
  const readByDenseId = async (type: string, ids: readonly bigint[]): Promise<PlacedVertex[]> => {
    if (ids.length === 0) return [];
    const address = vertexType(type);
    const tiles = [...new Set(ids.map((id) => address.tileOf(id)))].sort(ascending);
    const rows = await query(
      `SELECT * FROM read_parquet(${list(distinct(tiles.map((k) => address.tileUrl(k))))}) ` +
        `WHERE dense_id IN (${ids.join(', ')})`,
    );
    return rows.map((row) => vertexOf(type, row));
  };

  /**
   * Where each INDEX tile's keys begin and end, read from its own footer **once per type.**
   *
   * The same move {@link tileBoxes} makes for `x`/`y`, for the one column an index is sorted by,
   * and for the same reason: the arithmetic says which tiles exist and only the footers say which
   * ones a value can be in. The difference is which side does the pruning. A window leaves it to
   * the engine because a box over `x`/`y` is a conjunctive range and DuckDB prunes one; a lookup
   * cannot, because the predicate is a DISJUNCTION and DuckDB prunes none of those over a VARCHAR
   * column — measured on v1.5.3 over a million-vertex corpus, `key = 'x'` prunes to one tile and
   * reads 3.98 MB while `key IN ('x','y')`, or the same spelled with `OR`, prunes to none and reads
   * all 245 index tiles, 44.1 MB. So the pruning is done here, from the same statistics the engine
   * declined to use, and the batch stays one query.
   *
   * **It cannot work under `rowgroups` and does not pretend to.** There the whole index is one file
   * and a row group has no URL, so there is nothing to address and nothing to leave out of the
   * list: `null` here means the batched query over the whole index is what there is. Same for a
   * one-file index, where pruning has nothing to remove.
   *
   * A tile whose footer carries no statistics for the key column keeps `lo`/`hi` at `null` and is
   * always read — the conservative answer, and the same call {@link tileBoxes} makes for a tile
   * with no box.
   */
  interface KeyRange {
    readonly url: string;
    /** The tile's key bounds as bytes, or `null` when its footer declared none. */
    readonly lo: Uint8Array | null;
    readonly hi: Uint8Array | null;
  }

  const keyRanges = new Map<string, Promise<readonly KeyRange[] | null>>();

  const indexRanges = (type: VertexAddress): Promise<readonly KeyRange[] | null> => {
    const cached = keyRanges.get(type.type);
    if (cached) return cached;
    const index = type.index!;
    const loading = (async (): Promise<readonly KeyRange[] | null> => {
      if (index.container === 'rowgroups') return null;
      const urls = distinct([...index.files()]);
      if (urls.length < 2) return null;
      // `coalesce(stats_min_value, stats_min)` for the reason `tileBoxes` gives: Parquet's original
      // statistics fields are defined as a signed comparison and a writer that gets that right
      // leaves them empty, so a reader that knows only the deprecated pair concludes the footer
      // carries no bound at all. Read either.
      const rows = await query(
        `SELECT file_name AS file, ` +
          `min(coalesce(stats_min_value, stats_min)) AS lo, ` +
          `max(coalesce(stats_max_value, stats_max)) AS hi ` +
          `FROM parquet_metadata(${list(urls)}) ` +
          `WHERE path_in_schema = ${lit(index.orderedBy)} GROUP BY 1`,
      );
      const byUrl = new Map<string, KeyRange>();
      for (const row of rows) {
        const url = String(row['file']);
        const lo = row['lo'];
        const hi = row['hi'];
        byUrl.set(
          url,
          typeof lo === 'string' && typeof hi === 'string'
            ? { url, lo: utf8(lo), hi: utf8(hi) }
            : { url, lo: null, hi: null },
        );
      }
      // A tile whose name did not come back verbatim is one this cannot exclude either.
      return urls.map((url) => byUrl.get(url) ?? { url, lo: null, hi: null });
    })();
    keyRanges.set(type.type, loading);
    return loading;
  };

  /**
   * The index tiles a batch of keys can be in. Linear in tiles x keys, and deliberately: the tiles
   * of an index ARE disjoint and sorted, which would make this a binary search per key, but that is
   * a property of the writer rather than of the format and a reader that assumed it would answer a
   * badly written corpus with silence. A few hundred tiles against a caller's own batch is nothing
   * beside the read it decides.
   */
  const indexFilesFor = (ranges: readonly KeyRange[], keys: readonly Uint8Array[]): string[] =>
    ranges
      .filter((r) => r.lo === null || r.hi === null || keys.some((k) => couldHold(k, r.lo!, r.hi!)))
      .map((r) => r.url);

  /**
   * Every vertex whose identity is in `ids` — **one scan for the whole batch, per type.**
   *
   * The scan is the expense the whole surface is most able to multiply (see {@link Corpus.node}),
   * and a walk seeded by ten identities that ran ten of them would pay ten times for one answer.
   * Nothing bounds the `IN` list: it is the caller's own set, and the corpus cannot refuse it.
   */
  const findByIdentity = async (
    ids: readonly string[],
    only?: string,
  ): Promise<PlacedVertex[]> => {
    if (ids.length === 0) return [];
    const candidates = (only === undefined ? addressing.types : [vertexType(only)]).filter((type) =>
      has(type.type, IDENTITY),
    );
    if (candidates.length === 0) {
      throw new CorpusReadError(
        `no vertex type here carries a ${IDENTITY} column, so nothing in this corpus has a name ` +
          `that survives a re-layout — where the identity lives when the drawing tile does not ` +
          `carry it is an open convention`,
      );
    }
    const found: PlacedVertex[] = [];
    for (const type of candidates) {
      const index = type.index;
      if (index === null) {
        // **The scan**, and it is the whole cost this member has. There is no index from a
        // subject to an address, and the payload's own footers do not help: the rows are in
        // Morton order and subjects are not, so every tile's `min`/`max` for `subject` overlaps
        // every other's and the engine skips nothing. At five million vertices the `subject`
        // column is 8.016 compressed bytes per row, so one lookup reads about 40 MB — column
        // pruning is the only thing keeping it off the other six.
        const rows = await query(
          `SELECT * FROM read_parquet(${list(payloadFiles.get(type.type)!)}) ` +
            `WHERE ${ident(IDENTITY)} IN (${ids.map(lit).join(', ')})`,
        );
        for (const row of rows) found.push(vertexOf(type.type, row));
        continue;
      }

      // **The seek**, and it is addressed rather than searched at both ends.
      //
      // First the index: two columns over tiles sorted by the key with disjoint ranges, which is
      // the arrangement that lets a reader go to the one tile a value can be in. The payload cannot
      // be arranged that way and keep the Morton order a window depends on, which is why the index
      // is a second table rather than a second sort.
      //
      // **The arrangement is right and DuckDB does not exploit it**, so the list of files is what
      // exploits it. `key = 'x'` prunes to one index tile and reads 3.98 MB; `key IN ('x','y')`, or
      // the same spelled with `OR`, prunes to NONE and reads all 245 of them, 44.1 MB — measured on
      // v1.5.3 over a million-vertex corpus, and DuckDB prunes no disjunction over a VARCHAR column
      // at all. Taking the batch apart to get one equality per query is NOT the fix and was tried:
      // it crosses back over the batched read at about eleven seeds (11 x 3.98 > 44.1), and it is
      // a lookup per seed, which is the one cost this surface is most able to multiply.
      //
      // So {@link indexRanges} reads the same statistics the engine declined to use, once per
      // corpus, and the batch stays ONE query over the handful of files its keys can be in. Under
      // `rowgroups` there is nothing to leave out — one file, and a row group has no URL — and the
      // batched read over the whole index is what there is.
      const ranges = await indexRanges(type);
      const keys = ids.map(utf8);
      const indexFiles =
        ranges === null ? distinct([...index.files()]) : indexFilesFor(ranges, keys);
      // No tile's range can hold any of these keys, which is an answer and not a failure to look.
      if (indexFiles.length === 0) continue;
      const hits = await query(
        `SELECT ${ident(index.orderedBy)} AS id, dense_id ` +
          `FROM read_parquet(${list(indexFiles)}) ` +
          `WHERE ${ident(index.orderedBy)} IN (${ids.map(lit).join(', ')})`,
      );
      if (hits.length === 0) continue;

      // Then the payload, at the tiles those addresses NAME — not all of them. This is the half
      // that turns a lookup into arithmetic: `tileOf` is a shift.
      const addresses = hits.map((row) => idOf(row.dense_id, `${type.type}.dense_id`));
      const tiles = [...new Set(addresses.map((d) => type.tileOf(d)))].sort(ascending);
      const rows = await query(
        `SELECT * FROM read_parquet(${list(distinct(tiles.map((k) => type.tileUrl(k))))}) ` +
          `WHERE dense_id IN (${addresses.join(', ')})`,
      );
      // An index that names an address the payload does not have is a corpus defect, not a miss:
      // `apps/corpus`'s `index-agrees-with-the-payload` is what catches it, and a reader that
      // quietly returned fewer rows than the index promised would hide exactly that.
      if (rows.length !== addresses.length) {
        throw new CorpusReadError(
          `${type.type}'s index names ${addresses.length} address(es) and the payload has ` +
            `${rows.length} of them — the index disagrees with the tiles it indexes`,
        );
      }
      for (const row of rows) found.push(vertexOf(type.type, row));
    }
    return found;
  };

  /**
   * Read a plan's adjacency tiles, **emitting each edge once** however many orientations found it.
   *
   * One function for both members, because they had two copies of this loop and only one of them
   * deduplicated: an edge of a self-type relation is in `by_source` and in `by_target`, so reading
   * both and concatenating doubles it. `emitted` is a parameter so a multi-hop walk can carry one
   * set across every hop.
   */
  const readEdges = async (
    vertexTypeName: string,
    groups: readonly EdgeTiles[],
    restrict: (adjacency: AdjacencyAddress) => string,
    emitted: Set<string> = new Set(),
  ): Promise<PlacedEdge[]> => {
    const edges: PlacedEdge[] = [];
    for (const group of groups) {
      const adjacency = adjacencyFor(vertexTypeName, group.edgeType, group.direction);
      if (adjacency === null) continue;
      const rows = await query(
        `SELECT src_dense, dst_dense FROM read_parquet(${list(group.urls)}) ` +
          `WHERE ${restrict(adjacency)}`,
      );
      for (const row of rows) {
        const src = idOf(row['src_dense'], 'src_dense');
        const dst = idOf(row['dst_dense'], 'dst_dense');
        const key = `${group.edgeType} ${src} ${dst}`;
        if (emitted.has(key)) continue;
        emitted.add(key);
        edges.push({ edgeType: group.edgeType, src, dst });
      }
    }
    return edges;
  };

  /**
   * The verb half, booted on first use and not before.
   *
   * **The verbs name tables and this corpus is files, so something has to bridge that.** Every
   * statement `fossil-graph` composes reads `FROM "Person"` or `FROM "Person_knows_Person"` — it
   * is the same SQL the native runtime and `fossil-mcp` run, and it is single-source with them
   * precisely because it does not know where the bytes are. So the door registers the views the
   * verbs expect, over the paths the manifest already gave it, and `crates/fossil-mcp/src/lib.rs`
   * does the identical thing for the identical reason on the other side.
   *
   * **`TEMP`, and that is not a detail.** A plain `CREATE OR REPLACE VIEW "Person"` would
   * overwrite a host's own table of that name — `Person` is not an unlikely name for one, and
   * `tests/e2e.test.ts` creates exactly it. A temp view is resolved before `main` and dropped with
   * the connection, so opening a corpus shadows the host's catalog for as long as the corpus is
   * open and destroys nothing in it.
   *
   * **The edge orientation is a glob and it is the only one in this file.** A vertex type's tiles
   * are enumerated from `vertex_count` and `chunk_size`, which is what the declared count is for;
   * an adjacency's are not, because a tile whose vertices have no edges is a file that was never
   * written and a run of them is a gap no arithmetic predicts. `read_parquet` over an enumerated
   * list containing one absent file is an error, not an empty relation. The rule this file states
   * elsewhere — never glob — is about the vertex payload, where a glob picks up the staged
   * single-file copy beside the tiles and counts every row twice; `<adjacency>/tile*.parquet` has
   * no such sibling inside it. `fossil-mcp` makes the same exception and says so.
   */
  let transport: Promise<GraphClient> | null = null;

  const verbs = (): Promise<GraphClient> => {
    transport ??= (async (): Promise<GraphClient> => {
      if (options.wasmUrl !== undefined) await initFossilGraphWasm({ wasmUrl: options.wasmUrl });
      for (const type of addressing.types) {
        await query(
          `CREATE OR REPLACE TEMP VIEW ${ident(type.type)} AS ` +
            `SELECT * FROM read_parquet(${list(distinct(payloadFiles.get(type.type)!))})`,
        );
      }
      for (const edge of addressing.edges) {
        // The source-ordered orientation, because that is the one a verb reads the whole relation
        // from. An edge type publishing none gets no view rather than a path that 404s — and the
        // verb that reaches for it fails by name, which is the diagnosis.
        const adjacency = edge.adjacency('src');
        if (adjacency === null) continue;
        const source =
          adjacency.container === 'rowgroups'
            ? lit(adjacency.tileUrl(0))
            : lit(`${adjacency.prefix}tile*.parquet`);
        await query(
          `CREATE OR REPLACE TEMP VIEW ` +
            `${ident(`${edge.srcType}_${edge.edgeType}_${edge.dstType}`)} AS ` +
            `SELECT * FROM read_parquet(${source})`,
        );
      }
      return createGraphClient({ query, manifestFiles });
    })();
    return transport;
  };

  return {
    url,
    types,
    addressing,

    async schema(params = {}) {
      return (await verbs()).schema(params);
    },
    async read(params) {
      return (await verbs()).read(params);
    },
    async expand(params) {
      return (await verbs()).expand(params);
    },
    async path(params) {
      return (await verbs()).path(params);
    },
    async aggregate(params) {
      return (await verbs()).aggregate(params);
    },
    async executeSql(params) {
      return (await verbs()).executeSql(params);
    },

    async extent(type) {
      const address = vertexType(type);
      if (extents.has(address.type)) return extents.get(address.type)!;
      // The same footers a window needs, read once for both. This used to sweep
      // `parquet_metadata` over every tile on its own, which made opening a corpus and framing it
      // two O(N) passes over the same bytes — 2,450 range requests each at five million, measured
      // over a plain HTTP origin. The boxes are per tile; the extent is their union.
      let answer: Extent | null = null;
      if (has(address.type, 'x') && has(address.type, 'y')) {
        const boxed = await tileBoxes(address.type);
        if (boxed.length > 0) {
          answer = {
            minX: Math.min(...boxed.map((b) => b.x0)),
            maxX: Math.max(...boxed.map((b) => b.x1)),
            minY: Math.min(...boxed.map((b) => b.y0)),
            maxY: Math.max(...boxed.map((b) => b.y1)),
          };
        }
      }
      extents.set(address.type, answer);
      return answer;
    },

    /**
     * The vertices in a rectangle and the edges among them.
     *
     * **Vertices are pruned by the engine and edges by the address, and the asymmetry is the whole
     * design.** A box over `x`/`y` is one conjunctive range, which is precisely what a Parquet
     * row-group box answers, so the tile selection is left to DuckDB's footer pruning and this
     * writes none of it. That is not a contradiction of the measured finding that *pruning cannot
     * be expressed as a predicate*: what was measured there is 179 disjoint `dense_id` ranges,
     * which cost 189 ms against 5 ms for no pruning at all because the engine evaluates them per
     * row. The `WHERE` here selects which rows come back; what selects which bytes are read is the
     * footer, and the Morton order is what keeps the boxes tight enough for it to matter.
     *
     * Edges have no such column. An adjacency tile is addressed by the *vertex* tile of the endpoint
     * it is ordered by, so the tiles are computed from the answer — `dense_id >> shift` over the
     * vertices that came back — and only those files are opened. Asking the whole relation instead
     * was measured at a million vertices and did not return in 45 seconds.
     *
     * **`complete` is about incidence.** With both orientations it is `true`: every edge touching a
     * vertex in the box is in the answer. With `['src']` it is `false` with a `not-requested` gap,
     * because that is the drawing read — every drawable edge has its source on screen, and the
     * edges whose destination is drawn and whose source is off it render identically to nothing.
     * An orientation the corpus does not publish is a `not-declared` gap, which is a different fact
     * and is not reported as the same one.
     *
     * What it does **not** report is truncation, because it does not truncate: there is no row cap,
     * and a box over a dense region returns everything in it. A caller that needs a bound puts it
     * in the box.
     */
    async window(params) {
      const { type, directions = ['src', 'dst'], ...box } = params;
      const address = vertexType(type);
      if (!has(address.type, 'x') || !has(address.type, 'y')) {
        throw new CorpusReadError(
          `${address.type} carries no x/y, so no rectangle names any of it — its payload is ` +
            `${fieldsOf(address.type).map((f) => f.name).join(', ') || 'empty'}`,
        );
      }

      const all = await tileBoxes(address.type);
      // Every TILE carries a box, or the cache would have dropped it and this would read the whole
      // set — which is what it did before the cache existed, and is still the correct answer. The
      // comparison is against the tile count and not the file count: under the row-group container
      // one file carries every tile, and comparing files would have made a corpus with two or more
      // tiles look like one with boxes missing.
      const candidates = BigInt(all.length) === address.tiles
        ? distinct(intersecting(all, box).map((b) => address.tileUrl(b.tile)))
        : [...payloadFiles.get(address.type)!];
      const rows =
        candidates.length === 0
          ? []
          : await query(`SELECT * FROM read_parquet(${list(candidates)}) WHERE ${boxOf(box)}`);
      const vertices = rows.map((row) => vertexOf(address.type, row));
      const tiles = [...new Set(vertices.map((v) => address.tileOf(v.denseId)))].sort(ascending);

      // The addressing decides which orientations apply and why one is missing; this only fetches.
      // Reproducing that rule here is how the two halves of a window drift apart.
      const plan = addressing.tilesFor({ type: address.type, tiles, directions });
      const edges =
        tiles.length === 0
          ? []
          : await readEdges(
              address.type,
              plan.edges,
              (adjacency) =>
                `${ident(adjacency.column)} IN (SELECT dense_id FROM ` +
                `read_parquet(${list(plan.vertexUrls)}) WHERE ${boxOf(box)})`,
            );

      return {
        type: address.type,
        box,
        tiles,
        vertices,
        edges,
        complete: plan.complete,
        gaps: plan.gaps,
      };
    },

    levels(type) {
      const address = vertexType(type);
      const count = Number(address.count ?? 0n);
      const out: LevelInfo[] = [];
      for (let level = 0; ; level += 1) {
        const stride = strideOf(level);
        out.push({
          level,
          stride,
          count: Math.ceil(count / stride),
          // Nothing writes a level yet, and the field is here rather than absent because what
          // arrives when one does is a cheaper read of the same rows — see {@link ViewCost.read}.
          written: false,
        });
        if (stride >= count) return out;
      }
    },

    levelFor({ type, budget, ...box }) {
      const address = vertexType(type);
      const count = Number(address.count ?? 0n);
      const codes = anchors.get(address.type);
      if (codes === undefined) {
        // No anchor, so no rectangle reaches the tiles without a Parquet reader — and this member
        // may not have one, because a camera calls it before it asks for anything. The whole type
        // is the only pure answer, and it is the conservative one: too coarse, never too fine.
        return budget > 0 && count > budget ? Math.ceil(Math.log2(count / budget)) : 0;
      }
      return levelForBox({ box: gridBox(box), codes, budget, chunkSize: address.chunkSize, count });
    },

    async view(params) {
      const started = Date.now();
      const {
        type,
        level,
        fill = 'cluster_id',
        pinned = [],
        links: wantLinks = true,
        minLinkLength = 0,
        ...box
      } = params;
      const address = vertexType(type);
      if (!has(address.type, 'x') || !has(address.type, 'y')) {
        throw new CorpusReadError(
          `${address.type} carries no x/y, so no rectangle names any of it — its payload is ` +
            `${fieldsOf(address.type).map((f) => f.name).join(', ') || 'empty'}`,
        );
      }
      if (!Number.isInteger(level) || level < 0) {
        throw new CorpusReadError(`a level is a non-negative integer; got ${String(level)}`);
      }
      const stride = strideOf(level);
      const chunk = BigInt(address.chunkSize);
      const pins = [...new Set(pinned.map((id) => BigInt(id)))].sort(ascending);

      /**
       * Which tiles, and by which artefact.
       *
       * The anchor is preferred because a code range describes a tile more tightly than its `x`/`y`
       * box does; `/docs/design/corpus` measures the two over the same nine windows. The footer
       * path is the fallback for a corpus publishing no `codes:`, and it stays correct, only looser.
       */
      const all = await tileBoxes(address.type);
      const boxed = BigInt(all.length) === address.tiles;
      const codes = anchors.get(address.type);
      const addressed: ViewCost['addressed'] = codes !== undefined ? 'anchor' : 'footer';
      const selected = new Set<number>(
        codes !== undefined
          ? mortonTilesFor({ box: gridBox(box), codes })
          : boxed
            ? intersecting(all, box).map((b) => Number(b.tile))
            : all.map((b) => Number(b.tile)),
      );
      // A pin's tile is COUNTED. Left out of the selection the disjunct that brings the pin back
      // has nothing to match against, and the fetch it costs would be missing from the ledger
      // rather than absent from the read.
      for (const id of pins) selected.add(Number(address.tileOf(id)));

      const held = [...selected].sort((a, b) => a - b);
      const weights = new Map(all.map((b) => [Number(b.tile), b]));
      const runs = runsOf(held, weights);
      const bytes = runs.reduce((sum, run) => sum + run.bytes, 0);

      const urls = distinct(held.map((tile) => address.tileUrl(tile)));
      const empty: View = {
        type: address.type,
        box,
        level,
        stride,
        matched: 0,
        denseIds: new BigUint64Array(0),
        positions: new Float32Array(0),
        categories: new Uint32Array(0),
        marks: 0,
        links: new Uint32Array(0),
        cost: { tiles: 0, ofTiles: all.length, runs: 0, bytes: 0, read: 'strided', addressed, ms: 0 },
      };
      if (urls.length === 0) return empty;

      const ranges = (column: string): string =>
        runs
          .map(
            (run) =>
              `${column} BETWEEN ${BigInt(run.first) * chunk} AND ${BigInt(run.last + 1) * chunk - 1n}`,
          )
          .join(' OR ');
      const pinList = pins.length > 0 ? `dense_id IN (${pins.join(', ')})` : null;
      const strided = stride > 1 ? `dense_id % ${stride} = 0` : 'TRUE';

      const base =
        `WITH held AS (\n` +
        `  SELECT dense_id, x, y, ${ident(fill)} AS cat FROM read_parquet(${list(urls)})\n` +
        `  WHERE ${ranges('dense_id')}\n` +
        `), inrect AS (\n  SELECT * FROM held WHERE ${boxOf(box)}\n` +
        `), pool AS (\n  SELECT dense_id, x, y, cat FROM inrect WHERE ${strided}\n` +
        (pinList === null
          ? ''
          : `  UNION\n  SELECT dense_id, x, y, cat FROM held WHERE ${pinList}\n`) +
        `), vis AS (\n` +
        `  SELECT dense_id, x, y, cat, (SELECT count(*) FROM inrect) AS matched,\n` +
        `         (row_number() OVER (ORDER BY dense_id) - 1)::INTEGER AS local FROM pool\n)`;

      // The `src` orientations of every incident edge type, addressed by the tiles already chosen.
      // The addressing decides which orientations exist and why one is missing; reproducing that
      // rule here is how the two halves of a read drift apart.
      const plan = addressing.tilesFor({ type: address.type, tiles: held, directions: ['src'] });
      const edgeUrls = wantLinks ? distinct([...plan.edgeUrls]) : [];

      /**
       * The far ends, and why they are in the same answer.
       *
       * `span` keeps an edge with at least one end drawn and BOTH ends positioned — both in `held`,
       * which is the tiles that were opened. An edge neither of whose ends is drawn is an edge
       * somewhere else. `anchor` numbers the far ends past the marks, so `marks` stays a prefix
       * length and a caller can slice rather than filter.
       */
      const floor = Number.isFinite(minLinkLength) && minLinkLength > 0 ? minLinkLength : 0;
      const long =
        floor > 0
          ? ` AND (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) >= ${floor * floor}`
          : '';
      const withEdges =
        edgeUrls.length === 0
          ? base
          : `${base}, span AS (\n` +
            `  SELECT sv.local AS src, tv.local AS dst, e.src_dense, e.dst_dense\n` +
            `  FROM read_parquet(${list(edgeUrls)}) e\n` +
            `  JOIN held a ON a.dense_id = e.src_dense\n` +
            `  JOIN held b ON b.dense_id = e.dst_dense\n` +
            `  LEFT JOIN vis sv ON sv.dense_id = e.src_dense\n` +
            `  LEFT JOIN vis tv ON tv.dense_id = e.dst_dense\n` +
            `  WHERE (${ranges('e.src_dense')})\n` +
            `    AND (sv.dense_id IS NOT NULL OR tv.dense_id IS NOT NULL)${long}\n` +
            `), anchor AS (\n` +
            `  SELECT h.dense_id, h.x, h.y,\n` +
            `         ((SELECT count(*) FROM vis) + row_number() OVER (ORDER BY h.dense_id) - 1)::INTEGER AS local\n` +
            `  FROM held h WHERE h.dense_id IN (\n` +
            `    SELECT src_dense FROM span WHERE src IS NULL\n` +
            `    UNION SELECT dst_dense FROM span WHERE dst IS NULL)\n)`;

      const pointsSql =
        edgeUrls.length === 0
          ? `${withEdges}\nSELECT local, dense_id, x, y, cat, matched, TRUE AS mark FROM vis ORDER BY local`
          : `${withEdges}\nSELECT local, dense_id, x, y, cat, matched, TRUE AS mark FROM vis\n` +
            `UNION ALL\nSELECT local, dense_id, x, y, 0, NULL::BIGINT, FALSE AS mark FROM anchor\n` +
            `ORDER BY local`;

      const points = await query(pointsSql);
      const links =
        edgeUrls.length === 0
          ? []
          : await query(
              `${withEdges}\nSELECT coalesce(sp.src, sa.local) AS src, coalesce(sp.dst, da.local) AS dst\n` +
                `FROM span sp\n` +
                `LEFT JOIN anchor sa ON sa.dense_id = sp.src_dense\n` +
                `LEFT JOIN anchor da ON da.dense_id = sp.dst_dense`,
            );

      const rows = points.length;
      const denseIds = new BigUint64Array(rows);
      const positions = new Float32Array(rows * 2);
      const categories = new Uint32Array(rows);
      let marks = 0;
      let matched = 0;
      for (let i = 0; i < rows; i += 1) {
        const row = points[i]!;
        denseIds[i] = idOf(row['dense_id'], `${address.type}.dense_id`);
        positions[i * 2] = floatOf(row['x'] ?? 0, `${address.type}.x`);
        positions[i * 2 + 1] = floatOf(row['y'] ?? 0, `${address.type}.y`);
        categories[i] = Number(row['cat'] ?? 0) >>> 0;
        // `mark` is TRUE down the sample and FALSE down the anchors, and the answer is ordered by
        // `local`, so this is a prefix length rather than a count.
        if (row['mark'] === true || row['mark'] === 1) marks = i + 1;
        if (i === 0) matched = Number(row['matched'] ?? 0);
      }
      const edges = new Uint32Array(links.length * 2);
      for (let i = 0; i < links.length; i += 1) {
        edges[i * 2] = Number(links[i]!['src']) >>> 0;
        edges[i * 2 + 1] = Number(links[i]!['dst']) >>> 0;
      }

      return {
        type: address.type,
        box,
        level,
        stride,
        matched,
        denseIds,
        positions,
        categories,
        marks,
        links: edges,
        cost: {
          tiles: held.length,
          ofTiles: all.length,
          runs: runs.length,
          bytes,
          // Nothing writes `l{k}/` yet, so every level is the predicate over the full tiles: the
          // same rows, more bytes. The day a level file exists this is the number that changes and
          // the answer above is not.
          read: 'strided',
          addressed,
          ms: Date.now() - started,
        },
      };
    },

    /**
     * One vertex by identity.
     *
     * **`id` is the subject IRI, never the `dense_id`** — a re-layout renumbers every vertex, so
     * an address held outside the corpus names a different one after the next write. That decision,
     * the alternative it refuses, what would reverse it, and why `is_primary` is not consulted are
     * on `/docs/design/corpus` under *A reader is handed the subject IRI*.
     *
     * **What it costs, measured rather than asserted.** Where the type declares an `index:` this is
     * a seek: two reads, no scan of either table, and see {@link Corpus.types} — `indexed` says
     * which of the two a type gets. The index read is one query for the whole batch over only the
     * index tiles whose own footers say a key could be in them, because this engine prunes no
     * disjunction over a string column and {@link indexRanges} is where that is made up for. Where
     * the type declares no index, it is a scan of the `subject` column over
     * every tile, because the rows are in Morton order and subjects are not, so every tile's
     * `min`/`max` overlaps every other's and the footers prune nothing. At five million vertices
     * that column is 8.016 compressed bytes per row, so one lookup reads about 40 MB. Both answers
     * are the same row; only one is cheap.
     *
     * @throws {CorpusReadError} when no vertex type carries an identity column at all, or when two
     *   types claim the same IRI.
     */
    async node(id, params = {}) {
      if (typeof id !== 'string') {
        throw new TypeError(
          `node takes a subject IRI; got ${typeof id}. A dense_id is an address, and a re-layout ` +
            `gives it to a different vertex.`,
        );
      }
      const found = await findByIdentity([id], params.type);
      if (found.length > 1) {
        throw new CorpusReadError(
          `${id} names ${found.length} vertices, in ${[...new Set(found.map((v) => v.type))].join(', ')}; ` +
            `an identity is unique within its type and this corpus reuses one across types`,
        );
      }
      return found[0] ?? null;
    },

    /**
     * Everything within `depth` hops of a set of identities.
     *
     * One hop is: turn the frontier's ids into tiles, open those adjacency tiles, keep the rows
     * whose addressed endpoint is in the frontier. Both orientations by default, which is an
     * undirected neighbourhood and the reason the corpus stores the adjacency twice — the in-edges
     * of a vertex are in `by_target`'s tile for the same `k`, and asking the relation for them with
     * a recursive query is the thing that did not return in 45 seconds at a million vertices.
     *
     * **Seeds are identities and the walk is addresses.** The whole seed set costs **one** scan of
     * the `subject` column, not one per seed (see {@link Corpus.node} for what a scan is); every
     * hop after that is arithmetic. The vertices come back named, which costs one more read of the
     * tiles the reached ids address.
     *
     * **The frontier reaches SQL as a literal list**, so a hop over n ids emits an `IN` list of n
     * terms. Nothing here bounds n: `depth: 3` on a hub is a query the corpus cannot refuse and
     * this does not pretend to. A caller that needs a bound imposes it on the seeds — and reads
     * {@link Neighbourhood.frontier} to see what the bound cut off.
     *
     * All of one vertex type: seeds that resolve in two types are refused, because a `dense_id` is
     * meaningless without the type it belongs to and a frontier mixing two spaces addresses the
     * wrong tiles in one of them.
     */
    async neighbours(ids, params = {}) {
      const { depth = 1, directions = ['src', 'dst'] } = params;
      if (!Number.isInteger(depth) || depth < 1) {
        throw new RangeError(`depth is a whole number of hops, at least one; got ${depth}`);
      }
      const seeds = [...ids];
      for (const id of seeds) {
        if (typeof id !== 'string') {
          throw new TypeError(
            `neighbours is seeded by subject IRIs; got ${typeof id}. A dense_id is an address, ` +
              `and a re-layout gives it to a different vertex.`,
          );
        }
      }

      const resolved = await findByIdentity(seeds);
      const missing = seeds.filter((id) => !resolved.some((v) => v.id === id));
      const kinds = [...new Set(resolved.map((v) => v.type))];
      if (kinds.length > 1) {
        throw new CorpusReadError(
          `the seeds resolve in ${kinds.join(' and ')}; a dense_id has no meaning without its ` +
            `type, so one walk cannot cross two of them`,
        );
      }

      const type = kinds[0] ?? addressing.vertexType().type;
      const address = vertexType(type);
      const seen = new Map(resolved.map((v) => [v.denseId, v]));
      const edges: PlacedEdge[] = [];
      const emitted = new Set<string>();
      let frontier = resolved.map((v) => v.denseId);
      // One `not-declared`/`not-requested` reading for the whole walk: the orientations are the
      // same at every hop, so the addressing is asked once, with no tiles, rather than per hop.
      const orientations = addressing.tilesFor({ type, tiles: [], directions });

      for (let hop = 0; hop < depth && frontier.length > 0; hop += 1) {
        const tiles = [...new Set(frontier.map((id) => address.tileOf(id)))].sort(ascending);
        const inFrontier = frontier.join(', ');
        const found = await readEdges(
          type,
          addressing.tilesFor({ type, tiles, directions }).edges,
          (adjacency) => `${ident(adjacency.column)} IN (${inFrontier})`,
          emitted,
        );
        edges.push(...found);
        const fresh = [
          ...new Set(found.flatMap((edge) => [edge.src, edge.dst]).filter((end) => !seen.has(end))),
        ];
        for (const vertex of await readByDenseId(type, fresh)) seen.set(vertex.denseId, vertex);
        frontier = fresh;
      }

      return {
        type,
        depth,
        seeds: resolved.map((v) => v.id!),
        missing,
        vertices: [...seen.values()],
        edges,
        // Complete when no orientation was skipped AND nothing was left on the boundary. A frontier
        // that is still populated is an answer whose outermost ring is missing its own edges, which
        // is invisible in the counts — so it is a field rather than a caveat.
        complete: orientations.complete && frontier.length === 0,
        gaps: orientations.gaps,
        frontier,
      };
    },
  };
}
