/**
 * The address of a corpus, resolved from its manifest.
 *
 * A tile is a fixed range of `dense_id` and its address is a shift, so a reader computes every URL
 * it wants before it emits the first request. That arithmetic was published as prose and copied
 * into a reader by hand, and nothing compared the two: change a prefix or a shift and the reader
 * composes URLs that 404 at runtime, in a browser, with no type error and no failing test. This
 * module is the copy nobody has to make.
 *
 * **Synchronous, and that is the claim rather than an omission.** It takes no `fetch`, opens no
 * connection and returns no promise. The camera is addressed, not queried — a request between the
 * camera moving and a URL being computable is the `viewport` verb this format deleted. Everything
 * here is a function of the manifest bytes the host already holds.
 *
 * **What it does not do**, and each absence is the seam this package is on the far side of:
 *
 * - **No bytes.** Nothing here fetches, decodes or reads Parquet. `tileUrl` hands back a string.
 * - **No cache, no debounce, no sampling.** Those are the reader's, and they stay there.
 *
 * **Boxes used to be on that list, and the layout pass is what took them off.** It read: *which
 * tiles a rectangle touches comes from the per-tile `x`/`y` statistics in the Parquet footers … the
 * address is the half that cannot be re-derived from the corpus itself.* That was true when it was
 * written and the renumbering falsified it — `dense_id` is a vertex's rank by Morton code, so a
 * rectangle is a set of code ranges and a code range is a run of tiles. {@link mortonTilesFor} is
 * that decomposition, and it is arithmetic: no fetch, no reader, no engine, and no promise.
 *
 * What it needs instead is **one number per tile end** — {@link TileCodes} — because a rank is not
 * a code and no amount of `chunk_size` recovers the one from the other. The corpus publishes it:
 * `vertex/<Type>/codes.json`, named by the manifest and addressed by {@link VertexAddress.codesUrl},
 * parsed by {@link parseTileCodes}. That is 5,498 B on a million-vertex corpus — 1,960 B is what the
 * same 245 pairs pack to, and the difference buys a document with no endianness, no width and no
 * offset table to get wrong — against 1.15 MB of footer at five million. And it buys a *tighter*
 * answer rather than a looser one: 1.00× over-read against the geometric path's 1.13× on the same
 * window. The seam moved; it did not vanish, and it is now the smaller half.
 *
 * `openCorpus` in `./corpus.ts` is the layer that does all three, by taking an engine from the host
 * rather than growing one. It sits **on** this module and does not absorb it: the subpath
 * `@fossil-lang/corpus/address` stays importable with no dependencies and no `query`.
 *
 * @see {@link resolveCorpus}
 */

import {
  CorpusManifestError,
  mapping,
  GRAPH_INFO_PATH,
  join,
  mappings,
  optionalCount,
  paths,
  required,
  requiredNumber,
  scan,
  type ScannedManifest,
} from './manifest.js';

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

/** The payload file of a row-group container: one per set, its row groups the tiles. */
const TILES_FILE = 'tiles.parquet';

/** How many bits a `dense_id` is shifted right by, for the default `chunk_size` of 4,096. */
export const TILE_SHIFT = 12n;

/**
 * The shift that addresses a tile of `rows` rows, or `null` if no shift does.
 *
 * A tile size that is not a power of two forces a division where a shift does, which is why
 * `chunk_size` is a power of two or the corpus does not have an address.
 */
export function shiftFor(rows: number | bigint): bigint | null {
  const n = BigInt(rows);
  if (n <= 0n || (n & (n - 1n)) !== 0n) return null;
  let shift = 0n;
  for (let r = n; r > 1n; r >>= 1n) shift += 1n;
  return shift;
}

/**
 * The tile a `dense_id` lives in — the whole of the addressing scheme.
 *
 * **A `BigInt`, and it refuses a `Number`.** A `dense_id` may carry more than 53 bits, and
 * JavaScript's `>>` truncates to 32 bits *before* it shifts, so the same three characters mean
 * something different here than in the Rust that wrote the corpus. The published border vectors
 * (`apps/corpus/guards/vectors.json`) are 2³¹, where a port that took the shift as signed gives a
 * negative tile, and 2⁵³, where a port that went through a `Number` stops being exact.
 */
export function tileOf(denseId: bigint, shift: bigint = TILE_SHIFT): bigint {
  if (typeof denseId !== 'bigint') {
    throw new TypeError(
      `tileOf takes a BigInt; got ${typeof denseId}. A dense_id carries more bits than a Number ` +
        `can hold, and JavaScript's >> truncates to 32 before it shifts.`,
    );
  }
  if (denseId < 0n) throw new RangeError(`dense_id is unsigned; got ${denseId}`);
  return denseId >> shift;
}

/**
 * How many tiles a declared row count occupies, or `null` when no shift addresses `chunkSize`.
 *
 * **This is the arithmetic the manifest's count exists for**, and until `vertex_count` and
 * `edge_count` became required fields there was nothing to feed it: tiles are addressed and never
 * listed, HTTP gives no directory, and a tree holding `chunk0..chunk16` was indistinguishable from
 * a corpus with seventeen tiles. A hole in the middle breaks the addressing and is caught; a
 * missing tail breaks nothing at all. One `read_text` of the manifest now settles it.
 *
 * `BigInt` for the same reason {@link tileOf} is, and the border is published rather than argued:
 * `apps/corpus/guards/vectors.json`'s `declared_count` table carries 4,096 @ 4,096 → **one** tile
 * (the off-by-one addresses a `chunk1.parquet` nothing wrote), 4,097 → two with a tail of one (the
 * tile a truncated corpus loses), and 2⁵³+1, where a `Number` division comes out one tile short.
 */
export function tilesOf(count: bigint, chunkSize: bigint): bigint | null {
  if (typeof count !== 'bigint') {
    throw new TypeError(
      `tilesOf takes a BigInt count; got ${typeof count}. Above 2^53 a Number loses the tail tile.`,
    );
  }
  if (count < 0n) throw new RangeError(`a row count is unsigned; got ${count}`);
  const shift = shiftFor(chunkSize);
  if (shift === null) return null;
  return (count + chunkSize - 1n) >> shift;
}

/**
 * How many rows the last tile holds — `chunkSize` for a count that divides, the remainder
 * otherwise, and `0` for an empty type, which has no last tile because it has none at all.
 */
export function tailRows(count: bigint, chunkSize: bigint): bigint | null {
  const tiles = tilesOf(count, chunkSize);
  if (tiles === null) return null;
  return tiles === 0n ? 0n : count - (tiles - 1n) * chunkSize;
}

/** One vertex type's address: where its tiles are and which `dense_id` range each holds. */
export interface VertexAddress {
  /** The type label, e.g. `Person`. */
  readonly type: string;
  /** Where its tiles are, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /** Rows per tile. A power of two, checked at resolve. */
  readonly chunkSize: number;
  /** `log2(chunkSize)` — the shift that turns a `dense_id` into a tile number. */
  readonly shift: bigint;
  /** The manifest's `vertex_count`, or `null` when it declares none. */
  readonly count: bigint | null;
  /** `ceil(count / chunkSize)`, or `null` when the manifest declares no count. */
  readonly tiles: bigint | null;
  /** Which container carries this type's tiles. */
  readonly container: Container;
  /** The tile holding `denseId`. */
  tileOf(denseId: bigint): bigint;
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
   * Where this type's **tile-code anchor** is, or `null` when the manifest declares none.
   *
   * The one input {@link mortonTilesFor} needs and cannot derive — see {@link TileCodes}. It is a
   * URL and not a value, because this module does not fetch: the host reads it once, the way it
   * already reads the manifest, and hands the parsed document back through {@link parseTileCodes}.
   *
   * `null` is a legal corpus, on {@link VertexAddress.index}'s argument and not
   * {@link VertexAddress.count}'s: a reader that has a Parquet reader answers the same window
   * question out of the footers, more slowly and more loosely. What it is not is answerable by a
   * reader that has none, and that is the reader this exists for.
   */
  readonly codesUrl: string | null;
}

/**
 * Where a vertex type's identity index lives, and how to address one of its tiles.
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
  /** `<prefix>tile{k}.parquet`, or `<prefix>tiles.parquet` under `rowgroups`. */
  tileUrl(tile: number | bigint): string;
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
  readonly shift: bigint;
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
  tileOf(denseId: bigint): bigint;
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
   * `createGraphClient` already takes. They are small: one index plus one file per type.
   */
  manifestFiles: Record<string, string>;
  /**
   * Where the corpus lives, without a trailing slash — a URL origin and path, a static route, or
   * `''` for addresses relative to the dataset root. Prepended to every URL and nothing else.
   */
  base?: string;
}

/**
 * A corpus resolved to addresses. Every method is pure and synchronous.
 *
 * **It was `ResolvedCorpus`, one import away from `Corpus`, and neither name said what differed.**
 * Both are a corpus; one is the door — asynchronous, engine-backed, answering with rows — and this
 * one is the arithmetic underneath it, which answers with URLs and never reads a byte. So it is
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
   * **It was `window`, and that name belonged to the other layer.** {@link Corpus.window} takes a
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
  /**
   * The URLs a *rectangle* addresses — {@link mortonTilesFor} composed with {@link tilesFor}.
   *
   * This is the method the module header says the subpath stopped one step short of. A camera has
   * a rectangle, not a set of tile numbers, and until this existed the caller had to hold a Parquet
   * reader to get from one to the other. It still holds {@link TileCodes}, which is data — but data
   * three orders of magnitude smaller than a footer and readable by anything that can read a number.
   */
  tilesForBox(
    params: BoxQuery & { type?: string; directions?: readonly Direction[] },
  ): AddressedTiles;
}

const COLUMN: Record<Direction, 'src_dense' | 'dst_dense'> = {
  src: 'src_dense',
  dst: 'dst_dense',
};

/** A prefix as the manifest writes it: dataset-relative, one trailing separator. */
function prefixOf(value: string): string {
  return `${value.replace(/\/+$/, '')}/`;
}

/**
 * Where the tiles of one payload set are, in whichever container the corpus declares.
 *
 * One function for the three sets that had a copy of it each — the vertex payload, the identity
 * index and each adjacency orientation — because the only thing that ever differed between them is
 * the stem of the filename. Under `rowgroups` even that goes: a set is one file.
 */
function tileUrlFor(
  prefix: string,
  stem: 'chunk' | 'tile',
  container: Container,
): (tile: number | bigint) => string {
  return container === 'rowgroups'
    ? () => `${prefix}${TILES_FILE}`
    : (tile) => `${prefix}${stem}${BigInt(tile)}.parquet`;
}

/** Distinct, in order. Under `rowgroups` every tile of a set names the same file. */
const distinct = (urls: readonly string[]): string[] => [...new Set(urls)];

/**
 * Which container the corpus declares, from `graph.graph.yml`.
 *
 * Absent is `files`, and it is the one field here with a default rather than a {@link required}: a
 * corpus written before the field existed is the file-per-tile container, so absence is a statement
 * and not a gap. A third spelling is refused, because it would compose a URL.
 */
function containerOf(index: ScannedManifest): Container {
  const declared = index['container'];
  if (declared === undefined || declared === '') return 'files';
  if (declared !== 'files' && declared !== 'rowgroups') {
    throw new CorpusManifestError(
      `${GRAPH_INFO_PATH} declares container ${String(declared)}; a tile is a file or a row group`,
    );
  }
  return declared;
}

function vertexAddress(
  base: string,
  path: string,
  yaml: ScannedManifest,
  container: Container,
): VertexAddress {
  const type = required(yaml, path, 'type');
  const chunkSize = requiredNumber(yaml, path, 'chunk_size');
  const shift = shiftFor(chunkSize);
  if (shift === null) {
    throw new CorpusManifestError(
      `${path} declares a tile of ${chunkSize} rows, which no shift addresses`,
    );
  }
  const prefix = prefixOf(join(base, required(yaml, path, 'prefix')));
  const count = optionalCount(yaml, 'vertex_count');
  const tiles = count === null ? null : tilesOf(count, BigInt(chunkSize));
  const tileUrl = tileUrlFor(prefix, 'chunk', container);
  return {
    type,
    prefix,
    chunkSize,
    shift,
    count,
    tiles,
    container,
    index: indexAddress(prefix, path, yaml, count, container),
    codesUrl: codesUrlFor(prefix, path, yaml),
    tileOf: (denseId) => tileOf(denseId, shift),
    tileUrl,
    files: () => {
      if (tiles === null) {
        throw new CorpusManifestError(
          `${path} declares no vertex_count, so how many tiles ${type} has is not derivable — ` +
            `tiles are addressed and never listed, and HTTP gives no directory to fall back on`,
        );
      }
      const urls: string[] = [];
      for (let k = 0n; k < tiles; k += 1n) urls.push(tileUrl(k));
      return distinct(urls);
    },
  };
}

/**
 * The `index:` block of a vertex manifest, resolved, or `null` when there is none.
 *
 * Every field is required ONCE the block is present: a `prefix` with no `ordered_by` names files
 * whose sort a reader would have to guess, and guessing it wrong returns a plausible stranger
 * rather than nothing. A half-declared index is refused rather than ignored, because ignoring it
 * would read exactly like a corpus that declares none — which is the failure the scanner already
 * made once, before it could see a nested map at all.
 */
function indexAddress(
  vertexPrefix: string,
  path: string,
  yaml: ScannedManifest,
  count: bigint | null,
  container: Container,
): IndexAddress | null {
  const declared = mapping(yaml, path, 'index');
  if (declared === null) return null;

  const need = (key: string): string => {
    const value = declared[key];
    if (value === undefined || value === '') {
      throw new CorpusManifestError(
        `${path} declares an index and no ${key}, so its tiles address nothing`,
      );
    }
    return value;
  };
  const prefix = prefixOf(join(vertexPrefix, need('prefix')));
  const orderedBy = need('ordered_by');
  const chunkSize = Number(need('chunk_size'));
  if (!Number.isInteger(chunkSize) || chunkSize <= 0) {
    throw new CorpusManifestError(
      `${path} declares an index chunk_size of ${declared.chunk_size}, which is not a row count`,
    );
  }
  const tiles = count === null ? null : tilesOf(count, BigInt(chunkSize));
  const tileUrl = tileUrlFor(prefix, 'tile', container);
  return {
    prefix,
    orderedBy,
    chunkSize,
    tiles,
    container,
    tileUrl,
    files: () => {
      if (tiles === null) {
        throw new CorpusManifestError(
          `${path} declares no vertex_count, so how many index tiles ${required(yaml, path, 'type')} has is not derivable`,
        );
      }
      const urls: string[] = [];
      for (let k = 0n; k < tiles; k += 1n) urls.push(tileUrl(k));
      return distinct(urls);
    },
  };
}

/**
 * The `codes:` block of a vertex manifest, resolved to a URL, or `null` when there is none.
 *
 * One required key, and it is required for {@link indexAddress}'s reason: a block that declares the
 * anchor exists without saying where it is reads exactly like a corpus that declares none, and the
 * difference between those two is what a reader would act on.
 */
function codesUrlFor(vertexPrefix: string, path: string, yaml: ScannedManifest): string | null {
  const declared = mapping(yaml, path, 'codes');
  if (declared === null) return null;
  const relative = declared['path'];
  if (relative === undefined || relative === '') {
    throw new CorpusManifestError(
      `${path} declares codes and no path, so the anchor it names cannot be fetched`,
    );
  }
  return join(vertexPrefix, relative);
}

function edgeAddress(
  base: string,
  path: string,
  yaml: ScannedManifest,
  types: readonly VertexAddress[],
  container: Container,
): EdgeAddress {
  const srcType = required(yaml, path, 'src_type');
  const dstType = required(yaml, path, 'dst_type');
  const edgeType = required(yaml, path, 'edge_type');
  const prefix = prefixOf(join(base, required(yaml, path, 'prefix')));

  const endpoint = (name: string, role: string): VertexAddress => {
    const found = types.find((t) => t.type === name);
    if (!found) {
      throw new CorpusManifestError(
        `${path} names ${role} type ${name}, which the index does not declare`,
      );
    }
    return found;
  };
  const src = endpoint(srcType, 'source');
  const dst = endpoint(dstType, 'destination');

  // An edge tile is addressed by a *vertex* tile, so a different number here would address
  // nothing — and it would address nothing silently, because the URLs still compose and the files
  // they name mostly exist. Checked once, here, rather than trusted in a comment beside a reader.
  const sizes: Array<[string, number, VertexAddress]> = [
    ['src_chunk_size', requiredNumber(yaml, path, 'src_chunk_size'), src],
    ['dst_chunk_size', requiredNumber(yaml, path, 'dst_chunk_size'), dst],
  ];
  for (const [key, declared, vertex] of sizes) {
    if (declared !== vertex.chunkSize) {
      throw new CorpusManifestError(
        `${path} declares ${key} ${declared} against ${vertex.type}'s chunk_size ${vertex.chunkSize}, ` +
          `so its tiles address nothing`,
      );
    }
  }
  const chunkSize = requiredNumber(yaml, path, 'chunk_size');
  if (chunkSize !== sizes[0]![1]) {
    throw new CorpusManifestError(
      `${path} declares chunk_size ${chunkSize} and src_chunk_size ${sizes[0]![1]}`,
    );
  }

  const declared = new Map<Direction, AdjacencyAddress>();
  for (const entry of mappings(yaml, 'adj_lists')) {
    const alignedBy = entry.aligned_by;
    if (alignedBy !== 'src' && alignedBy !== 'dst') continue;
    // The one part of a tile's URL a reader cannot compute. An orientation declared without it has
    // tiles nobody can address, so it is not an address and does not become one here.
    const adjPrefix = (entry.prefix ?? '').replace(/\/+$/, '');
    if (adjPrefix === '') continue;
    const vertex = alignedBy === 'src' ? src : dst;
    const tilePrefix = prefixOf(join(prefix, adjPrefix));
    declared.set(alignedBy, {
      direction: alignedBy,
      prefix: tilePrefix,
      column: COLUMN[alignedBy],
      chunkSize: vertex.chunkSize,
      shift: vertex.shift,
      tiles: vertex.tiles,
      container,
      tileOf: (denseId) => tileOf(denseId, vertex.shift),
      tileUrl: tileUrlFor(tilePrefix, 'tile', container),
    });
  }

  return {
    edgeType,
    srcType,
    dstType,
    count: optionalCount(yaml, 'edge_count'),
    prefix,
    directions: (['src', 'dst'] as const).filter((d) => declared.has(d)),
    adjacency: (direction) => declared.get(direction) ?? null,
  };
}

/**
 * Resolve a corpus's manifest set into the addresses a reader composes URLs from.
 *
 * ```ts
 * const corpus = resolveCorpus({ manifestFiles, base: '/bench/1000000' });
 * const person = corpus.vertexType();
 * const { edgeUrls, complete, gaps } = corpus.tilesFor({ tiles: [3, 4], directions: ['src', 'dst'] });
 * ```
 *
 * Throws {@link CorpusManifestError} when the manifest cannot address itself — a missing file, a
 * `chunk_size` no shift addresses, an endpoint type the index does not declare, or an edge whose
 * declared tile size disagrees with the vertex type that addresses it. It does **not** throw for an
 * orientation the corpus does not publish: that is a legitimate corpus, and it is reported as an
 * address that does not exist rather than one that 404s.
 */
export function resolveCorpus(options: ResolveCorpusOptions): CorpusAddressing {
  const { manifestFiles, base = '' } = options;

  const read = (path: string): ScannedManifest => {
    const text = manifestFiles[path];
    if (text === undefined) {
      throw new CorpusManifestError(
        `the manifest names ${path}, which is not among the ${Object.keys(manifestFiles).length} ` +
          `file(s) given`,
      );
    }
    return scan(path, text);
  };

  const index = read(GRAPH_INFO_PATH);
  // `prefix` on the index is what the per-type paths are relative to; it is `''` in every corpus
  // fossil writes, and honoured rather than assumed because the field exists to be set.
  const root = join(base, typeof index['prefix'] === 'string' ? index['prefix'] : '');
  const container = containerOf(index);

  const types = paths(index, 'vertices').map((path) =>
    vertexAddress(root, path, read(path), container),
  );
  if (types.length === 0) {
    throw new CorpusManifestError(`${GRAPH_INFO_PATH} names no vertex type`);
  }
  const edges = paths(index, 'edges').map((path) =>
    edgeAddress(root, path, read(path), types, container),
  );

  const vertexType = (name?: string): VertexAddress => {
    if (name === undefined) return types[0]!;
    const found = types.find((t) => t.type === name);
    if (!found) {
      throw new CorpusManifestError(
        `no vertex type ${name} in ${GRAPH_INFO_PATH}; it names ${types.map((t) => t.type).join(', ')}`,
      );
    }
    return found;
  };

  const incident = (type: string): readonly EdgeAddress[] =>
    edges.filter((e) => e.srcType === type || e.dstType === type);

  const tilesFor: CorpusAddressing['tilesFor'] = ({ type, tiles, directions = ['src'] }) => {
    const vertex = vertexType(type);
    const wanted = new Set(directions);
    const numbers = [...tiles].map(Number);
    const edgeTiles: EdgeTiles[] = [];
    const gaps: Gap[] = [];

    for (const edge of incident(vertex.type)) {
      // Only the orientations whose *own* `dense_id` space is this window's. On a cross-type edge
      // `by_target` tile k addresses tile k of the destination type, which is a different set of
      // vertices — reading it for a window over the source type would answer a question nobody
      // asked and call it the neighbourhood.
      const applicable: Direction[] = [];
      if (edge.srcType === vertex.type) applicable.push('src');
      if (edge.dstType === vertex.type) applicable.push('dst');

      for (const direction of applicable) {
        const adjacency = edge.adjacency(direction);
        if (adjacency === null) {
          gaps.push({ edgeType: edge.edgeType, direction, reason: 'not-declared' });
          continue;
        }
        if (!wanted.has(direction)) {
          gaps.push({ edgeType: edge.edgeType, direction, reason: 'not-requested' });
          continue;
        }
        edgeTiles.push({
          edgeType: edge.edgeType,
          direction,
          urls: distinct(numbers.map((k) => adjacency.tileUrl(k))),
        });
      }
    }

    return {
      type: vertex.type,
      tiles: numbers,
      vertexUrls: distinct(numbers.map((k) => vertex.tileUrl(k))),
      edges: edgeTiles,
      edgeUrls: distinct(edgeTiles.flatMap((e) => e.urls)),
      complete: gaps.length === 0,
      gaps,
    };
  };

  return {
    base,
    container,
    types,
    edges,
    vertexType,
    incident,
    tilesFor,
    tilesForBox: ({ box, extent, codes, type, directions }) =>
      tilesFor({ type, tiles: mortonTilesFor({ box, extent, codes }), directions }),
  };
}

// ---------------------------------------------------------------------------
// The Z-order half: which tiles a rectangle touches, without a reader
// ---------------------------------------------------------------------------

/**
 * Bits per axis on the grid the layout pass quantises positions onto, so a code is a `u32`.
 *
 * The writer is `morton_codes` in `crates/fossil-layout/src/layout.rs` and the published second
 * implementation is `apps/corpus/guards/arithmetic.mjs`; this is the third, and the three agree
 * against `apps/corpus/guards/vectors.json` rather than against each other.
 */
export const MORTON_BITS = 16;

/** Cells per axis — `1 << MORTON_BITS`. */
export const MORTON_SIDE = 1 << MORTON_BITS;

/** A rectangle in the corpus's own coordinates: the camera's question, and the layout's extent. */
export interface Box {
  xlo: number;
  xhi: number;
  ylo: number;
  yhi: number;
}

/** The same rectangle on the Morton grid — `0..65535` per axis, both ends inclusive. */
export interface GridBox {
  xlo: number;
  xhi: number;
  ylo: number;
  yhi: number;
}

/**
 * Quantise one coordinate onto `0..65535` over the extent it was ranked within.
 *
 * **Every step is binary32**, hence the `Math.fround` on each one. The writer's positions, extent
 * and intermediate ratio are all `f32`; JavaScript's arithmetic is binary64, so a literal
 * transcription of the formula is a *different function* — `quantize(147, 0, 167)` is 57687 in
 * binary32 and 57686 in binary64, and one unit here is a different code, a different rank, a
 * different `dense_id` and a different tile. `vectors.json` carries that case.
 */
export function quantize(v: number, lo: number, hi: number): number {
  if (!(hi > lo)) return 0;
  const f = Math.fround;
  const t = Math.min(Math.max(f(f(f(v) - f(lo)) / f(f(hi) - f(lo))), 0), 1);
  return Math.round(f(t * 65535));
}

/** Spread the low 16 bits of `n` into the even bit positions of a `u32`. */
function spread(n: number): number {
  let v = n & 0xffff;
  v = (v | (v << 8)) & 0x00ff00ff;
  v = (v | (v << 4)) & 0x0f0f0f0f;
  v = (v | (v << 2)) & 0x33333333;
  v = (v | (v << 1)) & 0x55555555;
  return v >>> 0;
}

/**
 * Interleave two quantised coordinates into a 32-bit Z-order code — `x` in the even bits.
 *
 * `spread(y) << 1` reaches bit 31, so the result is a negative `Number` unless coerced back to
 * unsigned. That coercion is not a detail: without it the top half of the plane sorts before the
 * bottom half, and the corpus satisfies every count-based check while addressing nothing.
 */
export function morton2(x: number, y: number): number {
  return (spread(x) | (spread(y) << 1)) >>> 0;
}

/** The Morton code of a position within an extent — quantise, then interleave. */
export function mortonOf(x: number, y: number, extent: Box): number {
  return morton2(quantize(x, extent.xlo, extent.xhi), quantize(y, extent.ylo, extent.yhi));
}

/** Split a code back into the two grid coordinates {@link morton2} interleaved. */
export function mortonDecode(code: number): { x: number; y: number } {
  let x = 0;
  let y = 0;
  for (let bit = 0; bit < MORTON_BITS; bit += 1) {
    x |= ((code >>> (2 * bit)) & 1) << bit;
    y |= ((code >>> (2 * bit + 1)) & 1) << bit;
  }
  return { x, y };
}

/**
 * A rectangle onto the grid — every vertex inside `box` is in a cell inside the answer.
 *
 * `quantize` is monotone non-decreasing, so `v >= box.xlo` implies `quantize(v) >= quantize(xlo)`
 * and the containment is exact rather than padded. `null` when the rectangle and the extent are
 * disjoint, which is a real answer and not an empty one: `quantize` CLAMPS, so a box entirely left
 * of the extent would otherwise come back as the column of cells at `x = 0`.
 */
export function gridBoxOf(box: Box, extent: Box): GridBox | null {
  if (box.xhi < box.xlo || box.yhi < box.ylo) return null;
  if (box.xhi < extent.xlo || box.xlo > extent.xhi) return null;
  if (box.yhi < extent.ylo || box.ylo > extent.yhi) return null;
  return {
    xlo: quantize(box.xlo, extent.xlo, extent.xhi),
    xhi: quantize(box.xhi, extent.xlo, extent.xhi),
    ylo: quantize(box.ylo, extent.ylo, extent.yhi),
    yhi: quantize(box.yhi, extent.ylo, extent.yhi),
  };
}

/**
 * Which Morton codes each tile holds — **the one thing the arithmetic does not derive.**
 *
 * `dense_id` is renumbered into Morton order, so a vertex's address and its position are the same
 * number read two ways and a rectangle is a set of code ranges. What that does NOT give is where
 * one range falls in the *ranking*: `dense_id` is a vertex's RANK by code, not its code, and the
 * rank of a code is a function of how the positions are distributed. `chunk_size` alone cannot say
 * it, and assuming the ranks are uniform in the code space is not a conservative guess — measured
 * over nine windows of the million-vertex fixture it MISSES 129 of the 226 tiles that hold a
 * matching vertex. So this is data, and it is two `u32` per tile: 1,960 B at a million vertices,
 * against the 1.15 MB of Parquet footer the geometric path reads at five million.
 *
 * `lo[k]` is the lowest code in tile `k` and `hi[k]` the highest — the codes of its first and last
 * rows, because the rows within a tile are in code order too. Both are non-decreasing in `k`.
 */
export interface TileCodes {
  readonly lo: ArrayLike<number>;
  readonly hi: ArrayLike<number>;
  /**
   * The box the codes were quantised against, when the anchor carries one.
   *
   * The **other** half of what a rectangle needs, and the half that was quietly assumed: a code is
   * `quantize(x, xlo, xhi)` interleaved with `quantize(y, ylo, yhi)`, so a rectangle in the
   * corpus's own coordinates is a rectangle on the grid only once this is known. It is derivable —
   * it is the min and max of the `x` and `y` columns — but deriving it means opening every tile's
   * footer, which is the reader the arithmetic path exists without. So the writer publishes it
   * beside the codes, in the same document, and {@link mortonTilesFor} takes it from there when the
   * caller does not pass one.
   */
  readonly extent?: Box;
}

/**
 * The **tile-code anchor** as the writer publishes it: `vertex/<Type>/codes.json`, named by the
 * `codes:` block of the type's manifest and addressed by {@link VertexAddress.codesUrl}.
 *
 * It is JSON and not a packed array of `u32` for the reason the rest of this module is arithmetic:
 * a reader should need nothing it does not already have. Packed is 1,960 B at a million vertices
 * against about 5.4 kB here, and both are two orders of magnitude under the 226 kB of Parquet
 * footer that answers the same question — what the extra 3.4 kB buys is that there is no
 * endianness, no width and no offset table to get wrong, in any language.
 */
export interface TileCodesDocument extends TileCodes {
  /** Bits per axis on the grid the writer quantised onto. Must be {@link MORTON_BITS}. */
  readonly mortonBits: number;
  /** Rows per tile the anchor was cut at — the type's own `chunk_size`. */
  readonly chunkSize: number;
  /** How many tiles it names. `lo.length`, `hi.length`, and the manifest's own count agree. */
  readonly tiles: number;
  readonly lo: readonly number[];
  readonly hi: readonly number[];
  readonly extent: Box;
}

/** A number that is a `u32`, which every code and every tile count in the anchor is. */
function u32(value: unknown, where: string, what: string): number {
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 0xffffffff) {
    throw new CorpusManifestError(`${where}: ${what} is ${String(value)}, which is not a u32`);
  }
  return value;
}

/**
 * Parse a tile-code anchor. **Synchronous, and it validates rather than trusts.**
 *
 * The host fetches {@link VertexAddress.codesUrl} the way it already fetches the manifest and hands
 * the text here; this module still opens nothing. What comes back satisfies {@link TileCodes}, so
 * it goes straight into {@link mortonTilesFor} and {@link CorpusAddressing.tilesForBox}.
 *
 * Four things are checked, and each one is a wrong picture rather than an exception if it is not:
 *
 * - **`morton_bits` is this module's.** A corpus quantised onto a different grid is addressed by
 *   different arithmetic, and every function here would answer confidently and wrongly.
 * - **`lo` and `hi` are the same length**, and it is `tiles`. A pair per tile is the whole shape.
 * - **`lo[k] <= hi[k]`**, and **both arrays are non-decreasing**. {@link tilesForGrid} answers with
 *   two binary searches, and a binary search over an unsorted array does not fail, it misses.
 * - **The extent is four finite numbers.** A `null` — which is what a non-finite `f32` becomes in
 *   JSON — would quantise every code to zero.
 */
export function parseTileCodes(text: string, where = 'the tile-code anchor'): TileCodesDocument {
  let raw: unknown;
  try {
    raw = JSON.parse(text) as unknown;
  } catch (cause) {
    throw new CorpusManifestError(`${where} is not JSON: ${String(cause)}`);
  }
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) {
    throw new CorpusManifestError(`${where} is not an object`);
  }
  const doc = raw as Record<string, unknown>;

  const mortonBits = u32(doc['morton_bits'], where, 'morton_bits');
  if (mortonBits !== MORTON_BITS) {
    throw new CorpusManifestError(
      `${where} declares ${mortonBits} bits per axis and this reader addresses ${MORTON_BITS}; ` +
        `a different grid is different arithmetic, not a smaller one`,
    );
  }
  const chunkSize = u32(doc['chunk_size'], where, 'chunk_size');
  const tiles = u32(doc['tiles'], where, 'tiles');

  const codes = (key: 'lo' | 'hi'): number[] => {
    const value = doc[key];
    if (!Array.isArray(value)) {
      throw new CorpusManifestError(`${where}: ${key} is not an array`);
    }
    if (value.length !== tiles) {
      throw new CorpusManifestError(
        `${where} declares ${tiles} tile(s) and ${value.length} ${key} code(s)`,
      );
    }
    return value.map((v, k) => u32(v, where, `${key}[${k}]`));
  };
  const lo = codes('lo');
  const hi = codes('hi');
  for (let k = 0; k < tiles; k += 1) {
    if (lo[k]! > hi[k]!) {
      throw new CorpusManifestError(`${where}: tile ${k} spans ${lo[k]}..${hi[k]}, backwards`);
    }
    if (k > 0 && (lo[k - 1]! > lo[k]! || hi[k - 1]! > hi[k]!)) {
      throw new CorpusManifestError(
        `${where}: tile ${k} does not follow tile ${k - 1} in code order, so no binary search ` +
          `over these arrays finds it`,
      );
    }
  }

  const box = doc['extent'];
  if (box === null || typeof box !== 'object' || Array.isArray(box)) {
    throw new CorpusManifestError(`${where} publishes no extent, so no rectangle reaches the grid`);
  }
  const side = (key: keyof Box): number => {
    const value = (box as Record<string, unknown>)[key];
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      throw new CorpusManifestError(
        `${where}: extent.${key} is ${String(value)}, which is not a finite coordinate`,
      );
    }
    return value;
  };
  const extent: Box = { xlo: side('xlo'), xhi: side('xhi'), ylo: side('ylo'), yhi: side('yhi') };

  return { mortonBits, chunkSize, tiles, lo, hi, extent };
}

/** Smallest tile whose codes can reach `code`, or `codes.lo.length` when none can. */
function firstTile(codes: TileCodes, code: number): number {
  let a = 0;
  let b = codes.hi.length - 1;
  let found = codes.hi.length;
  while (a <= b) {
    const mid = (a + b) >> 1;
    if (codes.hi[mid]! >= code) {
      found = mid;
      b = mid - 1;
    } else a = mid + 1;
  }
  return found;
}

/** Largest tile whose codes can reach down to `code`, or `-1` when none can. */
function lastTile(codes: TileCodes, code: number): number {
  let a = 0;
  let b = codes.lo.length - 1;
  let found = -1;
  while (a <= b) {
    const mid = (a + b) >> 1;
    if (codes.lo[mid]! <= code) {
      found = mid;
      a = mid + 1;
    } else b = mid - 1;
  }
  return found;
}

/**
 * The tiles a grid rectangle touches: descend the quadtree, stop where the answer stops changing.
 *
 * A node of the Z-order tree is a code range `[base, base + 4^level)` AND a square of cells, which
 * is what makes the descent possible at all — one test on the square says whether the node is
 * outside the rectangle, inside it, or on its edge, and two binary searches over {@link TileCodes}
 * say which tiles the range falls in. The recursion stops on any of three answers rather than on
 * depth: outside (nothing), inside (every tile the range touches), or **the range is within one
 * tile** — refining a node that cannot split the answer costs work and buys nothing.
 *
 * The result **over-covers**, because the curve enters and leaves the rectangle: a tile whose code
 * range straddles the boundary is named whole. Measured against the tiles that actually hold a
 * matching vertex, over nine windows of the million-vertex fixture, that over-read is **1.00×** —
 * the geometric path over the same windows is 1.41×, and 1.13× on the 10% one the design page
 * records. A code range is a tighter description of a tile than its `x`/`y` box: the box is the
 * hull of an arc that snakes, and the arc is what the vertices are on.
 */
export function tilesForGrid(grid: GridBox, codes: TileCodes): number[] {
  const tiles = codes.lo.length;
  if (tiles === 0 || codes.hi.length !== tiles) return [];
  const selected = new Set<number>();
  const stack: Array<[number, number]> = [[0, MORTON_BITS]];
  while (stack.length > 0) {
    const [base, level] = stack.pop()!;
    const side = 2 ** level;
    const { x, y } = mortonDecode(base);
    const x1 = x + side - 1;
    const y1 = y + side - 1;
    if (x1 < grid.xlo || x > grid.xhi || y1 < grid.ylo || y > grid.yhi) continue;
    const span = 4 ** level;
    const first = firstTile(codes, base);
    const last = lastTile(codes, base + span - 1);
    // A gap in the ranking: no tile holds a code in this range, so no vertex does either.
    if (first > last) continue;
    const inside = x >= grid.xlo && x1 <= grid.xhi && y >= grid.ylo && y1 <= grid.yhi;
    if (inside || first === last || level === 0) {
      for (let k = first; k <= last; k += 1) selected.add(k);
      continue;
    }
    const quarter = span / 4;
    for (let i = 0; i < 4; i += 1) stack.push([base + i * quarter, level - 1]);
  }
  return [...selected].sort((a, b) => a - b);
}

/** What {@link mortonTilesFor} and {@link CorpusAddressing.tilesForBox} take. */
export interface BoxQuery {
  /** The rectangle, in the corpus's own coordinates. */
  box: Box;
  /**
   * The extent the type's positions were quantised against — its own bounding box.
   *
   * **Optional since the anchor publishes it.** It was required and had to be, because the only
   * way to obtain it was to scan every tile's `x`/`y`; a {@link TileCodesDocument} carries it, so
   * the ordinary call passes `codes` alone. An explicit one still wins, which is what
   * `address-morton.test.ts` builds its own renumberings with.
   */
  extent?: Box;
  /** Which codes each tile holds. See {@link TileCodes} for why this is not derivable. */
  codes: TileCodes;
}

/** The tiles a rectangle in corpus coordinates touches. Synchronous, and it reads no byte. */
export function mortonTilesFor({ box, extent, codes }: BoxQuery): number[] {
  const against = extent ?? codes.extent;
  if (against === undefined) {
    throw new CorpusManifestError(
      `a rectangle reaches the Morton grid through the extent its codes were quantised against, ` +
        `and neither the call nor the anchor carries one`,
    );
  }
  const grid = gridBoxOf(box, against);
  return grid === null ? [] : tilesForGrid(grid, codes);
}

// ---------------------------------------------------------------------------
// The other half of a camera's question: which LEVEL, and how coarse that is
// ---------------------------------------------------------------------------

/**
 * The decimation level *k* means: **the vertices whose `dense_id` is a multiple of 2^k**.
 *
 * Over a Morton-ordered `dense_id` that is one vertex per quadtree cell of depth *k*, so a level is
 * a level of the same curve the tile address is read off — not a sample, not a budget, not a cap.
 * Two consequences follow from the definition alone and neither needs a byte on disk:
 *
 * - **It nests.** `dense_id % 2^(k+1) == 0` is a strict subset of `dense_id % 2^k == 0`, so
 *   refining only ever ADDS, and a vertex drawn once stays drawn at the same position.
 * - **It is a function of the level and nothing else.** No `matched`, no `limit`, no camera. Which
 *   is what makes {@link levelFor} a *separate* function rather than a step inside the read.
 */
export const strideOf = (level: number): number => 2 ** Math.max(0, Math.trunc(level));

/** What {@link levelFor} takes: a rectangle, what it may cost, and the anchor that sizes it. */
export interface LevelQuery extends BoxQuery {
  /**
   * How many vertices the caller is willing to be handed. **A budget, not a cap** — the answer is
   * a level, and a level's population is whatever the rectangle holds at that level.
   */
  budget: number;
  /** Rows per tile. Taken from the anchor when it carries one, as `codes.json` does. */
  chunkSize?: number;
  /** The type's `vertex_count`, when known — the ceiling on any estimate. */
  count?: number;
}

/**
 * The coarsest level whose population fits a budget — **pure, synchronous, and nobody's session.**
 *
 * This is deliberately not a step inside the read, and three arguments say why.
 *
 * - **Zarr decides the level on the client.** Its multiscale metadata says which levels exist and
 *   the reader picks; the store answers for the one it is asked about. Copying the *shape* of that
 *   is the point of naming Zarr at all.
 * - **"The same rectangle at the same level" has to be expressible.** It is the statement monotone
 *   refinement is *about* — `{drawn closer} ⊇ {drawn farther} ∩ {new window}` compares two levels
 *   over one rectangle — and a reader that chose its own level inside the read could not be asked
 *   the question. The decision and the test are the same decision.
 * - **A budget is made of pixels.** Screen size, device ratio and what a person finds legible are
 *   the host's facts and none of them is a corpus's business. They stop at this function's
 *   argument list.
 *
 * And it exists at all rather than being left to each consumer, because the arithmetic below is
 * exactly the kind that gets re-derived differently in every reader — which is the failure this
 * module was written against.
 *
 * **The estimate is the tiles, not the area.** A rectangle covering 9% of the extent of the
 * million-vertex fixture holds 25% of its vertices, because a layout clusters; area is off by 2.8×
 * there, which is a level and a half. Tiles are not: `tiles × chunk_size` over-counts the same
 * rectangle by 1.57×, under one level, and it is *monotone in the rectangle*, which area is too but
 * uniformity is not. `count` caps it, because a rectangle covering the corpus cannot hold more than
 * the corpus.
 */
export function levelFor({ budget, chunkSize, count, ...box }: LevelQuery): number {
  const rows = chunkSize ?? (box.codes as TileCodes & { chunkSize?: number }).chunkSize ?? 0;
  const tiles = mortonTilesFor(box).length;
  const estimate = Math.min(tiles * rows || 0, count ?? Number.POSITIVE_INFINITY);
  if (!(budget > 0) || !(estimate > budget)) return 0;
  return Math.ceil(Math.log2(estimate / budget));
}
