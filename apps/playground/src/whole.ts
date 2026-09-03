/**
 * The baseline: load the whole corpus once, then pan and zoom on the GPU and ask nothing again.
 *
 * `src/tiles.ts` is the windowed path — every camera move opens the tiles the new rectangle
 * touches and nothing else. That is the interesting claim, and an interesting claim is worth
 * whatever it is compared against. This is what it is compared against: the thing every tiled
 * viewer is judged against, and the thing a reader assumes you are avoiding for a reason. *Just
 * load the array.*
 *
 * So it is here, beside it, over the same corpus and behind the same `BoundedSource`, and the two
 * ledgers are the argument. Nothing in this file is a straw man: it reads the four columns a point
 * needs and the two an edge needs, through the app's own `query`, and after that it ignores the
 * viewport completely — **the renderer does the culling**, which is the whole point of the
 * baseline. A camera move reaches no engine, issues no request and rebuilds no array.
 *
 * ## Why the same `Slice` object comes back every time
 *
 * `useQueryLoop` stores each answer with `setState`, and React bails out of a re-render when the
 * next state is `Object.is`-equal to the current one. The effect that uploads geometry —
 * `setPointPositions` / `setLinks` — is keyed on that state. Handing back the *same object* is
 * therefore what makes the second camera move cost nothing at all rather than re-uploading 8 MB of
 * positions and 16 MB of links to say the identical thing. A fresh `Slice` wrapping the same
 * arrays would typecheck and would re-upload on every pan, which is not the baseline anybody means.
 *
 * ## What it costs, and where the number comes from
 *
 * DuckDB issues its own reads from inside its Worker, where this thread cannot weigh them — the
 * same limitation `src/tiles.ts` records. So {@link WholeCost.bytes} is footer arithmetic:
 * `total_compressed_size` summed over **the columns actually projected**, across the vertex payload
 * and the `by_source` adjacency. It is derived and it says so.
 *
 * It is **not** the convention the streaming ledger beside it uses, and the two must not be added
 * up. `ViewCost.bytes` counts whole vertex *tiles* — every column, including `subject`, which is
 * the widest one and is never drawn — and counts no adjacency file at all. Compared like for like
 * on this corpus: 12,957 kB of vertex columns here against 21,127 kB of whole vertex tiles there
 * for the same opening frame, plus 14,035 kB of adjacency that the other ledger does not report.
 *
 * ## The ceiling, declared
 *
 * A baseline that quietly dies is worse than no baseline. {@link WholeSourceOptions.budgetBytes} is
 * the arithmetic this file will commit to typed arrays before it allocates anything, and crossing
 * it is a refusal *with the number attached* — reported on the ledger, not thrown into the
 * renderer's `onFailure`, because "this corpus is too big to hold" is a measurement and not a
 * crash. Anything that does throw on the way — an allocation the tab cannot serve, a query that
 * dies — is caught and lands in the same field.
 */
import type { Corpus } from '@fossil-lang/corpus';
import type { BoundedSource, Slice, SliceRequest, Viewport } from '@kanzo-tech/graph';
import { vertexId } from '@kanzo-tech/graph';

import type { QueryFn } from './duckdb.js';
import { extentOf, type Rect, type TileBox } from './stream.js';

/**
 * What the baseline cost — the four numbers a tiled viewer is judged on, plus what it holds.
 *
 * Deliberately the same vocabulary as {@link import('./tiles.js').SliceCost} where the two have a
 * question in common (`bytes`, `ms`) and deliberately not where they do not: there are no `tiles`
 * here because the baseline does not select any, and no `matched` because everything matched.
 */
export interface WholeCost {
  /** Vertices held in this tab, and edges. Both are the whole corpus or the load failed. */
  rows: number;
  links: number;
  /**
   * Compressed bytes the one load had to read, over the columns it projected.
   *
   * Footer arithmetic, not a weighed response — see this file's header for why, and for why this
   * is not the same convention as the streaming ledger's `in runs`.
   */
  bytes: number;
  /** Bytes of typed array this tab is holding — the units the budget is declared in. */
  held: number;
  /**
   * Queries the load issued.
   *
   * On the ledger because it is the number that must stop growing. A camera move that reaches
   * DuckDB would make this a slower streaming path rather than a baseline, and this is where that
   * would show.
   */
  queries: number;
  /** Wall clock for the load — which is the whole of time to first paint. */
  loadMs: number;
  /** Wall clock for *this* answer. Equal to `loadMs` on the first, memory-speed on every later one. */
  ms: number;
  /** Camera moves answered since the load. */
  moves: number;
  /** Why the baseline refused or died, with the number that says so. `null` while it holds. */
  failure: string | null;
}

export interface WholeSourceOptions {
  /** The corpus, open or opening — the same promise `corpusSource` takes, for the same reason. */
  corpus: Corpus | Promise<Corpus>;
  /** The footer, already bought. Read for `extent()` alone, exactly as the windowed source does. */
  boxes: readonly TileBox[];
  /** The one capability. The app's single DuckDB connection, and there is no second engine. */
  query: QueryFn;
  /** Which vertex type to draw. Defaults to the first the index names. */
  vertexType?: string;
  /** Told what the load cost, and then told again — with `moves` incremented — on every camera move. */
  onCost?(cost: WholeCost): void;
  /** Palette slots, asked of the theme rather than assumed. Same argument as `corpusSource`. */
  slots?: number;
  /**
   * The ceiling, in bytes of typed array, that this source will not cross.
   *
   * 256 MiB, which is not a browser limit — it is a number large enough that the bench corpus
   * (34 MB held) is nowhere near it and small enough that a corpus which *would* wedge the tab
   * gets refused with an arithmetic answer instead of an out-of-memory page. Move it when a
   * measurement disagrees with it, not to make something fit.
   */
  budgetBytes?: number;
  /**
   * How many `dense_id` a single query covers.
   *
   * The load is batched, and the batching is not about the engine — it is about `QueryFn`, whose
   * contract is `Record<string, unknown>[]`. A million vertices in one call is a million JS objects
   * alive at once, built to be read four fields from and dropped. Batching bounds that peak; it
   * does not change what is read, and the byte figure is identical either way.
   */
  batch?: number;
}

const EMPTY: Slice = {
  n: 0,
  marks: 0,
  vertices: new BigUint64Array(0),
  positions: new Float32Array(0),
  links: new Float32Array(0),
  categories: new Uint16Array(0),
};

/** Widths arrive as `bigint` from some hosts and `number` from others; `QueryFn` says so. */
const num = (value: unknown): number => (typeof value === 'bigint' ? Number(value) : Number(value));

/** A single-quoted SQL string literal. The URLs are the corpus's own; the escape is still owed. */
const lit = (value: string): string => `'${value.replace(/'/g, "''")}'`;

/** A list of them, for `read_parquet([…])` and `parquet_metadata([…])`. */
const list = (values: readonly string[]): string => `[${values.map(lit).join(', ')}]`;

/** Same rule as `tiles.ts`: a `fill` that is a colour is not a column name. */
function column(fill: string | undefined): string {
  return fill && !fill.startsWith('var(') && !fill.startsWith('#') ? fill : 'cluster_id';
}

/**
 * Every payload file of a set, under either container.
 *
 * `rowgroups` is one file whose row groups are the tiles, so tile 0's URL names all of them and the
 * rest are the same string; `files` is one per tile. `files()` on a vertex address already answers
 * this, and an adjacency has no such method — hence the loop, and hence the `Set`.
 */
function filesOf(tiles: bigint | null, urlOf: (tile: number) => string): string[] {
  const seen = new Set<string>();
  const total = tiles === null ? 1 : Math.max(1, Number(tiles));
  for (let k = 0; k < total; k += 1) seen.add(urlOf(k));
  return [...seen];
}

/** What the load found before it built anything. */
interface Plan {
  type: string;
  typeIndex: number;
  count: number;
  edgeCount: number;
  vertexFiles: string[];
  edgeFiles: string[];
}

/**
 * The whole corpus, held — a `BoundedSource` that answers every rectangle with all of it.
 *
 * `total()` reports the real count, so `shouldSlice` says `true` and the query loop keeps observing
 * the camera. That is left alone on purpose: under-reporting the total to make the loop stop asking
 * would buy the same behaviour by lying about the corpus, and the ledger's `moves` counter is a
 * more honest way to show that the asking costs nothing.
 */
export function wholeSource(options: WholeSourceOptions): BoundedSource {
  const { batch = 250_000, boxes, budgetBytes = 256 * 1024 * 1024, onCost, query, slots = 8, vertexType } = options;
  const extent: Rect | null = extentOf(boxes);

  /** The one load. `null` until the first `slice()`, and never re-entered. */
  let loading: Promise<void> | null = null;
  /** The answer, built once and handed back by reference — see the header. */
  let held: Slice | null = null;

  let opened: Promise<{ corpus: Corpus; count: number }> | null = null;
  let queries = 0;
  let moves = 0;
  let bytes = 0;
  let heldBytes = 0;
  let loadMs = 0;
  let failure: string | null = null;

  const ask = async (sql: string): Promise<Record<string, unknown>[]> => {
    queries += 1;
    return query(sql);
  };

  const open = () => {
    opened ??= (async () => {
      const corpus = await options.corpus;
      const address = corpus.addressing.vertexType(vertexType);
      const declared = corpus.types.vertices.find((v) => v.type === address.type);
      return { corpus, count: Number(declared?.count ?? address.count ?? 0n) };
    })();
    return opened;
  };

  const report = (ms: number): void =>
    onCost?.({
      rows: held ? held.marks : 0,
      links: held ? held.links.length / 2 : 0,
      bytes,
      held: heldBytes,
      queries,
      loadMs,
      ms,
      moves,
      failure,
    });

  /** What the corpus says it holds and where, before a byte of it is read. */
  async function plan(): Promise<Plan> {
    const { corpus } = await open();
    const address = corpus.addressing.vertexType(vertexType);
    const declared = corpus.types.vertices.find((v) => v.type === address.type);
    const count = Number(declared?.count ?? address.count ?? 0n);

    // Out-edges only. `by_source` is complete for DRAWING — every edge whose source is held has its
    // destination held too, because everything is held — and reading `by_target` as well would be
    // the same relation a second time. The addressing layer says exactly this about `['src']`.
    const edgeFiles: string[] = [];
    let edgeCount = 0;
    for (const edge of corpus.addressing.incident(address.type)) {
      const adjacency = edge.adjacency('src');
      if (adjacency === null) continue;
      edgeCount += Number(edge.count ?? 0n);
      for (const url of filesOf(adjacency.tiles, (k) => adjacency.tileUrl(k))) edgeFiles.push(url);
    }

    return {
      type: address.type,
      typeIndex: Math.max(0, corpus.addressing.types.indexOf(address)),
      count,
      edgeCount,
      vertexFiles: filesOf(address.tiles, (k) => address.tileUrl(k)),
      edgeFiles: [...new Set(edgeFiles)],
    };
  }

  /** Compressed bytes of named columns across named files — the footer's answer, not a response's. */
  async function weigh(files: readonly string[], columns: readonly string[]): Promise<number> {
    if (files.length === 0 || columns.length === 0) return 0;
    const wanted = columns.map(lit).join(', ');
    const rows = await ask(
      `SELECT sum(total_compressed_size) AS bytes FROM parquet_metadata(${list(files)}) ` +
        `WHERE path_in_schema IN (${wanted})`,
    );
    return num(rows[0]?.bytes ?? 0);
  }

  /**
   * Read everything, once.
   *
   * Indexed by `dense_id` rather than accumulated in arrival order, and that is what removes the
   * `ORDER BY`: `dense_id` is a vertex's rank along the Morton curve, so it is dense over
   * `[0, count)` and it IS the row index of the answer. A sort over a million rows to recover a
   * number the rows already carry would be the load's largest cost and buy nothing.
   */
  async function load(fill: string): Promise<void> {
    const started = performance.now();
    const shape = await plan();
    const { count, edgeCount } = shape;

    if (count === 0) {
      failure = 'the manifest declares no vertex count, so there is no whole to load';
      loadMs = performance.now() - started;
      return;
    }

    // Positions, identities, categories, links — the four allocations, priced before any of them
    // exists. 8 B of identity + 8 B of position + 2 B of category per vertex, 8 B per edge.
    const want = count * 18 + edgeCount * 8;
    if (want > budgetBytes) {
      failure =
        `refused: ${count.toLocaleString()} vertices and ${edgeCount.toLocaleString()} edges ` +
        `would hold ${(want / 1024 / 1024).toFixed(0)} MB of typed array, over the ` +
        `${(budgetBytes / 1024 / 1024).toFixed(0)} MB this baseline commits to`;
      heldBytes = want;
      loadMs = performance.now() - started;
      return;
    }

    bytes =
      (await weigh(shape.vertexFiles, ['dense_id', 'x', 'y', fill])) +
      (await weigh(shape.edgeFiles, ['src_dense', 'dst_dense']));

    const positions = new Float32Array(count * 2);
    const vertices = new BigUint64Array(count);
    const categories = new Uint16Array(count);
    const links = new Float32Array(edgeCount * 2);
    heldBytes =
      positions.byteLength + vertices.byteLength + categories.byteLength + links.byteLength;

    for (let dense = 0; dense < count; dense += 1) vertices[dense] = vertexId(shape.typeIndex, dense) as bigint;

    let seen = 0;
    let stray = 0;
    for (let lo = 0; lo < count; lo += batch) {
      const hi = Math.min(count, lo + batch);
      const rows = await ask(
        `SELECT dense_id, x, y, "${fill.replace(/"/g, '""')}" AS cat ` +
          `FROM read_parquet(${list(shape.vertexFiles)}) ` +
          `WHERE dense_id >= ${lo} AND dense_id < ${hi}`,
      );
      for (const row of rows) {
        const dense = num(row.dense_id);
        if (dense < 0 || dense >= count) {
          stray += 1;
          continue;
        }
        positions[dense * 2] = num(row.x);
        positions[dense * 2 + 1] = num(row.y);
        // The same fold `tiles.ts` applies, for the same measured reason: unfolded, everything past
        // the palette's last slot resolves to one muted token and the picture is a grey cloud.
        categories[dense] = ((num(row.cat) % slots) + slots) % slots;
        seen += 1;
      }
    }

    /**
     * Links, as row indices — and a `Float32Array` of them, which is the renderer's contract and
     * is exact here rather than approximately exact. A row index is a `dense_id`, and every integer
     * below 2²⁴ is representable in float32, so this is lossless up to 16,777,216 vertices. Past
     * that a corpus loses edge endpoints silently, which is one more reason the baseline has a
     * declared ceiling.
     */
    let cursor = 0;
    let dangling = 0;
    // Edges are addressed by their SOURCE's `dense_id`, so the batch is a vertex range and the rows
    // it returns are however many edges those vertices happen to have — proportional, not equal.
    const step = edgeCount > 0 ? Math.max(1, Math.floor((batch * count) / edgeCount)) : count;
    for (let lo = 0; lo < count && edgeCount > 0; lo += step) {
      const hi = Math.min(count, lo + step);
      const rows = await ask(
        `SELECT src_dense, dst_dense FROM read_parquet(${list(shape.edgeFiles)}) ` +
          `WHERE src_dense >= ${lo} AND src_dense < ${hi}`,
      );
      for (const row of rows) {
        const src = num(row.src_dense);
        const dst = num(row.dst_dense);
        if (src < 0 || src >= count || dst < 0 || dst >= count || cursor + 2 > links.length) {
          dangling += 1;
          continue;
        }
        links[cursor] = src;
        links[cursor + 1] = dst;
        cursor += 2;
      }
    }

    held = {
      n: seen,
      // Everything held is drawn. There are no anchors in a baseline: an anchor is a far end out of
      // a tile the window happened to open, and this opened all of them.
      marks: seen,
      vertices,
      positions,
      links: cursor === links.length ? links : links.subarray(0, cursor),
      categories,
    };
    loadMs = performance.now() - started;

    // Both are `0` on this corpus and neither is asserted away: a row the load could not place is a
    // row the picture is missing, and the ledger is where that belongs rather than a console.
    if (seen !== count || stray > 0) {
      failure = `held ${seen.toLocaleString()} of ${count.toLocaleString()} vertices${stray > 0 ? `, ${stray} out of range` : ''}`;
    } else if (dangling > 0) {
      failure = `dropped ${dangling.toLocaleString()} of ${edgeCount.toLocaleString()} edges with an endpoint it could not place`;
    }
  }

  return {
    /** The real count, off the manifest, with no query — the same answer the windowed source gives. */
    total: async () => (await open()).count,

    /** The corpus's rectangle, off the footer already bought. No query, so the camera frames first. */
    extent: async (): Promise<Viewport> =>
      extent
        ? { xMin: extent.xlo, yMin: extent.ylo, xMax: extent.xhi, yMax: extent.yhi }
        : { xMin: 0, yMin: 0, xMax: 0, yMax: 0 },

    /**
     * Everything, every time — **the viewport is read and then ignored**, which is the baseline.
     *
     * `limit` is ignored for the same reason: a baseline that sampled would be a worse windowed
     * source rather than a different thing. `pinned` needs no handling because nothing is ever
     * absent, and `perPixel` none because there is no per-move work for a pixel floor to save.
     */
    async slice(request: SliceRequest): Promise<Slice> {
      const started = performance.now();
      if (loading === null) {
        // The one thing a request decides here, and only the FIRST one decides it: which column is
        // held as the categorical. Recolouring would be a second load of the whole corpus, so a
        // later request naming another column is answered with the one in hand rather than paid for.
        const fill = column(request.fill);
        loading = load(fill).catch((cause: unknown) => {
          // An allocation the tab cannot serve, or a query that died. It is reported on the ledger
          // rather than rethrown: `onFailure` paints the canvas over with a message, and "this
          // corpus does not fit" is a result of the comparison rather than a broken component.
          failure = `the load did not finish: ${String(cause)}`;
          held = null;
        });
      } else {
        moves += 1;
      }
      await loading;
      report(performance.now() - started);
      return held ?? EMPTY;
    },
  };
}
