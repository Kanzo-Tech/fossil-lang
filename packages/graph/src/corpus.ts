/**
 * A corpus, opened from a URL — the reference API, with the addressing underneath and invisible.
 *
 * `resolveCorpus` is not this. It is the addressing layer: it returns URLs and leaves the consumer
 * knowing what a tile is, which container carries one, how to ask for footers and how to join CSR
 * with CSC. That is exactly the knowledge the handover asked not to need. Here there are no tiles,
 * no `dense_id`, no Morton, no `by_source`, no prefixes and no footers in the caller's face:
 *
 * ```ts
 * const corpus = await openCorpus(url, { query });
 * corpus.types                                  // what is inside
 * await corpus.window({ x, y, w, h })           // vertices + edges, and whether that is all of them
 * await corpus.node(iri)
 * await corpus.neighbours([iri], { depth: 2 })
 * ```
 *
 * **The one thing a host brings is an engine.** See `./query.ts` for why the capability is a single
 * `query` callback and not a bundled Parquet decoder: this package still has zero runtime
 * dependencies, and DuckDB's own footer pruning is the half of a windowed read nobody has to write.
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
 * - **Which container** — **not** absorbed. This reads the file-per-tile container, refuses the
 *   other by name and never globs. See {@link openCorpus}.
 *
 * And one shape of corpus it refuses rather than guesses at: **an edge label incident twice to one
 * vertex type**, which the addressing's `Window` cannot name unambiguously. See {@link Corpus.window}.
 */

import {
  type AdjacencyAddress,
  CorpusManifestError,
  type Direction,
  type EdgeTiles,
  type Gap,
  GRAPH_INFO_PATH,
  resolveCorpus,
  type ResolvedCorpus,
  type VertexAddress,
} from './address.js';
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

/** One vertex, as an answer carries it. */
export interface CorpusVertex {
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
 * construction. Join them against {@link CorpusVertex.denseId}, which is in the same space, and
 * never store one: {@link Corpus.node} says why.
 */
export interface CorpusEdge {
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
   * incidence — `resolveCorpus.window` defaults to `['src']` instead, because that is the drawing
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
  readonly vertices: readonly CorpusVertex[];
  readonly edges: readonly CorpusEdge[];
}

/** What {@link Corpus.neighbours} took and what it reached. */
export interface Neighbourhood extends Answer {
  readonly type: string;
  readonly depth: number;
  /** The identities that resolved to a vertex, and the ones that did not. */
  readonly seeds: readonly string[];
  readonly missing: readonly string[];
  /** Every vertex reached, the seeds included. */
  readonly vertices: readonly CorpusVertex[];
  readonly edges: readonly CorpusEdge[];
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
  /** What is inside. */
  readonly types: CorpusTypes;
  /**
   * The addressing underneath, for a caller that has outgrown this surface — a drawing path that
   * wants tile URLs to fetch itself, for instance. Nothing here needs it.
   */
  readonly addressing: ResolvedCorpus;
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
  /** One vertex by identity, or `null`. */
  node(id: string, params?: NodeParams): Promise<CorpusVertex | null>;
  /** Everything within `depth` hops of a set of identities. */
  neighbours(ids: Iterable<string>, params?: NeighboursParams): Promise<Neighbourhood>;
}

/** What {@link openCorpus} takes. */
export interface OpenCorpusOptions {
  /** The host's engine. One method, and see `./query.ts` for why it is the only one. */
  query: QueryFn;
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

/** A quoted SQL identifier. Column names come from the payload, so they are not interpolated raw. */
function ident(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
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
 * `packages/graph`'s own test fixture declares three of which one is `dense_id`. The manifest's
 * property list is a promise; the payload is the artefact, and this reads the artefact. The cost is
 * one round trip per vertex type at open.
 *
 * **Which container, and this one is not absorbed.** A tile is a range of rows; whether it is a
 * file (`chunk{k}.parquet`) or a row group inside one file is a second question, **and no manifest
 * field distinguishes them**. This reads the file-per-tile container, because that is the one with
 * a per-tile URL to compose and the one the conformance corpus is. Handed the other, `read_parquet`
 * over the derived URL list fails on the first name — `chunk0.parquet` is not there — and the error
 * names the file. It does not fall back and it does not glob: a reader that globbed would pick up
 * the staged single-file copy beside the tiles and count every row twice. The measured winner is
 * the row-group container (5.6 requests per window against 22.3, and 496 kB of footer against
 * 1.15 MB, at five million vertices), which is the container fossil does not write and this cannot
 * address until a field says which one it is looking at.
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

  // Every vertex type's tile set, derived once. `tileUrls` throws when the manifest declares no
  // count, which is the corpus this API cannot open and the addressing layer still can.
  const tileUrls = new Map<string, readonly string[]>();
  const columns = new Map<string, readonly CorpusField[]>();
  for (const type of addressing.types) {
    tileUrls.set(type.type, type.tileUrls());
    const first = tileUrls.get(type.type)![0];
    let described: QueryRow[] = [];
    if (first !== undefined) {
      try {
        described = await query(`DESCRIBE SELECT * FROM read_parquet(${lit(first)})`);
      } catch (cause) {
        // The container question, at the one place it bites. This does not diagnose the failure —
        // a truncated corpus reaches here too — it states what was addressed and why nothing else
        // was tried, because the tempting recovery is a glob and a glob is wrong: it would pick up
        // a staged single-file copy beside the tiles and count every row twice.
        throw new CorpusReadError(
          `${first} is the first tile of ${type.type} and it did not open. A tile is a range of ` +
            `rows, and whether one is a file or a row group inside a single file is the container ` +
            `question — no manifest field distinguishes them, so this reads the file-per-tile ` +
            `container and neither falls back nor globs. (${(cause as Error).message})`,
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
      // `tileUrls()` above threw if this were absent: a corpus whose extent is not derivable is the
      // one this API refuses to open, and it is also the only thing the manifest's count is for.
      count: type.count!,
      fields: fieldsOf(type.type),
      identity: has(type.type, IDENTITY) ? IDENTITY : null,
      geometry: has(type.type, 'x') && has(type.type, 'y'),
    })),
    edges: addressing.edges.map((edge) => ({
      edgeType: edge.edgeType,
      srcType: edge.srcType,
      dstType: edge.dstType,
      count: edge.count,
      directions: edge.directions,
    })),
  };

  const extents = new Map<string, Extent | null>();

  const vertexOf = (type: string, row: QueryRow): CorpusVertex => {
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
   * The addressing's `Window` names an orientation by edge *label*, so a corpus where two edge
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

  const boxOf = ({ x, y, w, h }: Box): string =>
    `x >= ${x} AND x < ${x + w} AND y >= ${y} AND y < ${y + h}`;

  const vertexType = (name?: string): VertexAddress => addressing.vertexType(name);

  const ascending = (a: bigint, b: bigint): number => (a < b ? -1 : a > b ? 1 : 0);

  /** The vertices a set of dense ids names, read out of the tiles those ids address. */
  const readByDenseId = async (type: string, ids: readonly bigint[]): Promise<CorpusVertex[]> => {
    if (ids.length === 0) return [];
    const address = vertexType(type);
    const tiles = [...new Set(ids.map((id) => address.tileOf(id)))].sort(ascending);
    const rows = await query(
      `SELECT * FROM read_parquet(${list(tiles.map((k) => address.tileUrl(k)))}) ` +
        `WHERE dense_id IN (${ids.join(', ')})`,
    );
    return rows.map((row) => vertexOf(type, row));
  };

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
  ): Promise<CorpusVertex[]> => {
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
    const found: CorpusVertex[] = [];
    for (const type of candidates) {
      const rows = await query(
        `SELECT * FROM read_parquet(${list(tileUrls.get(type.type)!)}) ` +
          `WHERE ${ident(IDENTITY)} IN (${ids.map(lit).join(', ')})`,
      );
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
  ): Promise<CorpusEdge[]> => {
    const edges: CorpusEdge[] = [];
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

  return {
    url,
    types,
    addressing,

    async extent(type) {
      const address = vertexType(type);
      if (extents.has(address.type)) return extents.get(address.type)!;
      let answer: Extent | null = null;
      if (has(address.type, 'x') && has(address.type, 'y')) {
        const urls = tileUrls.get(address.type)!;
        const rows =
          urls.length === 0
            ? []
            : await query(
                `SELECT path_in_schema AS axis,
                        min(CAST(stats_min_value AS DOUBLE)) AS lo,
                        max(CAST(stats_max_value AS DOUBLE)) AS hi
                   FROM parquet_metadata(${list(urls)})
                  WHERE path_in_schema IN ('x', 'y') AND stats_min_value IS NOT NULL
                  GROUP BY 1`,
              );
        const axis = new Map(rows.map((row) => [text(row, 'axis'), row]));
        const x = axis.get('x');
        const y = axis.get('y');
        if (x && y) {
          answer = {
            minX: floatOf(x['lo'], 'x.min'),
            maxX: floatOf(x['hi'], 'x.max'),
            minY: floatOf(y['lo'], 'y.min'),
            maxY: floatOf(y['hi'], 'y.max'),
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

      const rows = await query(
        `SELECT * FROM read_parquet(${list(tileUrls.get(address.type)!)}) WHERE ${boxOf(box)}`,
      );
      const vertices = rows.map((row) => vertexOf(address.type, row));
      const tiles = [...new Set(vertices.map((v) => address.tileOf(v.denseId)))].sort(ascending);

      // The addressing decides which orientations apply and why one is missing; this only fetches.
      // Reproducing that rule here is how the two halves of a window drift apart.
      const plan = addressing.window({ type: address.type, tiles, directions });
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

    /**
     * One vertex by identity.
     *
     * **`id` is the subject IRI, and this is the decision the handover left without an owner.**
     *
     * The alternative was the `dense_id`, and it is the cheap one: a shift names the tile, one
     * request answers, and the whole lookup is `O(1)`. It is refused because *the spatial order is
     * the id space* — redoing the layout ranks every vertex by the Morton code of its new position
     * and lets that rank be its id, so a `dense_id` held anywhere outside the corpus names a
     * different vertex after the next write. A bookmark, a selection, a link from another system
     * and a row in somebody else's database all key on something, and an address is not something
     * to key on.
     *
     * The corpus states this and the tree contradicts itself about it, which is worth naming rather
     * than inheriting: `apps/corpus/guards/guards.mjs` says *"`dense_id` is an address and cannot
     * also be an identity"*, the conformance corpus's own manifest marks `subject` `is_primary`,
     * and `crates/fossil-df/src/lib.rs` marks `dense_id` `is_primary` on every vertex type it
     * writes. Two writers, two answers, one field. This picks the one the guard and the artefact
     * agree on.
     *
     * **The flag in the manifest is not consulted, because the two writers disagree about it.**
     * `property_groups` carries `is_primary`, and it would be the obvious place to read this from:
     * the conformance corpus marks `subject` primary, and `crates/fossil-df/src/lib.rs` —
     * `vertex_info`, the writer with no second writer left to mirror — marks `dense_id` primary and
     * `subject` not. Reading the flag would therefore pick the address on every corpus `fossil run`
     * produces. The column name is the stable thing, so the column name is what this keys on, and
     * the disagreement is a defect in the artefact rather than something to route around here.
     *
     * **What it costs, measured rather than asserted.** There is no index from a subject to an
     * address — no side table, no sorted copy, nothing in the manifest — so this is a scan of the
     * `subject` column over every tile of the type. Parquet's footer statistics do not help: the
     * rows are in Morton order and subjects are not, so the per-tile `min`/`max` overlap and the
     * engine skips nothing. On the conformance corpus that is 5 tiles of 300 rows; at five million
     * vertices the `subject` column is 8.016 compressed bytes per row, so a single lookup reads
     * about 40 MB. Column pruning is the only thing that keeps it off the other five columns.
     *
     * **What would change it.** A `subject → dense_id` index in the corpus, tiled and addressed
     * like everything else, would make this `O(1)` and would have to be rewritten by every layout
     * pass — which is affordable exactly because the pass already rewrites every tile. Nothing in
     * the format has one today, and adding one is a change to the artefact and not to this file.
     * The other thing that would change it is a stable layout, and the format says there is not
     * one.
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
      const edges: CorpusEdge[] = [];
      const emitted = new Set<string>();
      let frontier = resolved.map((v) => v.denseId);
      // One `not-declared`/`not-requested` reading for the whole walk: the orientations are the
      // same at every hop, so the addressing is asked once, with no tiles, rather than per hop.
      const orientations = addressing.window({ type, tiles: [], directions });

      for (let hop = 0; hop < depth && frontier.length > 0; hop += 1) {
        const tiles = [...new Set(frontier.map((id) => address.tileOf(id)))].sort(ascending);
        const inFrontier = frontier.join(', ');
        const found = await readEdges(
          type,
          addressing.window({ type, tiles, directions }).edges,
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
