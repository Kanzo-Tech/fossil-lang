/**
 * The address of a corpus, resolved from its manifest — **by the reader, which is in Rust.**
 *
 * A tile is a fixed range of `dense_id` and its address is a shift, so a reader computes every URL
 * it wants before it emits the first request. That arithmetic used to be written twice: once in
 * `crates/fossil-graph/src/plan.rs`, for `fossil-mcp`, which is a native server with no JS runtime
 * and therefore cannot be the one that goes; and once here, in a thousand lines of TypeScript that
 * agreed with it because people kept making it agree. Nothing compared the two. This module is what
 * is left of the second one: the shapes the answers arrive in, and the calls that ask.
 *
 * **It costs a WASM module, and that is the price rather than an oversight.** Composing a URL was
 * synchronous arithmetic over a parsed manifest and is now a call into `fossil-graph-wasm`, so a
 * caller must `await initFossilGraphWasm({ wasmUrl })` before {@link resolveCorpus} — the same
 * precondition every verb already had. The subpath `@fossil-lang/corpus/address` existed exactly so
 * a third party could address a corpus *without* that, and it is deleted: a WASM-free path is a
 * second implementation, and a second implementation is what this change removed.
 *
 * **What it does not do**, and each absence is the seam this package is on the far side of:
 *
 * - **No bytes.** Nothing here fetches, decodes or reads Parquet. `tileUrl` hands back a string.
 * - **No cache, no debounce, no sampling.** Those are the reader's, and they stay there.
 * - **No rectangles.** Which tiles a box touches comes from the per-tile `x`/`y` statistics in the
 *   Parquet footers — `footer-is-the-index` — and reading a footer needs a Parquet reader. The host
 *   has one, so the host reads its own footers and hands the tile numbers back to
 *   {@link CorpusAddressing.tilesFor}.
 *
 * `openCorpus` in `./corpus.ts` is the layer that reads those footers, by taking an engine from the
 * host rather than growing one. It sits **on** this module and does not absorb it.
 *
 * @see {@link resolveCorpus}
 */

// '../pkg/fossil_graph_wasm.js' is the wasm-bindgen `--target web` output, gitignored and always
// present at build time — the same import `./load.js` makes, and the reason this module now needs
// `initFossilGraphWasm` to have been awaited before any of it runs.
import {
  Corpus as CorpusReader,
  rowsAt as rowsAtLevel,
  strideBits as bitsPerLevel,
  strideOf as idsPerLevelRow,
} from '../pkg/fossil_graph_wasm.js';

import { CorpusManifestError } from './manifest.js';

export { CorpusManifestError, GRAPH_INFO_PATH } from './manifest.js';

/**
 * Which endpoint column addresses an adjacency's tiles — the manifest's own `aligned_by`.
 *
 * `src` is CSR and `dst` is CSC. They are separate addresses rather than one undirected one for the
 * same reason the camera is addressed at all: a union of two relations is a scan, and a pair of
 * URLs is not.
 */
export type Direction = 'src' | 'dst';

/**
 * Which container carries the tiles — `graph.graph.yml`'s own `container`.
 *
 * `files` is one Parquet per tile with the address in the name; `rowgroups` is one Parquet per
 * payload set whose row groups are the tiles. Both are addressed by the same arithmetic and they
 * cost differently: 22.3 requests per window against 5.6, and 1.15 MB of footer against 496 kB, at
 * five million vertices. A file boundary is the only thing that stops contiguous tiles from being
 * fetched in one range.
 *
 * It is a manifest field and not something a reader works out, because working it out means listing
 * a directory and there is no listing over HTTP.
 */
export type Container = 'files' | 'rowgroups';

/**
 * How many bits of `dense_id` level `k` drops — `2k`, and the number a reader ADDS to its payload
 * tile shift to address a level tile.
 *
 * The pyramid's base is spelled once, in `VertexLevels::stride_bits`, and reached from here rather
 * than respelled: a `2^k` on this side of the boundary would be a different corpus.
 */
export const strideBits = (level: number): number => bitsPerLevel(level);

/**
 * How many `dense_id`s one row of level `k` stands for — `4^k`.
 *
 * The decimation level *k* means **the vertices whose `dense_id` is a multiple of this**. Over a
 * Morton-ordered `dense_id` that is one vertex per quadtree cell of depth *k*, so a level is a level
 * of the same curve the tile address is read off — not a sample, not a budget, not a cap.
 */
export const strideOf = (level: number): bigint => idsPerLevelRow(level);

/**
 * How many rows level `k` of a type of `count` rows holds — `ceil(count / strideOf(k))`.
 *
 * A level is a predicate, so this answers for every `k` and not only for the ones a writer spent
 * bytes on — which is what lets {@link LevelAddress.has} be a cost and not a refusal.
 */
export const rowsAt = (count: bigint, level: number): bigint => rowsAtLevel(count, level);

/** One vertex type's address: where its tiles are and which `dense_id` range each holds. */
export interface VertexAddress {
  /** The type label, e.g. `Person`. */
  readonly type: string;
  /** Where its tiles are, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /** Rows per tile. A power of two, checked at resolve. */
  readonly chunkSize: number;
  /** `log2(chunkSize)` — the shift that turns a `dense_id` into a tile number. */
  readonly shift: number;
  /** The manifest's `vertex_count`, or `null` when it declares none. */
  readonly count: bigint | null;
  /** `ceil(count / chunkSize)`, or `null` when the manifest declares no count. */
  readonly tiles: bigint | null;
  /** Which container carries this type's tiles. */
  readonly container: Container;
  /** The tile holding `denseId`. */
  tileOf(denseId: bigint): bigint;
  /**
   * The tiles holding a batch of `dense_id`s, in order.
   *
   * **The batch is the shape that crosses**, and {@link VertexAddress.tileOf} is one call to it: a
   * reader asks this of a frontier, of a pin set, of the addresses an index handed back, and a call
   * per id would pay the boundary once per vertex.
   */
  tilesOf(denseIds: Iterable<bigint>): bigint[];
  /**
   * The file tile `k` is in: `<prefix>chunk{k}.parquet` under `files`, `<prefix>tiles.parquet`
   * under `rowgroups`, where every tile of the type names the same file and the footer's box on
   * `dense_id` is what says which row groups are the tile.
   */
  tileUrl(tile: number | bigint): string;
  /**
   * Every payload FILE of this type, in order and distinct — one per tile under `files`, one in
   * total under `rowgroups`. Throws when the manifest declares no count, because then there is no
   * set to enumerate: that is the difference between addressing a tile somebody asked for and
   * knowing how many there are.
   */
  files(): readonly string[];
  /**
   * The identity index, when the manifest declares one, and `null` otherwise.
   *
   * `null` is a legal corpus and not an incomplete one — a lookup by identity answers without it,
   * by scanning — so a reader that finds none reports the cost rather than refusing. That is the
   * opposite of {@link VertexAddress.count}, whose absence makes a question unanswerable.
   */
  readonly index: IndexAddress | null;
  /**
   * The **written levels** of this type, or `null` when the manifest declares none.
   *
   * `null` is a legal corpus and the most legal of the three optional blocks: a level is a
   * predicate, so every level is answerable with or without this, and what it changes is which
   * bytes answer it. See {@link LevelAddress}.
   */
  readonly levels: LevelAddress | null;
}

/**
 * Where a vertex type's identity index lives.
 *
 * A second copy of the type ordered by identity, tiled with the same `tile{k}` spelling as
 * everything else. It cannot be a column of the payload: one table has one sort, the payload's is
 * Morton because the spatial order IS the id space, and a lookup by identity needs the other one.
 *
 * Its tiles are sorted by {@link IndexAddress.orderedBy} with disjoint ranges, which is what lets a
 * reader binary-search the footers to one tile — the thing the payload's own footers cannot do for
 * `subject`, because Morton order and lexicographic order have nothing to do with each other and
 * every tile's range overlaps every other's.
 */
export interface IndexAddress {
  /** Where the index tiles are, resolved against the corpus base, with a trailing separator. */
  readonly prefix: string;
  /** The column the tiles are sorted by, and the one a lookup is keyed on. */
  readonly orderedBy: string;
  /** Rows per index tile. Unrelated to the payload's: tile `k` here is the `k`th slice of the
   *  SORTED order, not a `dense_id` range. */
  readonly chunkSize: number;
  /** `ceil(count / chunkSize)`, or `null` when the manifest declares no `vertex_count`. */
  readonly tiles: bigint | null;
  /** Which container carries the index tiles. The corpus's, never a second answer. */
  readonly container: Container;
  /** Every index file, in order and distinct. Throws when the count is absent, like {@link VertexAddress.files}. */
  files(): readonly string[];
}

/** One orientation of one edge type: declared by the manifest, or absent from it. */
export interface AdjacencyAddress {
  readonly direction: Direction;
  /** Where its tiles are, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /** The endpoint column tile `k` filters on: `src_dense` for `src`, `dst_dense` for `dst`. */
  readonly column: 'src_dense' | 'dst_dense';
  /** `src_chunk_size` for `src`, `dst_chunk_size` for `dst` — a different space on a cross-type edge. */
  readonly chunkSize: number;
  readonly shift: number;
  /**
   * How many tiles this orientation has — **the endpoint vertex type's tile count, not the edge's.**
   *
   * An edge tile is addressed by a *vertex* tile, so `edge_count / chunk_size` is the wrong
   * division and it is wrong quietly: on this corpus it gives ten where there are five, and every
   * URL past the fifth composes cleanly and 404s. `null` when the endpoint type declares no count.
   */
  readonly tiles: bigint | null;
  /** Which container carries this orientation's tiles. The corpus's, never a second answer. */
  readonly container: Container;
  /**
   * `<edge prefix><adj prefix>tile{k}.parquet`. A 404 is "these vertices have no edges here".
   *
   * Under `rowgroups` it is one file for the whole orientation, and the row-group ordinal is *not*
   * the tile: an adjacency tile is however many edges its vertices happen to have, and a tile whose
   * vertices have none contributes no row group to be numbered. What locates it is the footer's box
   * on {@link AdjacencyAddress.column} over the tile's `dense_id` range.
   */
  tileUrl(tile: number | bigint): string;
}

/** One edge type's address, with an entry per orientation the manifest declares. */
export interface EdgeAddress {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  /**
   * The manifest's `edge_count`, or `null` when it declares none. **One number for both
   * orientations** — they are one relation stored twice — so it is not the tile count of either.
   */
  readonly count: bigint | null;
  /** Where the type lives, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /**
   * The orientations that resolve to an address — never a direction the manifest does not publish.
   *
   * An `adj_lists` entry the manifest omits, or declares without a `prefix`, is not here. A
   * corpus that tiles only CSR has `['src']`, and asking it for `dst` returns `null` rather than a
   * string that 404s.
   */
  readonly directions: readonly Direction[];
  /** The declared orientation, or `null` when the corpus does not publish one. */
  adjacency(direction: Direction): AdjacencyAddress | null;
  /**
   * The **written levels of this relation**, or `null` when the manifest declares none.
   *
   * A level of a relation is the edges incident to a level-`k` vertex, carrying BOTH endpoints'
   * coordinates — which is what lets a coarse camera draw a line without opening the vertex
   * payload for its far end. `null` is a corpus and not a gap: a reader without one draws the same
   * edges out of the adjacency and the payload, and only reads more.
   */
  readonly levels: EdgeLevelAddress | null;
}

/**
 * Where a relation's **level sets** are, and which ones exist.
 *
 * The sibling of {@link LevelAddress} and addressed by the same rule: tile `j` of level `k` holds
 * the rows whose `src_dense` is in `[j · chunkSize · stride(k), (j+1) · chunkSize · stride(k))` —
 * the SOURCE level's own tile range — so a reader that can address a vertex level can address the
 * edges beside it with no new arithmetic.
 *
 * **What it carries that no other set does is the endpoints' coordinates.** `src_x`, `src_y`,
 * `dst_x`, `dst_y` beside the two ids, which makes a level set self-drawing: the lines and their
 * far ends come out of one file. Measured on the bench corpus at a three-pixel floor, a VERTEX
 * level can position 0.79% of the edges the same view draws, so a pyramid without this one answers
 * a view with links by opening the payload — the read it exists to avoid.
 */
export interface EdgeLevelAddress {
  /** The levels written, finest first — the source type's own. */
  readonly levels: readonly number[];
  /** Rows per tile of the SOURCE vertex type, which the tile's `dense_id` range is built from. */
  readonly chunkSize: number;
  /** Which container carries them. The corpus's, never a second answer. */
  readonly container: Container;
  /** Whether level `k` is written. `false` is a cost and not a refusal. */
  has(level: number): boolean;
  /** The file tile `j` of level `k` is in, spelled by the corpus's container. */
  tileUrl(level: number, tile: number | bigint): string;
}

/**
 * Where a vertex type's **written levels** are, and which ones exist.
 *
 * **Level `k` is `dense_id % strideOf(k) == 0`, whatever this says.** A level is a predicate over
 * the payload, and a written `l{k}/` is a cache of it — so a corpus declaring none draws the
 * identical picture and only reads more, and that is what keeps the pyramid from being a second
 * contract. What this block changes is a byte count, and `Frame.matchedAt` in `./corpus.ts` is
 * where the difference is visible.
 *
 * **The numbers are declared and not derived**, unlike everything else here, and the manifest side
 * argues why: a level list is `log4(V / chunk_size)` integers whatever the corpus is, and *which*
 * levels a writer spent bytes on is a policy — a reader re-deriving it from `vertex_count` and
 * `chunk_size` would reimplement the writer's plan and 404 the day the plan moved.
 */
export interface LevelAddress {
  /** The levels written, finest first, as the manifest declares them. */
  readonly levels: readonly number[];
  /**
   * Rows per tile within a level set. Declared rather than inherited from
   * {@link VertexAddress.chunkSize}, because turning a level tile back into a `dense_id` range
   * multiplies by it and a number that has to be assumed is one a writer can change in silence.
   */
  readonly chunkSize: number;
  /** Which container carries the level tiles. The corpus's, never a second answer. */
  readonly container: Container;
  /**
   * Whether level `k` is written.
   *
   * `false` is not a refusal and not an absence of the level: the level exists at every `k` — it is
   * a predicate — and this says only whether reading it costs the level's bytes or the type's.
   */
  has(level: number): boolean;
  /** The tile of level `k` holding `denseId`: the payload's shift plus {@link strideBits}. */
  tileOf(level: number, denseId: bigint): bigint;
  /** The file tile `j` of level `k` is in, spelled by the corpus's container. */
  tileUrl(level: number, tile: number | bigint): string;
  /** How many rows level `k` holds, or `null` when the manifest declares no count. */
  rows(level: number): bigint | null;
  /** How many tiles level `k` has, or `null` when the manifest declares no count. */
  tiles(level: number): bigint | null;
  /**
   * Every file of level `k`, in order and distinct.
   *
   * Throws on {@link VertexAddress.files}' argument when the count is absent, and on a level that
   * is not written — the second one because the URL would name a prefix nobody wrote, which is the
   * one failure a reader cannot tell from an empty level.
   */
  files(level: number): readonly string[];
}

/** Why an orientation is missing from an answer. Both reasons are honest; they are not the same. */
export type GapReason =
  /** The caller did not ask for this direction. */
  | 'not-requested'
  /** The manifest does not publish an address for it, so no URL exists to ask for. */
  | 'not-declared';

/** One orientation of one edge type that a window did not read, and why. */
export interface Gap {
  readonly edgeType: string;
  readonly direction: Direction;
  readonly reason: GapReason;
}

/** The files one edge type contributes to a window, distinct, in the orientation that addresses it. */
export interface EdgeTiles {
  readonly edgeType: string;
  readonly direction: Direction;
  readonly urls: readonly string[];
}

/**
 * The tiles a set of vertex tiles addresses, **and what that set is complete for.**
 *
 * A partial answer has to be distinguishable from a complete one. CSR alone is complete for
 * *drawing* — every drawable edge has its source on screen, therefore in a tile the window already
 * fetched — and incomplete for *incidence*: an edge whose destination is drawn and whose source the
 * window never selected is unreachable through `by_source`, and there are measurably many. So
 * `complete` is about incidence and `gaps` says which orientations are missing from it, separating
 * the caller not asking from the corpus not publishing.
 */
export interface AddressedTiles {
  /** The vertex type the tile numbers are in the `dense_id` space of. */
  readonly type: string;
  readonly tiles: readonly number[];
  /**
   * The files those tiles are in, distinct and in order.
   *
   * Distinct is a no-op under `files` and the whole answer under `rowgroups`, where every tile of a
   * type is the same file: a list that named it once per tile would be one scan per tile.
   */
  readonly vertexUrls: readonly string[];
  readonly edges: readonly EdgeTiles[];
  /** Every URL in {@link edges}, flattened and distinct, in declaration order. */
  readonly edgeUrls: readonly string[];
  /** `true` when every edge incident to a vertex in these tiles is in one of these files. */
  readonly complete: boolean;
  readonly gaps: readonly Gap[];
}

/** What {@link resolveCorpus} takes. */
export interface ResolveCorpusOptions {
  /**
   * The manifest YAMLs, keyed by dataset-relative path, pre-fetched by the host — the same shape
   * the verbs already take. They are small: one index plus one file per type.
   */
  manifestFiles: Record<string, string>;
  /**
   * Where the corpus lives, without a trailing slash — a URL origin and path, a static route, or
   * `''` for addresses relative to the dataset root. Prepended to every URL and nothing else.
   */
  base?: string;
}

/**
 * A corpus resolved to addresses.
 *
 * **It was `ResolvedCorpus`, one import away from `Corpus`, and neither name said what differed.**
 * Both are a corpus; one is the door — asynchronous, engine-backed, answering with rows — and this
 * one is the addressing underneath it, which answers with URLs and never reads a byte. So it is
 * named for the layer rather than for the noun the two share, and {@link Corpus.addressing} is
 * where a caller that has outgrown the door reaches it.
 */
export interface CorpusAddressing {
  readonly base: string;
  /** Which container the corpus declares. One answer for every payload set in it. */
  readonly container: Container;
  readonly types: readonly VertexAddress[];
  readonly edges: readonly EdgeAddress[];
  /** One vertex type by name, or the first the index names when no name is given. */
  vertexType(name?: string): VertexAddress;
  /** The edge types incident to `type` — as source, as destination, or both on a self-edge. */
  incident(type: string): readonly EdgeAddress[];
  /**
   * The URLs a set of vertex tiles addresses.
   *
   * `directions` defaults to `['src']`, which is the drawing read: it fetches the out-edges of
   * every vertex in the set and returns `complete: false` with a `not-requested` gap, because a
   * set of drawn vertices has in-edges it did not ask for. Pass `['src', 'dst']` for the incident
   * set.
   *
   * **It was `window`, and that name belonged to the other layer.** `Corpus.rows` takes a
   * rectangle in the corpus's own coordinates and answers with vertices and edges; this takes tile
   * numbers and answers with URLs. One noun for a camera and a URL builder is how a caller ends up
   * passing a box to the one that wants tiles. This layer returns addresses, so it is named for
   * what it addresses.
   */
  tilesFor(params: {
    type?: string;
    tiles: Iterable<number | bigint>;
    directions?: readonly Direction[];
  }): AddressedTiles;
}

// ── what the reader hands back ────────────────────────────────────────────────
//
// `Corpus.snapshot()` serialises `fossil_graph::plan::ReadPlan`, so these are that struct's own
// field names, in `serde`'s spelling. They are the ONLY place this package writes them down: every
// camelCase name above is produced from one of these below, once, at resolve.

interface PlanSnapshot {
  readonly base: string;
  readonly container: Container;
  readonly types: readonly VertexSnapshot[];
  readonly edges: readonly EdgeSnapshot[];
}

interface VertexSnapshot {
  readonly type: string;
  readonly prefix: string;
  readonly chunk_size: bigint;
  readonly shift: number;
  readonly count: bigint | null;
  readonly tiles: bigint | null;
  readonly container: Container;
  readonly index: IndexSnapshot | null;
  readonly levels: LevelSnapshot | null;
}

interface IndexSnapshot {
  readonly prefix: string;
  readonly ordered_by: string;
  readonly chunk_size: bigint;
  readonly tiles: bigint | null;
  readonly container: Container;
}

interface LevelSnapshot {
  readonly levels: readonly number[];
  readonly chunk_size: bigint;
  readonly container: Container;
}

interface AdjacencySnapshot {
  readonly direction: Direction;
  readonly prefix: string;
  readonly column: 'src_dense' | 'dst_dense';
  readonly chunk_size: bigint;
  readonly shift: number;
  readonly tiles: bigint | null;
  readonly container: Container;
}

interface EdgeSnapshot {
  readonly edge_type: string;
  readonly src_type: string;
  readonly dst_type: string;
  readonly count: bigint | null;
  readonly prefix: string;
  readonly directions: readonly Direction[];
  readonly adjacencies: readonly AdjacencySnapshot[];
  readonly levels: LevelSnapshot | null;
}

interface WindowSnapshot {
  readonly type: string;
  readonly tiles: readonly number[];
  readonly vertex_urls: readonly string[];
  readonly edges: ReadonlyArray<{
    readonly edge_type: string;
    readonly direction: Direction;
    readonly urls: readonly string[];
  }>;
  readonly edge_urls: readonly string[];
  readonly complete: boolean;
  readonly gaps: ReadonlyArray<{
    readonly edge_type: string;
    readonly direction: Direction;
    readonly reason: GapReason;
  }>;
}

/**
 * Every refusal the reader makes, as the error this package's callers already catch.
 *
 * `JsError` crosses as a plain `Error` carrying the Rust message verbatim, so what is preserved
 * here is the TYPE and not the wording: a manifest that cannot address itself still names what was
 * wrong, and still does it as a {@link CorpusManifestError}.
 */
function asked<T>(ask: () => T): T {
  try {
    return ask();
  } catch (cause) {
    if (cause instanceof CorpusManifestError) throw cause;
    throw new CorpusManifestError(
      cause instanceof Error ? cause.message : String(cause),
    );
  }
}

/**
 * A batch of `dense_id`s in the width the reader takes them in.
 *
 * **It refuses a `Number`**, which `BigUint64Array` does for it, and a negative, which it does not:
 * `-1n` would arrive as 2⁶⁴−1 and address a tile at the top of the space. A `dense_id` carries more
 * than 53 bits, and JavaScript's `>>` truncates to 32 *before* it shifts, so the same three
 * characters mean something different on each side of this boundary — which is why none of them
 * appear on this one.
 */
function widths(denseIds: Iterable<bigint>): BigUint64Array {
  const batch = [...denseIds];
  for (const id of batch) {
    if (typeof id !== 'bigint') {
      throw new TypeError(
        `a dense_id is a BigInt; got ${typeof id}. It carries more bits than a Number can hold.`,
      );
    }
    if (id < 0n) throw new RangeError(`dense_id is unsigned; got ${id}`);
  }
  return BigUint64Array.from(batch);
}

/** `bigint | undefined` is how an absent `u64` crosses; `null` is how this package spells it. */
const orNull = (value: bigint | undefined): bigint | null => value ?? null;

function levelAddress(
  reader: CorpusReader,
  type: string,
  declared: LevelSnapshot,
): LevelAddress {
  const written = new Set(declared.levels);
  return {
    levels: declared.levels,
    chunkSize: Number(declared.chunk_size),
    container: declared.container,
    has: (level) => written.has(level),
    // `widths` is OUTSIDE `asked`: a caller handing this a `Number` has made a type error and not
    // written an unaddressable manifest, and the two must not come back as the same class.
    tileOf: (level, denseId) => {
      const batch = widths([denseId]);
      return asked(() => reader.levelTilesOf(type, level, batch)[0]!);
    },
    tileUrl: (level, tile) => asked(() => reader.levelTileUrl(type, level, BigInt(tile))),
    rows: (level) => asked(() => orNull(reader.levelRows(type, level))),
    tiles: (level) => asked(() => orNull(reader.levelTiles(type, level))),
    files: (level) => asked(() => reader.levelFiles(type, level)),
  };
}

function vertexAddress(reader: CorpusReader, declared: VertexSnapshot): VertexAddress {
  const type = declared.type;
  const index = declared.index;
  return {
    type,
    prefix: declared.prefix,
    chunkSize: Number(declared.chunk_size),
    shift: declared.shift,
    count: declared.count,
    tiles: declared.tiles,
    container: declared.container,
    index:
      index === null
        ? null
        : {
            prefix: index.prefix,
            orderedBy: index.ordered_by,
            chunkSize: Number(index.chunk_size),
            tiles: index.tiles,
            container: index.container,
            files: () => asked(() => reader.indexFiles(type)),
          },
    levels: declared.levels === null ? null : levelAddress(reader, type, declared.levels),
    // `widths` is OUTSIDE `asked`, for the reason {@link levelAddress} states.
    tileOf: (denseId) => {
      const batch = widths([denseId]);
      return asked(() => reader.tilesOf(type, batch)[0]!);
    },
    tilesOf: (denseIds) => {
      const batch = widths(denseIds);
      return asked(() => [...reader.tilesOf(type, batch)]);
    },
    tileUrl: (tile) => asked(() => reader.vertexTileUrl(type, BigInt(tile))),
    files: () => asked(() => reader.vertexFiles(type)),
  };
}

function edgeAddress(reader: CorpusReader, declared: EdgeSnapshot): EdgeAddress {
  const edgeType = declared.edge_type;
  const adjacencies = new Map<Direction, AdjacencyAddress>(
    declared.adjacencies.map((adjacency) => [
      adjacency.direction,
      {
        direction: adjacency.direction,
        prefix: adjacency.prefix,
        column: adjacency.column,
        chunkSize: Number(adjacency.chunk_size),
        shift: adjacency.shift,
        tiles: adjacency.tiles,
        container: adjacency.container,
        tileUrl: (tile) =>
          asked(() => reader.adjacencyTileUrl(edgeType, adjacency.direction, BigInt(tile))!),
      },
    ]),
  );
  const levels = declared.levels;
  const written = new Set(levels?.levels ?? []);
  return {
    edgeType,
    srcType: declared.src_type,
    dstType: declared.dst_type,
    count: declared.count,
    prefix: declared.prefix,
    directions: declared.directions,
    adjacency: (direction) => adjacencies.get(direction) ?? null,
    levels:
      levels === null
        ? null
        : {
            levels: levels.levels,
            chunkSize: Number(levels.chunk_size),
            container: levels.container,
            has: (level) => written.has(level),
            tileUrl: (level, tile) =>
              asked(() => reader.edgeLevelTileUrl(edgeType, level, BigInt(tile))),
          },
  };
}

/**
 * Resolve a corpus's manifest set into the addresses a reader composes URLs from.
 *
 * ```ts
 * await initFossilGraphWasm({ wasmUrl });
 * const corpus = resolveCorpus({ manifestFiles, base: '/bench/1000000' });
 * const person = corpus.vertexType();
 * const { edgeUrls, complete, gaps } = corpus.tilesFor({ tiles: [3, 4], directions: ['src', 'dst'] });
 * ```
 *
 * **`initFossilGraphWasm` must have been awaited first.** The resolution runs in
 * `fossil-graph-wasm`; without the module instantiated this throws the same way a verb call does.
 *
 * Throws {@link CorpusManifestError} when the manifest cannot address itself — a missing file, a
 * `chunk_size` no shift addresses, an endpoint type the index does not declare, or an edge whose
 * declared tile size disagrees with the vertex type that addresses it. It does **not** throw for an
 * orientation the corpus does not publish: that is a legitimate corpus, and it is reported as an
 * address that does not exist rather than one that 404s.
 */
export function resolveCorpus(options: ResolveCorpusOptions): CorpusAddressing {
  const { manifestFiles, base = '' } = options;
  const reader = asked(() => new CorpusReader(manifestFiles, base));
  const plan = asked(() => reader.snapshot() as PlanSnapshot);

  const types = plan.types.map((declared) => vertexAddress(reader, declared));
  const edges = plan.edges.map((declared) => edgeAddress(reader, declared));
  const byName = new Map(types.map((type) => [type.type, type]));

  const vertexType = (name?: string): VertexAddress =>
    // The refusal is the reader's, not a second copy of the list it names: an unknown type here is
    // told what the manifest DOES declare, and that sentence has one author.
    byName.get(asked(() => reader.vertexTypeName(name)))!;

  return {
    base: plan.base,
    container: plan.container,
    types,
    edges,
    vertexType,
    incident: (type) => edges.filter((e) => e.srcType === type || e.dstType === type),
    tilesFor: ({ type, tiles, directions = ['src'] }) => {
      const window = asked(
        () =>
          reader.window(
            type,
            BigUint64Array.from([...tiles].map(BigInt)),
            [...directions],
          ) as WindowSnapshot,
      );
      return {
        type: window.type,
        tiles: window.tiles,
        vertexUrls: window.vertex_urls,
        edges: window.edges.map((e) => ({
          edgeType: e.edge_type,
          direction: e.direction,
          urls: e.urls,
        })),
        edgeUrls: window.edge_urls,
        complete: window.complete,
        gaps: window.gaps.map((g) => ({
          edgeType: g.edge_type,
          direction: g.direction,
          reason: g.reason,
        })),
      };
    },
  };
}
