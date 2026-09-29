/**
 * The relations incident to a tile, read out of the adjacency the corpus stores twice — `by_source`
 * is CSR and `by_target` is CSC — at `z = Z`, and out of the rung's quotient below it; one tile of
 * it at a time. And, until step 5 retires them, the incidence `rows` and `neighbours` read.
 */

import type {
  CorpusAddressing,
  Direction,
  EdgeTiles,
  Gap,
  ProjectionAddress,
} from './address.js';
import { vertexOf, type Identity, type PlacedVertex } from './identity.js';
import type { QueryFn, Reads } from './query.js';
import { ascending, CorpusReadError, distinct, ident, idOf, list, lit } from './sql.js';
import type { TileAddress, Zooms } from './tile-matrix.js';

/** What {@link Corpus.edges} takes: one tile, the half of the relation aligned on it, and a signal. */
export interface EdgesParams {
  readonly from: TileAddress;
  /**
   * Which endpoint the tile holds: `src` reads the source-aligned half — the edges leaving the
   * tile — and `dst` the target-aligned one, the edges arriving in it.
   */
  readonly direction: Direction;
  /** One relation by label, which also admits one whose far end is another type. */
  readonly relation?: string;
  readonly signal?: AbortSignal;
}

/**
 * One relation's edges out of one tile, as columns. At `z = Z` the two ends are `dense_id`s and
 * `weight` is `null`. Below it they are the rung's cell ids and `weight` is how many edges a
 * quotient edge stands for — and the quotient is **undirected**, one row per cell pair keyed on the
 * lower cell (`crates/fossil-layout/src/layout/cells.rs, pair_edges`), so `src` is the lower of the
 * two and not the source of anything.
 */
export interface EdgeBatch {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  readonly src: BigUint64Array;
  readonly dst: BigUint64Array;
  readonly weight: BigUint64Array | null;
}

/** What {@link Corpus.edges} answers: the relations read, and the ones declined with why. */
export interface EdgeAnswer {
  readonly batches: readonly EdgeBatch[];
  readonly declined: readonly Gap[];
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
  /** Two addresses, not two IRIs: an edge a canvas draws between placed vertices. */
  readonly edgeType: string;
  readonly src: bigint;
  readonly dst: bigint;
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

/** What {@link Corpus.neighbours} takes beyond the identities. */
export interface NeighboursParams {
  /** Hops. Defaults to 1. */
  depth?: number;
  /** Which orientations to walk. Defaults to both — an undirected neighbourhood. */
  directions?: readonly Direction[];
}

export interface Edges {
  edges(params: EdgesParams): Promise<EdgeAnswer>;
  readEdges(
    vertexTypeName: string,
    groups: readonly EdgeTiles[],
    restrict: (adjacency: ProjectionAddress) => string,
    emitted?: Set<string>,
  ): Promise<PlacedEdge[]>;
  neighbours(ids: Iterable<string>, params?: NeighboursParams): Promise<Neighbourhood>;
}

export function edgesOf(reads: {
  readonly query: QueryFn;
  readonly reads: Reads;
  readonly addressing: CorpusAddressing;
  readonly identity: Identity;
  readonly zooms: (type: string) => Zooms;
}): Edges {
  const { query, addressing } = reads;
  const { findByIdentity } = reads.identity;

  const wide = (values: ArrayLike<unknown> | undefined): BigUint64Array =>
    BigUint64Array.from(Array.from(values ?? [], (v) => BigInt(v as number | bigint)));

  /**
   * One tile's edges out of one edge set, by one conjunctive range on the aligned column — the
   * shape `scan.read` selects a tile with, for the reason it gives.
   */
  const edgeBatch = async (
    sql: { readonly url: string; readonly key: string; readonly span: bigint; readonly tile: number },
    columns: readonly [string, string] | readonly [string, string, string],
    signal: AbortSignal | undefined,
  ) => {
    const lo = BigInt(sql.tile) * sql.span;
    const batch = await reads.reads.batch(
      `SELECT ${columns.map(ident).join(', ')} FROM read_parquet(${lit(sql.url)}) ` +
        `WHERE ${ident(sql.key)} >= ${lo} AND ${ident(sql.key)} < ${lo + sql.span}`,
      columns,
      signal,
    );
    return {
      src: wide(batch.getChild(columns[0])?.toArray()),
      dst: wide(batch.getChild(columns[1])?.toArray()),
      weight: columns[2] === undefined ? null : wide(batch.getChild(columns[2])?.toArray()),
    };
  };
  const none = () => ({ src: new BigUint64Array(), dst: new BigUint64Array(), weight: null });

  /**
   * **The relations incident to one tile** — CSR or CSC at `Z`, the quotient below it.
   *
   * A relation cut on the tile's type in this orientation is read from the one tile of its
   * adjacency the address names, unless the tile manifest says that tile has no edges, in which
   * case nothing is asked. A relation whose far end is another type is declined as `other-space`
   * unless `relation` names it; an orientation the corpus does not publish is `not-declared`.
   * Below `Z` the quotient is tiled on its lower cell and stored once, so `src` reads the pairs
   * whose lower cell is in the tile and `dst` is always declined: the pairs whose UPPER cell is
   * there are in the tiles below it, and no one range selects them.
   */
  const edges = async (params: EdgesParams): Promise<EdgeAnswer> => {
    const { from, direction, relation, signal } = params;
    const zoom = reads.zooms(from.type).zoom(from);
    const batches: EdgeBatch[] = [];
    const declined: Gap[] = [];
    if (zoom.rung === null) {
      const incident = addressing
        .incident(from.type)
        .filter((e) => relation === undefined || e.edgeType === relation);
      if (relation !== undefined && incident.length === 0) {
        throw new CorpusReadError(`${relation} is not a relation incident to ${from.type}`);
      }
      for (const edge of incident) {
        if ((direction === 'src' ? edge.srcType : edge.dstType) !== from.type) continue;
        if ((direction === 'src' ? edge.dstType : edge.srcType) !== from.type && relation === undefined) {
          declined.push({ edgeType: edge.edgeType, direction, reason: 'other-space' });
          continue;
        }
        const adjacency = edge.adjacency(direction);
        if (adjacency === null) {
          declined.push({ edgeType: edge.edgeType, direction, reason: 'not-declared' });
          continue;
        }
        const published = zoom.adjacencies.find(
          (a) =>
            a.edgeType === edge.edgeType &&
            a.srcType === edge.srcType &&
            a.dstType === edge.dstType &&
            a.alignedBy === direction,
        );
        const read =
          published !== undefined && !published.tiles.has(from.tile)
            ? none()
            : await edgeBatch(
                {
                  url: adjacency.tileUrl(from.tile),
                  key: adjacency.column,
                  span: BigInt(adjacency.chunkSize),
                  tile: from.tile,
                },
                ['src_dense', 'dst_dense'],
                signal,
              );
        batches.push({ edgeType: edge.edgeType, srcType: edge.srcType, dstType: edge.dstType, ...read });
      }
      return { batches, declined };
    }
    const tree = addressing.vertexType(from.type).cells!;
    const label = tree.relations.join('+');
    const whole = relation === undefined || (tree.relations.length === 1 && tree.relations[0] === relation);
    if (!whole || direction === 'dst' || zoom.quotient === null) {
      declined.push({ edgeType: relation ?? label, direction, reason: 'not-declared' });
      return { batches, declined };
    }
    const read = !zoom.quotient.has(from.tile)
      ? { ...none(), weight: new BigUint64Array() }
      : await edgeBatch(
          {
            url: zoom.quotientUrl(from.tile)!,
            key: 'src_cell',
            span: BigInt(zoom.matrix.tileRows),
            tile: from.tile,
          },
          ['src_cell', 'dst_cell', 'weight'],
          signal,
        );
    batches.push({ edgeType: label, srcType: from.type, dstType: from.type, ...read });
    return { batches, declined };
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

  /** The vertices a set of dense ids names, read out of the tiles those ids address. */
  const readByDenseId = async (type: string, ids: readonly bigint[]): Promise<PlacedVertex[]> => {
    if (ids.length === 0) return [];
    const address = addressing.vertexType(type);
    const tiles = [...new Set(address.tilesOf(ids))].sort(ascending);
    const rows = await query(
      `SELECT * FROM read_parquet(${list(distinct(tiles.map((k) => address.tileUrl(k))))}) ` +
        `WHERE dense_id IN (${ids.join(', ')})`,
    );
    return rows.map((row) => vertexOf(type, row));
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
    restrict: (adjacency: ProjectionAddress) => string,
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

  return {
    edges,
    readEdges,

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
      const address = addressing.vertexType(type);
      const seen = new Map(resolved.map((v) => [v.denseId, v]));
      const edges: PlacedEdge[] = [];
      const emitted = new Set<string>();
      let frontier = resolved.map((v) => v.denseId);
      // One `not-declared`/`not-requested` reading for the whole walk: the orientations are the
      // same at every hop, so the addressing is asked once, with no tiles, rather than per hop.
      const orientations = addressing.tilesFor({ type, tiles: [], directions });

      for (let hop = 0; hop < depth && frontier.length > 0; hop += 1) {
        const tiles = [...new Set(address.tilesOf(frontier))].sort(ascending);
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
