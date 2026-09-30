/**
 * The address of a corpus, resolved from its manifest — **by the reader, which is in Rust.**
 *
 * A tile is a fixed range of `dense_id` and its address is a shift, so a reader computes every URL
 * it wants before it emits the first request. That arithmetic is `crates/fossil-graph/src/plan.rs`,
 * compiled to wasm32; this module is the shapes its answers arrive in and the calls that ask. There
 * was a second implementation here, in TypeScript, and nothing compared the two.
 *
 * **Internal.** `open` resolves it first and reads through the host's engine; nothing here is on
 * the barrel but `Direction`, `Gap`, `GapReason` and `Channel`, which the door's own answers name.
 * It fetches nothing and caches nothing: `tileUrl` hands back a string.
 */

// '../pkg/fossil_graph_wasm.js' is the wasm-bindgen `--target web` output, gitignored and always
// present at build time — the same import `./load.js` makes, and the reason this module needs the
// module booted before any of it runs.
import { Corpus as CorpusReader } from '../pkg/fossil_graph_wasm.js';

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
 * Which container carries the tiles — `graph.graph.yml`'s own `container`, read off the manifest
 * because working it out means listing a directory, and there is no listing over HTTP.
 *
 * Under **`files`** the file is the tile: `chunk{k}.parquet` holds tile `k` entire. Under
 * **`rowgroups`** a set is one `tiles.parquet` whose row group `k` is tile `k` — except on an
 * adjacency, where a tile whose vertices have no edges contributes no row group, and the range on
 * {@link ProjectionAddress.column} is what locates one.
 */
export type Container = 'files' | 'rowgroups';

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
  /**
   * The tiles holding a batch of `dense_id`s, in order — a batch, because a call per id would pay
   * the boundary once per vertex.
   */
  tilesOf(denseIds: Iterable<bigint>): bigint[];
  /** The file tile `k` is in: `<prefix>chunk{k}.parquet` or `<prefix>tiles.parquet`. */
  tileUrl(tile: number | bigint): string;
  /**
   * Every payload FILE of this type, in order and distinct. Throws when the manifest declares no
   * count, because then there is no set to enumerate.
   */
  files(): readonly string[];
  /**
   * The identity index, or `null` — a legal corpus, where a lookup by identity scans instead: a
   * cost, where an absent {@link VertexAddress.count} is a refusal.
   */
  readonly index: IndexAddress | null;
  /** The zooms below the payload — the cell tree, when the document declares one. */
  readonly cells: CellsAddress | null;
  /** The file rung `k`'s tile `t` is in, or `null` where the type declares no such rung. */
  rungTileUrl(rung: number, tile: number | bigint): string | null;
  /** The file rung `k`'s quotient tile `t` is in, or `null` where the rung publishes none. */
  quotientTileUrl(rung: number, tile: number | bigint): string | null;
  /** Where the tile manifest is, or `null` where the document names none. */
  readonly tileManifest: string | null;
  /** The coordinate system `x` and `y` are in, by name, where declared. */
  readonly coordinates: string | null;
  /** What a view can draw the type with — the document's `channels:`. */
  readonly channels: readonly Channel[];
}

/** One `channels:` entry — what a view can colour or size a type by. */
export interface Channel {
  readonly name: string;
  /** The payload column it reads. */
  readonly column: string;
  readonly scale: 'categorical' | 'quantitative' | string;
  /** A categorical's value count over the writer's ordinal `0..domain`, where declared. */
  readonly domain: number | null;
  readonly derivedBy: string | null;
}

/** A vertex type's cell tree — `fossil_graph::plan::CellsAddress` as it crosses. */
export interface CellsAddress {
  /** The relations a quotient and an internal weight sum, by label. */
  readonly relations: readonly string[];
  /** Which channel a rung's `mode` is the mode of, by name. */
  readonly modeChannel: string | null;
  /** Every rung, finest first. */
  readonly rungs: readonly RungAddress[];
}

/** One rung: `k`, how many bits of `dense_id` a cell drops, and whether it has a quotient. */
export interface RungAddress {
  readonly rung: number;
  /** `B + 2(k − 1)` — a cell id is `dense_id >> shift`. */
  readonly shift: number;
  readonly cellCount: bigint;
  /** The quotient's own edge count, or `null` where the rung publishes none. */
  readonly quotient: { readonly edgeCount: bigint } | null;
}

/**
 * Where a vertex type's identity index lives — a second ORDER over the same rows, sorted by
 * {@link IndexAddress.orderedBy} with disjoint ranges, so a reader prunes its footers to the tiles a
 * key can be in: the thing the payload's footers cannot do for `subject`, whose Hilbert order has
 * nothing to do with its lexicographic one.
 */
export interface IndexAddress {
  /** The column the tiles are sorted by, and the one a lookup is keyed on. */
  readonly orderedBy: string;
  /** Which container carries the index tiles. The corpus's, never a second answer. */
  readonly container: Container;
  /** Every index file, in order and distinct. Throws when the count is absent. */
  files(): readonly string[];
}

/**
 * **One orientation of a relation's adjacency, resolved into an address** —
 * `fossil_graph::plan::ProjectionAddress` at `scale: 1`, as it crosses.
 */
export interface ProjectionAddress {
  /** Where its tiles are, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /** Which endpoint column addresses these tiles. */
  readonly direction: Direction;
  /** The column tile `k` is a range of. */
  readonly column: 'src_dense' | 'dst_dense';
  /**
   * Rows per tile — the ALIGNED endpoint type's, a different space from the other endpoint's on a
   * cross-type edge. Never `edge_count / chunkSize`: an edge tile is addressed by a vertex tile.
   */
  readonly chunkSize: number;
  /** `log2(chunkSize)` — the shift that names a tile, and never a division. */
  readonly shift: number;
  /** Which container carries these tiles. The corpus's, never a second answer. */
  readonly container: Container;
  /** The file tile `j` is in: `<prefix>chunk{j}.parquet` or `<prefix>tiles.parquet`. */
  tileUrl(tile: number | bigint): string;
}

/** One edge type's address, with an entry per orientation the manifest declares. */
export interface EdgeAddress {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  /** The manifest's `edge_count` — one number for both orientations — or `null`. */
  readonly count: bigint | null;
  /** Where the type lives, resolved against the corpus base and with a trailing separator. */
  readonly prefix: string;
  /**
   * The orientations that resolve to an address — never one the manifest does not publish. A
   * corpus that tiles only CSR has `['src']`, and asking it for `dst` is `null`, not a 404.
   */
  readonly directions: readonly Direction[];
  /** The declared orientation's adjacency, or `null`. */
  adjacency(direction: Direction): ProjectionAddress | null;
  /** Every file of the orientation's adjacency, in order and distinct. */
  adjacencyFiles(direction: Direction): readonly string[];
}

/** Why an orientation is missing from an answer. The two reasons are honest; they are not the same. */
export type GapReason =
  /** The manifest does not publish an address for it, so no URL exists to ask for. */
  | 'not-declared'
  /**
   * The relation's other end is a different vertex type, so its far ends are numbered in another
   * `dense_id` space. `edges` declines it unless the call names the relation.
   */
  | 'other-space';

/** One orientation of one edge type that an answer did not read, and why. */
export interface Gap {
  readonly edgeType: string;
  readonly direction: Direction;
  readonly reason: GapReason;
}

/**
 * A corpus resolved to addresses — the layer under the door, which answers with URLs and never
 * reads a byte. Internal: the door reads through the host's engine, and a consumer that wanted a
 * URL has outgrown a surface whose claim is that a corpus is a URL.
 */
export interface CorpusAddressing {
  /** Which container the corpus declares. One answer for every payload set in it. */
  readonly container: Container;
  readonly types: readonly VertexAddress[];
  readonly edges: readonly EdgeAddress[];
  /**
   * **Every file the corpus can address**, distinct and in declaration order — the list a host
   * that lends access file by file hands over whole. `fossil_graph::plan::ReadPlan::files`.
   */
  files(): readonly string[];
  /** One vertex type by name, or the first the index names when no name is given. */
  vertexType(name?: string): VertexAddress;
  /** The edge types incident to `type` — as source, as destination, or both on a self-edge. */
  incident(type: string): readonly EdgeAddress[];
}

// ── what the reader hands back ────────────────────────────────────────────────
//
// `Corpus.snapshot()` serialises `fossil_graph::plan::ReadPlan`, so these are that struct's own
// field names, in `serde`'s spelling — the ones this package reads, and the only place it writes
// them down.

interface PlanSnapshot {
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
  readonly index: { readonly ordered_by: string; readonly container: Container } | null;
  readonly cells: {
    readonly relations: readonly string[];
    readonly mode_channel: string | null;
    readonly rungs: ReadonlyArray<{
      readonly rung: number;
      readonly shift: number;
      readonly cell_count: bigint;
      readonly quotient: { readonly edge_count: bigint } | null;
    }>;
  } | null;
  readonly tile_manifest: string | null;
  readonly coordinates: string | null;
  readonly channels: ReadonlyArray<{
    readonly name: string;
    readonly column: string;
    readonly scale: string;
    readonly domain: bigint | null;
    readonly derived_by: string | null;
  }>;
}

interface ProjectionSnapshot {
  readonly prefix: string;
  readonly scale: bigint;
  /** Absent on a vertex projection — `serde` skips a `None` rather than writing a null. */
  readonly direction?: Direction;
  readonly column: string;
  readonly chunk_size: bigint;
  readonly shift: number;
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
 * One orientation's adjacency out of the snapshot. `tileUrl` crosses back to the reader, because it
 * is a function of an argument the manifest does not contain.
 */
function adjacencyAddress(
  reader: CorpusReader,
  edgeType: string,
  declared: ProjectionSnapshot,
): ProjectionAddress {
  // Non-null by construction: `plan` writes no edge projection without an `aligned_by`, which is
  // what makes one addressable at all.
  const direction = declared.direction!;
  return {
    prefix: declared.prefix,
    direction,
    column: declared.column as 'src_dense' | 'dst_dense',
    chunkSize: Number(declared.chunk_size),
    shift: declared.shift,
    container: declared.container,
    tileUrl: (tile) => asked(() => reader.edgeProjectionTileUrl(edgeType, direction, 1n, BigInt(tile))!),
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
    index:
      index === null
        ? null
        : {
            orderedBy: index.ordered_by,
            container: index.container,
            files: () => asked(() => reader.indexFiles(type)),
          },
    // `widths` is OUTSIDE `asked`: a caller handing this a `Number` has made a type error and not
    // written an unaddressable manifest, and the two must not come back as the same class.
    tilesOf: (denseIds) => {
      const batch = widths(denseIds);
      return asked(() => [...reader.tilesOf(type, batch)]);
    },
    tileUrl: (tile) => asked(() => reader.vertexTileUrl(type, BigInt(tile))),
    files: () => asked(() => reader.vertexFiles(type)),
    cells:
      declared.cells === null
        ? null
        : {
            relations: declared.cells.relations,
            modeChannel: declared.cells.mode_channel,
            rungs: declared.cells.rungs.map((r) => ({
              rung: r.rung,
              shift: r.shift,
              cellCount: r.cell_count,
              quotient: r.quotient === null ? null : { edgeCount: r.quotient.edge_count },
            })),
          },
    rungTileUrl: (rung, tile) => asked(() => reader.rungTileUrl(type, rung, BigInt(tile))) ?? null,
    quotientTileUrl: (rung, tile) =>
      asked(() => reader.quotientTileUrl(type, rung, BigInt(tile))) ?? null,
    tileManifest: declared.tile_manifest,
    coordinates: declared.coordinates,
    channels: declared.channels.map((c) => ({
      name: c.name,
      column: c.column,
      scale: c.scale,
      domain: c.domain === null ? null : Number(c.domain),
      derivedBy: c.derived_by,
    })),
  };
}

function edgeAddress(reader: CorpusReader, declared: EdgeSnapshot): EdgeAddress {
  const edgeType = declared.edge_type;
  const adjacencies = declared.projections
    .filter((p) => p.scale === 1n)
    .map((p) => adjacencyAddress(reader, edgeType, p));
  return {
    edgeType,
    srcType: declared.src_type,
    dstType: declared.dst_type,
    count: declared.count,
    prefix: declared.prefix,
    directions: declared.directions,
    adjacency: (direction) => adjacencies.find((a) => a.direction === direction) ?? null,
    adjacencyFiles: (direction) =>
      asked(() => reader.edgeProjectionFiles(edgeType, direction, 1n)),
  };
}

/**
 * Resolve a corpus's manifest set into the addresses a reader composes URLs from — **the shallow
 * half of `open`, and not a door of its own.**
 *
 * This was `resolveCorpus`, exported beside the door, and then `corpus.addressing`; it is neither
 * now. `./open.ts` is the only caller.
 *
 * **It is synchronous and stays synchronous**, because the boot is the caller's problem one layer
 * up: `open` awaits the boot before it gets here, exactly as it does for a verb.
 *
 * Throws {@link CorpusManifestError} when the manifest cannot address itself — a missing file, a
 * `chunk_size` no shift addresses, an endpoint type the index does not declare, or an edge whose
 * declared tile size disagrees with the vertex type that addresses it. It does **not** throw for an
 * orientation the corpus does not publish: that is a legitimate corpus, and it is reported as an
 * address that does not exist rather than one that 404s.
 */
export function addressManifests(
  manifestFiles: Record<string, string>,
  base = '',
): CorpusAddressing {
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
    container: plan.container,
    types,
    edges,
    files: () => reader.files(),
    vertexType,
    incident: (type) => edges.filter((e) => e.srcType === type || e.dstType === type),
  };
}
