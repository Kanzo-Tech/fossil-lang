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
 * - **No boxes.** Which tiles a rectangle touches comes from the per-tile `x`/`y` statistics in the
 *   Parquet footers, and reading a footer needs a Parquet reader. The host has one; this package
 *   would have to grow one, and the address is the half that cannot be re-derived from the corpus
 *   itself. So the host reads its own footers and hands the tile numbers back here.
 * - **No cache, no debounce, no sampling.** Those are the reader's, and they stay there.
 *
 * `openCorpus` in `./corpus.ts` is the layer that does all three, by taking an engine from the host
 * rather than growing one. It sits **on** this module and does not absorb it: the subpath
 * `@fossil-lang/graph/address` stays importable with no dependencies and no `query`.
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
export interface Window {
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

/** A corpus resolved to addresses. Every method is pure and synchronous. */
export interface ResolvedCorpus {
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
   * every vertex in the window and returns `complete: false` with a `not-requested` gap, because a
   * window of drawn vertices has in-edges it did not ask for. Pass `['src', 'dst']` for the
   * incident set.
   */
  window(params: {
    type?: string;
    tiles: Iterable<number | bigint>;
    directions?: readonly Direction[];
  }): Window;
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
 * const { edgeUrls, complete, gaps } = corpus.window({ tiles: [3, 4], directions: ['src', 'dst'] });
 * ```
 *
 * Throws {@link CorpusManifestError} when the manifest cannot address itself — a missing file, a
 * `chunk_size` no shift addresses, an endpoint type the index does not declare, or an edge whose
 * declared tile size disagrees with the vertex type that addresses it. It does **not** throw for an
 * orientation the corpus does not publish: that is a legitimate corpus, and it is reported as an
 * address that does not exist rather than one that 404s.
 */
export function resolveCorpus(options: ResolveCorpusOptions): ResolvedCorpus {
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

  return {
    base,
    container,
    types,
    edges,
    vertexType,
    incident,

    window({ type, tiles, directions = ['src'] }) {
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
    },
  };
}
