/**
 * The relations incident to a tile, read out of the adjacency the corpus stores twice — `by_source`
 * is CSR and `by_target` is CSC — at `z = Z`, and out of the rung's quotient below it.
 */

import type { CorpusAddressing, Direction, Gap } from './address.js';
import { runsOf, splitByTile, type Batch, type Reads } from './query.js';
import { CorpusReadError, ident, list, lit } from './sql.js';
import type { TileAddress, Zooms } from './tile-matrix.js';

/**
 * What {@link Corpus.edges} takes: the tiles, the half of the relation aligned on them, and a
 * signal. One answer comes back per tile, and a run of consecutive tiles is read in one statement.
 */
export interface EdgesParams {
  readonly from: readonly TileAddress[];
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

/** What {@link Corpus.edges} answers for one tile: the relations read, and the ones declined with why. */
export interface EdgeAnswer {
  readonly batches: readonly EdgeBatch[];
  readonly declined: readonly Gap[];
}

export interface Edges {
  edges(params: EdgesParams): Promise<readonly EdgeAnswer[]>;
}

export function edgesOf(reads: {
  readonly reads: Reads;
  readonly addressing: CorpusAddressing;
  readonly zooms: (type: string) => Zooms;
}): Edges {
  const { addressing } = reads;

  const wide = (values: ArrayLike<unknown> | undefined): BigUint64Array =>
    BigUint64Array.from(Array.from(values ?? [], (v) => BigInt(v as number | bigint)));

  /** One tile's share of one edge set: where it is read from, and where its answer goes. */
  interface Want {
    /** The edge set, so a run is never spread over two of them. */
    readonly set: string;
    readonly url: string;
    readonly key: string;
    readonly span: bigint;
    readonly columns: readonly string[];
    readonly tile: number;
    /** The tile manifest lists no edges for this tile: it is answered without a request. */
    readonly empty: boolean;
    readonly answer: number;
    readonly batch: number;
    readonly meta: Pick<EdgeBatch, 'edgeType' | 'srcType' | 'dstType'>;
  }

  /**
   * **The relations incident to each tile** — CSR or CSC at `Z`, the quotient below it — one answer
   * per address, in the order given.
   *
   * A relation cut on the tile's type in this orientation is read from the tile of its adjacency the
   * address names, unless the tile manifest says that tile has no edges, in which case nothing is
   * asked. A relation whose far end is another type is declined as `other-space` unless `relation`
   * names it; an orientation the corpus does not publish is `not-declared`. Below `Z` the quotient
   * is tiled on its lower cell and stored once, so `src` reads the pairs whose lower cell is in the
   * tile and `dst` is always declined: the pairs whose UPPER cell is there are in the tiles below
   * it, and no one range selects them.
   *
   * **A run of consecutive tiles of one edge set is one statement**, one conjunctive range on the
   * aligned column split back per tile — `scan.read`'s shape, for its reason.
   */
  const edges = async (params: EdgesParams): Promise<readonly EdgeAnswer[]> => {
    const { from, direction, relation, signal } = params;
    const answers = from.map(() => ({ batches: [] as EdgeBatch[], declined: [] as Gap[] }));
    const wants: Want[] = [];
    from.forEach((address, answer) => {
      const zoom = reads.zooms(address.type).zoom(address);
      const { batches, declined } = answers[answer]!;
      const want = (w: Omit<Want, 'answer' | 'batch'>) => {
        wants.push({ ...w, answer, batch: batches.length });
        batches.push(undefined as unknown as EdgeBatch);
      };
      if (zoom.rung === null) {
        const incident = addressing
          .incident(address.type)
          .filter((e) => relation === undefined || e.edgeType === relation);
        if (relation !== undefined && incident.length === 0) {
          throw new CorpusReadError(`${relation} is not a relation incident to ${address.type}`);
        }
        for (const edge of incident) {
          if ((direction === 'src' ? edge.srcType : edge.dstType) !== address.type) continue;
          if ((direction === 'src' ? edge.dstType : edge.srcType) !== address.type && relation === undefined) {
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
          want({
            set: `${edge.edgeType} ${edge.srcType} ${edge.dstType} ${direction}`,
            url: adjacency.tileUrl(address.tile),
            key: adjacency.column,
            span: BigInt(adjacency.chunkSize),
            columns: ['src_dense', 'dst_dense'],
            tile: address.tile,
            empty: published !== undefined && !published.tiles.has(address.tile),
            meta: { edgeType: edge.edgeType, srcType: edge.srcType, dstType: edge.dstType },
          });
        }
        return;
      }
      const tree = addressing.vertexType(address.type).cells!;
      const label = tree.relations.join('+');
      const whole = relation === undefined || (tree.relations.length === 1 && tree.relations[0] === relation);
      if (!whole || direction === 'dst' || zoom.quotient === null) {
        declined.push({ edgeType: relation ?? label, direction, reason: 'not-declared' });
        return;
      }
      want({
        set: `${address.type} ${address.z} quotient`,
        url: zoom.quotientUrl(address.tile) ?? '',
        key: 'src_cell',
        span: BigInt(zoom.matrix.tileRows),
        columns: ['src_cell', 'dst_cell', 'weight'],
        tile: address.tile,
        empty: !zoom.quotient.has(address.tile),
        meta: { edgeType: label, srcType: address.type, dstType: address.type },
      });
    });

    const fill = (w: Want, batch: Batch | null) => {
      const column = (name: string | undefined) =>
        name === undefined ? null : wide(batch?.getChild(name)?.toArray());
      answers[w.answer]!.batches[w.batch] = {
        ...w.meta,
        src: column(w.columns[0])!,
        dst: column(w.columns[1])!,
        weight: column(w.columns[2]),
      };
    };
    const sets = new Map<string, Want[]>();
    for (const w of wants) sets.set(w.set, [...(sets.get(w.set) ?? []), w]);
    const statements: Promise<void>[] = [];
    for (const set of sets.values()) {
      set.sort((a, b) => a.tile - b.tile);
      for (const run of runsOf(set)) {
        if (run.every((w) => w.empty)) {
          for (const w of run) fill(w, null);
          continue;
        }
        statements.push(
          (async () => {
            const { key, span, columns } = run[0]!;
            const lo = BigInt(run[0]!.tile) * span;
            const hi = (BigInt(run[run.length - 1]!.tile) + 1n) * span;
            const urls = [...new Set(run.filter((w) => !w.empty).map((w) => w.url))];
            const batch = await reads.reads.batch(
              `SELECT ${columns.map(ident).join(', ')} FROM read_parquet(${urls.length === 1 ? lit(urls[0]!) : list(urls)}) ` +
                `WHERE ${ident(key)} >= ${lo} AND ${ident(key)} < ${hi}`,
              columns,
              signal,
            );
            const tiles = splitByTile(batch, key, Number(span), columns);
            for (const w of run) fill(w, tiles.get(w.tile) ?? null);
          })(),
        );
      }
    }
    await Promise.all(statements);
    return answers;
  };

  return { edges };
}
