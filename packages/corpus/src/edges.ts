/**
 * The relations incident to a set of vertices, read out of the adjacency the corpus stores twice —
 * `by_source` is CSR and `by_target` is CSC — one tile of it at a time.
 */

import type {
  CorpusAddressing,
  Direction,
  EdgeTiles,
  Gap,
  ProjectionAddress,
} from './address.js';
import { vertexOf, type Identity, type PlacedVertex } from './identity.js';
import type { QueryFn } from './query.js';
import { ascending, CorpusReadError, distinct, ident, idOf, list } from './sql.js';

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
  readonly addressing: CorpusAddressing;
  readonly identity: Identity;
}): Edges {
  const { query, addressing } = reads;
  const { findByIdentity } = reads.identity;

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
