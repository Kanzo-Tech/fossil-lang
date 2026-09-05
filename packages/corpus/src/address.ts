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
 * bytes on — which is what lets a missing {@link VertexAddress.projection} be a cost and not a
 * refusal.
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
   * **Every projection of this type**, in manifest order: the payload at `scale: 1` and one per
   * written level. See {@link ProjectionAddress}.
   *
   * A type declaring only its payload is a legal corpus and the most legal of them: a level is a
   * predicate, so every scale is answerable with or without a file for it, and what a written one
   * changes is which bytes answer it.
   */
  readonly projections: readonly ProjectionAddress[];
  /**
   * One projection by its scale, or `null` when the manifest wrote none at that scale — which is
   * a cost and not a refusal.
   */
  projection(scale: number | bigint): ProjectionAddress | null;
  /**
   * Every file of the projection at `scale`, in order and distinct.
   *
   * Throws on {@link VertexAddress.files}' argument when the count is absent, and on a scale
   * nobody wrote — the second one naming the scales that were, because a URL under an unwritten
   * `l{k}/` is the one failure a reader cannot tell from an empty level.
   */
  projectionFiles(scale: number | bigint): readonly string[];
}

/**
 * Where a vertex type's identity index lives.
 *
 * **The one artefact of a corpus that is not a {@link ProjectionAddress}**: it is a second ORDER
 * over the same rows, so the Morton cut does not address it — which is why it carries a
 * {@link IndexAddress.chunkSize} of its own, no scale, and keeps the `tile{k}` filename stem every
 * projection gave up. It cannot be a column of the payload: one table has one sort, the payload's
 * is Morton because the spatial order IS the id space, and a lookup by identity needs the other.
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

/**
 * **One projection of a corpus's sequence, resolved into an address.**
 *
 * Every artefact of a corpus except {@link IndexAddress} is one of these: the payload at
 * `scale: 1`, a vertex level at a coarser scale, an orientation of an adjacency at `scale: 1` with
 * a {@link direction}, a level of a relation at a coarser scale with one. **The payload is not a
 * special case** — it is the projection whose scale is one, and this type does not know which of
 * its instances is which. It is `fossil_graph::plan::ProjectionAddress` as it crosses, and the
 * argument for the shape is there and in `fossil_sinks::manifest`, not restated here.
 *
 * A projection nobody wrote is still ANSWERABLE: a level is the predicate `dense_id % scale == 0`
 * over the payload, and a written `l{k}/` is a cache of it. So a corpus declaring one projection
 * draws the identical picture as one declaring five, and only reads more.
 */
export interface ProjectionAddress {
  /** Where its tiles are, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /**
   * **How many rows of the underlying sequence one row here stands for** — `1` for the payload and
   * the adjacency, and the product a level's exponent left its one home as.
   */
  readonly scale: number;
  /**
   * Which endpoint column addresses these tiles, on an edge projection. `null` on a vertex one,
   * whose address is its own `dense_id`.
   */
  readonly direction: Direction | null;
  /** The column tile `k` filters on — `src_dense`/`dst_dense` on an edge projection, `dense_id` on a vertex one. */
  readonly column: 'dense_id' | 'src_dense' | 'dst_dense';
  /**
   * Rows per tile: the cut, which does not change with the scale. On an edge projection this is the
   * ALIGNED endpoint type's — a different space from the other endpoint's on a cross-type edge.
   */
  readonly chunkSize: number;
  /** `log2(chunkSize) + log2(scale)` — the shift that names a tile, and never a division. */
  readonly shift: number;
  /**
   * How many rows of the sequence this projection addresses: the type's own `vertex_count` divided
   * by the scale, or the aligned endpoint type's on an edge. `null` when no count is declared.
   *
   * **Not the edge's row count**, on an edge projection. An edge tile is addressed by a *vertex*
   * tile, so `edge_count / chunkSize` is the wrong division and it is wrong quietly: on the
   * conformance corpus it gives ten where there are five, and every URL past the fifth composes
   * cleanly and 404s.
   */
  readonly rows: bigint | null;
  /** `ceil(rows / chunkSize)`, or `null` when no count is declared. */
  readonly tiles: bigint | null;
  /** Which container carries these tiles. The corpus's, never a second answer. */
  readonly container: Container;
  /** The tile of this projection holding `denseId` — its own {@link shift}, never a division. */
  tileOf(denseId: bigint): bigint;
  /**
   * The file tile `j` is in: `<prefix>chunk{j}.parquet` under `files`, `<prefix>tiles.parquet`
   * under `rowgroups` — the same two spellings whatever the scale, because a level is not a
   * different kind of thing from a payload. {@link IndexAddress} is the artefact that spells its
   * files `tile{k}` instead, and it is the one that is not a projection.
   *
   * Under `rowgroups` on an adjacency the row-group ordinal is *not* the tile: an adjacency tile is
   * however many edges its vertices happen to have, and a tile whose vertices have none contributes
   * no row group to be numbered. What locates it is the footer's box on {@link column} over the
   * tile's `dense_id` range.
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
   * A projection the manifest omits, or declares without a `path`, is not here. A corpus that tiles
   * only CSR has `['src']`, and asking it for `dst` returns `null` rather than a string that 404s.
   */
  readonly directions: readonly Direction[];
  /**
   * **Every projection of this relation**: one per orientation at `scale: 1` — the adjacency — and
   * one per written level, source-aligned and carrying both endpoints' coordinates.
   *
   * The same list a vertex type has, told apart by {@link ProjectionAddress.direction}. A relation
   * declaring only its adjacencies is a corpus and not a gap: a reader draws the same edges out of
   * the adjacency and the payload, and only reads more.
   *
   * **What a level of a relation carries that no other projection does is the endpoints'
   * coordinates** — `src_x`, `src_y`, `dst_x`, `dst_y` beside the two ids, which makes it
   * self-drawing. A VERTEX level can position 0.79% of the edges the same view draws, so a pyramid
   * without this one answers a view with links by opening the payload.
   */
  readonly projections: readonly ProjectionAddress[];
  /** The declared orientation's adjacency — its projection at `scale: 1` — or `null`. */
  adjacency(direction: Direction): ProjectionAddress | null;
  /**
   * One projection by scale and orientation, or `null` when the corpus publishes neither.
   *
   * A level is always source-aligned: a level of a relation is *which vertices are in it*, and the
   * source type's own pyramid is what says which.
   */
  projection(scale: number | bigint, direction: Direction): ProjectionAddress | null;
  /**
   * Every file of the projection at `scale` in `direction`, in order and distinct.
   *
   * Throws on a scale nobody wrote by naming the ones that were: the adjacency and the payload are
   * what answer it.
   */
  projectionFiles(scale: number | bigint, direction: Direction): readonly string[];
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
  readonly projections: readonly ProjectionSnapshot[];
}

interface IndexSnapshot {
  readonly prefix: string;
  readonly ordered_by: string;
  readonly chunk_size: bigint;
  readonly tiles: bigint | null;
  readonly container: Container;
}

interface ProjectionSnapshot {
  readonly prefix: string;
  readonly scale: bigint;
  /** Absent on a vertex projection — `serde` skips a `None` rather than writing a null. */
  readonly direction?: Direction;
  readonly column: 'dense_id' | 'src_dense' | 'dst_dense';
  readonly chunk_size: bigint;
  readonly shift: number;
  readonly rows: bigint | null;
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
  readonly projections: readonly ProjectionSnapshot[];
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

/**
 * One projection of the snapshot, as the address a reader composes URLs from.
 *
 * `tileOf` and `tileUrl` cross back to the reader keyed on the projection's own scale rather than
 * arriving with the snapshot, because they are functions of an argument the manifest does not
 * contain. Both are non-null by construction: the only scales this is ever built for are the ones
 * the snapshot declares.
 */
function projectionAddress(
  declared: ProjectionSnapshot,
  tileOf: (scale: bigint, denseId: bigint) => bigint | undefined,
  tileUrl: (scale: bigint, tile: bigint) => string | undefined,
): ProjectionAddress {
  const scale = declared.scale;
  return {
    prefix: declared.prefix,
    scale: Number(scale),
    direction: declared.direction ?? null,
    column: declared.column,
    chunkSize: Number(declared.chunk_size),
    shift: declared.shift,
    rows: declared.rows,
    tiles: declared.tiles,
    container: declared.container,
    // `widths` is OUTSIDE `asked`: a caller handing this a `Number` has made a type error and not
    // written an unaddressable manifest, and the two must not come back as the same class.
    tileOf: (denseId) => {
      const [id] = widths([denseId]);
      return asked(() => tileOf(scale, id!)!);
    },
    tileUrl: (tile) => asked(() => tileUrl(scale, BigInt(tile))!),
  };
}

function vertexAddress(reader: CorpusReader, declared: VertexSnapshot): VertexAddress {
  const type = declared.type;
  const index = declared.index;
  const projections = declared.projections.map((p) =>
    projectionAddress(
      p,
      (scale, denseId) => reader.projectionTileOf(type, scale, denseId),
      (scale, tile) => reader.projectionTileUrl(type, scale, tile),
    ),
  );
  const byScale = new Map(projections.map((p) => [BigInt(p.scale), p]));
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
    projections,
    projection: (scale) => byScale.get(BigInt(scale)) ?? null,
    projectionFiles: (scale) => asked(() => reader.projectionFiles(type, BigInt(scale))),
    // `widths` is OUTSIDE `asked`, for the reason {@link projectionAddress} states.
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
  const projections = declared.projections.map((p) => {
    // Non-null by construction: `plan` writes no edge projection without an `aligned_by`, which is
    // what makes one addressable at all.
    const at = p.direction!;
    return projectionAddress(
      p,
      (scale, denseId) => reader.edgeProjectionTileOf(edgeType, at, scale, denseId),
      (scale, tile) => reader.edgeProjectionTileUrl(edgeType, at, scale, tile),
    );
  });
  const key = (scale: number | bigint, direction: Direction): string => `${direction}@${scale}`;
  const byScale = new Map(projections.map((p) => [key(p.scale, p.direction!), p]));
  const projection = (scale: number | bigint, direction: Direction): ProjectionAddress | null =>
    byScale.get(key(scale, direction)) ?? null;
  return {
    edgeType,
    srcType: declared.src_type,
    dstType: declared.dst_type,
    count: declared.count,
    prefix: declared.prefix,
    directions: declared.directions,
    projections,
    // The adjacency is the projection at `scale: 1` — which is why its files are `chunk{k}` like
    // every other projection rather than a stem of their own.
    adjacency: (direction) => projection(1, direction),
    projection,
    projectionFiles: (scale, direction) =>
      asked(() => reader.edgeProjectionFiles(edgeType, direction, BigInt(scale))),
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
