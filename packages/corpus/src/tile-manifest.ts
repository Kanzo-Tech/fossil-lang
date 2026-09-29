/**
 * What the reader knows about each tile before it reads one — Iceberg's manifest: the per-tile
 * statistics the writer publishes, read once per type while the corpus opens ({@link published}),
 * and, for the members step 5 retires, the Parquet footers they were read off before the writer
 * published them.
 */

import { CorpusManifestError, strideOf, type CorpusAddressing, type Direction } from './address.js';
import type { Metrics } from './expression.js';
import type { QueryFn } from './query.js';
import { lit, list } from './sql.js';

/** One tile's published entry: its number and Iceberg's four statistics. */
export interface TileEntry extends Metrics {
  readonly tile: number;
}

/** One adjacency orientation cut on a type's tiles, as the manifest lists it — sparse. */
export interface AdjacencyEntries {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  readonly alignedBy: Direction;
  readonly tiles: ReadonlyMap<number, TileEntry>;
}

/** One zoom's entries: its tiles, dense, and the edge sets its tiles address. */
export interface MatrixEntries {
  readonly z: number;
  readonly tiles: readonly TileEntry[];
  readonly adjacencies: readonly AdjacencyEntries[];
  /** The rung's quotient, sparse, or `null` where the manifest lists none. */
  readonly quotient: ReadonlyMap<number, TileEntry> | null;
}

/**
 * **A type's tile manifest, parsed** — `crates/fossil-sinks/src/tiles.rs, TileManifest` read back.
 *
 * A bound that is an integer beyond 2^53 is dropped rather than kept rounded: `JSON.parse` rounds
 * it, and a rounded bound can prune the tile holding the match, where an absent one only costs the
 * pruning.
 */
export function published(text: string, where: string): readonly MatrixEntries[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    throw new CorpusManifestError(`${where} is not JSON: ${(cause as Error).message}`);
  }
  const record = (value: unknown): Record<string, unknown> =>
    typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
  const numbers = (value: unknown, exact: boolean): Record<string, number> => {
    const out: Record<string, number> = {};
    for (const [key, v] of Object.entries(record(value))) {
      if (typeof v !== 'number' || (exact && Number.isInteger(v) && !Number.isSafeInteger(v))) continue;
      out[key] = v;
    }
    return out;
  };
  const entry = (value: unknown): TileEntry => {
    const e = record(value);
    return {
      tile: Number(e['tile']),
      recordCount: Number(e['record_count']),
      nullValueCounts: numbers(e['null_value_counts'], false),
      lowerBounds: numbers(e['lower_bounds'], true),
      upperBounds: numbers(e['upper_bounds'], true),
    };
  };
  const sparse = (list: unknown): Map<number, TileEntry> =>
    new Map((Array.isArray(list) ? list : []).map((t) => [Number(record(t)['tile']), entry(t)]));
  const matrices = record(parsed)['matrices'];
  if (!Array.isArray(matrices)) throw new CorpusManifestError(`${where} lists no matrices`);
  return matrices.map((m) => {
    const matrix = record(m);
    const quotient = matrix['quotient'];
    return {
      z: Number(matrix['z']),
      tiles: (Array.isArray(matrix['tiles']) ? matrix['tiles'] : []).map(entry),
      adjacencies: (Array.isArray(matrix['adjacencies']) ? matrix['adjacencies'] : []).map((a) => {
        const adjacency = record(a);
        return {
          edgeType: String(adjacency['edge_type']),
          srcType: String(adjacency['src_type']),
          dstType: String(adjacency['dst_type']),
          alignedBy: adjacency['aligned_by'] === 'dst' ? 'dst' : 'src',
          tiles: sparse(adjacency['tiles']),
        };
      }),
      quotient: Array.isArray(quotient) ? sparse(quotient) : null,
    };
  });
}

/** The bounding box of one vertex type's positions. */
export interface Extent {
  readonly minX: number;
  readonly maxX: number;
  readonly minY: number;
  readonly maxY: number;
}

/**
 * Where each tile is, read from the footers **once per corpus**.
 *
 * The arithmetic says which tiles exist; only the boxes say which ones a rectangle intersects,
 * and the boxes are Parquet statistics. Without them the only correct read is every tile with a
 * `WHERE`, and DuckDB prunes the rows but still opens each footer — `ceil(vertex_count /
 * chunk_size)` of them per window, which is 2,442 at ten million and made a window that answers
 * 3,152 vertices take 4.7 s. Measured from kanzo-ui, 2026-08-25: the same one-percent rectangle
 * cost 75.6 ms at a million and 1,000.8 at five, out of one tile either way.
 *
 * So it is read once and kept: a few thousand rows of metadata, no column data, and every window
 * after it opens only the tiles it names. `Corpus.extent` still scans every tile because its
 * answer is about every tile.
 */
export interface TileBox {
  readonly tile: bigint;
  readonly x0: number;
  readonly x1: number;
  readonly y0: number;
  readonly y1: number;
  /**
   * Where the tile's bytes begin in its file, and how far they run.
   *
   * Off the same footer read as the box, because it is the same row of `parquet_metadata` — and
   * needed for the same reason the box is: {@link Corpus.frame} reports what an answer cost, and a
   * cost report whose byte figure is a guess is not one. `min(coalesce(dictionary_page_offset,
   * data_page_offset))` is where a row group starts — the dictionary page comes first when there
   * is one, and `file_offset` is not reliably populated by every writer.
   */
  readonly start: number;
  readonly bytes: number;
}

/**
 * Selected tiles collapsed into maximal runs, each a single byte interval — what a rectangle
 * costs in `Range` requests rather than in tiles.
 *
 * Two tiles join a run when they are consecutive ordinals **and their bytes actually abut**. The
 * second condition is not paranoia about the format: Parquet does not promise that row groups are
 * written back to back, and a run whose members are not contiguous names an interval containing
 * bytes belonging to nobody. Against the corpora this repo writes every boundary abuts — measured,
 * 0 gaps in 244 — so the O(√n)-runs claim holds as O(√n) requests, and it is checked rather than
 * assumed. A tile with no footer entry contributes a run of its own and no bytes.
 */
export interface TileRun {
  first: number;
  last: number;
  bytes: number;
}

export const runsOf = (tiles: readonly number[], weights: Map<number, TileBox>): TileRun[] => {
  const runs: TileRun[] = [];
  let end: number | null = null;
  for (const tile of tiles) {
    const box = weights.get(tile);
    const run = runs[runs.length - 1];
    if (run !== undefined && box !== undefined && tile === run.last + 1 && end === box.start) {
      run.last = tile;
      run.bytes += box.bytes;
    } else {
      runs.push({ first: tile, last: tile, bytes: box?.bytes ?? 0 });
    }
    end = box === undefined ? null : box.start + box.bytes;
  }
  return runs;
};

/** The intervals a read is bounded by, over the column they are stated in. */
export interface ByteBound {
  readonly column: string;
  readonly runs: readonly TileRun[];
  /** How many ids a tile of the artefact being weighed spans — the read's own `span`. */
  readonly span: bigint;
}

/** The footers of one corpus, read once and kept. */
export interface TileManifest {
  tileBoxes(type: string, level?: number): Promise<readonly TileBox[]>;
  readonly footers: ReadonlyMap<string, readonly TileBox[]>;
  bytesOf(urls: readonly string[], columns: readonly string[], within?: ByteBound): Promise<number>;
  extent(type?: string): Promise<Extent | null>;
}

export async function tileManifestOf(reads: {
  readonly query: QueryFn;
  readonly addressing: CorpusAddressing;
  readonly payloadFiles: ReadonlyMap<string, readonly string[]>;
  readonly has: (type: string, column: string) => boolean;
}): Promise<TileManifest> {
  const { query, addressing, payloadFiles, has } = reads;

  const extents = new Map<string, Extent | null>();

  const boxes = new Map<string, Promise<readonly TileBox[]>>();

  /**
   * @param level When given, the footers of that WRITTEN level's tiles rather than the payload's.
   *   One `parquet_metadata` read over a handful of files, cached beside the payload's under its
   *   own key — and it is what keeps `FrameCost.bytes` a measurement under a level read instead of
   *   the payload's number wearing a level's label.
   */
  const tileBoxes = (type: string, level?: number): Promise<readonly TileBox[]> => {
    // **Level 0 and the payload are one key**, because they are one artefact: the payload IS the
    // projection at `scale: 1` — `address.ts` states it normatively — so asking for its footers by
    // level has to hit the read `open` already made and not issue a second one under a name
    // for the same bytes. Without this the seam's «the payload is the cache for stride 1» would be
    // true of the answer and false of the request count.
    const payload = level === undefined || level === 0;
    const key = payload ? type : `${type}\u0000l${level}`;
    const cached = boxes.get(key);
    if (cached) return cached;
    const address = addressing.vertexType(type);
    const urls = payload ? payloadFiles.get(type)! : [...address.projectionFiles(strideOf(level!))];
    // Which tile a footer row is about. Under `files` it is the file — one per tile, and the row
    // groups inside it are one tile's worth however many there are. Under `rowgroups` it is the
    // ordinal, and this is the one place in this file where that ordinal is the address: a vertex
    // tile is exactly `chunk_size` gapless rows, so row group `k` IS tile `k`.
    const index = new Map(urls.map((url, k) => [url, BigInt(k)]));
    const perGroup = address.container === 'rowgroups';
    const loading = (async (): Promise<readonly TileBox[]> => {
      // `min_value`/`max_value`, never `min`/`max`: Parquet's original statistics fields compare
      // bytes as signed, which is meaningless for an unsigned column, so a writer that gets it
      // right leaves them empty and a reader that only knows the deprecated pair concludes the
      // footer carries no box at all. `coalesce` reads either.
      // No `WHERE path_in_schema IN ('x','y')`, and that is deliberate rather than sloppy: the box
      // still comes only from those two columns, through the CASE arms, and the two sums below want
      // EVERY column of the row group. One footer read answers both, which is what keeps this "read
      // once and kept" rather than "read twice and kept".
      const rows = await query(
        `SELECT file_name AS file, row_group_id AS rg, ` +
          `min(coalesce(dictionary_page_offset, data_page_offset)) AS start, ` +
          `sum(total_compressed_size) AS bytes, ` +
          `min(CASE WHEN path_in_schema = 'x' THEN coalesce(stats_min_value, stats_min)::DOUBLE END) AS x0, ` +
          `max(CASE WHEN path_in_schema = 'x' THEN coalesce(stats_max_value, stats_max)::DOUBLE END) AS x1, ` +
          `min(CASE WHEN path_in_schema = 'y' THEN coalesce(stats_min_value, stats_min)::DOUBLE END) AS y0, ` +
          `max(CASE WHEN path_in_schema = 'y' THEN coalesce(stats_max_value, stats_max)::DOUBLE END) AS y1 ` +
          `FROM parquet_metadata(${list(urls)}) GROUP BY 1, 2`,
      );
      const merged = new Map<bigint, TileBox>();
      for (const row of rows) {
        const tile = perGroup ? BigInt(String(row['rg'])) : index.get(String(row['file']));
        const [x0, x1, y0, y1] = ['x0', 'x1', 'y0', 'y1'].map((k) => Number(row[k]));
        const start = Number(row['start'] ?? 0);
        const bytes = Number(row['bytes'] ?? 0);
        // A tile whose name did not come back verbatim, or whose footer carries no statistics for
        // x or y, has no box — and a tile with no box is one this cannot exclude. Keeping it is
        // the conservative answer: the read stays correct and only loses the pruning.
        if (tile === undefined || ![x0, x1, y0, y1].every(Number.isFinite)) continue;
        const held = merged.get(tile);
        merged.set(
          tile,
          held === undefined
            ? { tile, x0: x0!, x1: x1!, y0: y0!, y1: y1!, start, bytes }
            : {
                tile,
                x0: Math.min(held.x0, x0!),
                x1: Math.max(held.x1, x1!),
                y0: Math.min(held.y0, y0!),
                y1: Math.max(held.y1, y1!),
                start: Math.min(held.start, start),
                bytes: held.bytes + bytes,
              },
        );
      }
      return [...merged.values()];
    })();
    boxes.set(key, loading);
    return loading;
  };

  /**
   * The payload boxes of every type that has geometry, resolved **while the corpus is opening.**
   *
   * It is not lazy, and the reason is {@link Corpus.frame}: it derives the level from the canvas
   * before it opens anything, and a level that arrived as a promise would be a request between
   * the camera moving and a URL being computable, which is the `viewport` verb this format deleted.
   * So the footers are bought with the manifests.
   *
   * **This is not an extra read.** `tileBoxes` is read once per corpus and cached whatever calls it
   * first; all this does is decide that the first caller is `open`. What it replaces is a
   * `read_text` of one `codes.json` per type — a second index over the question these same bytes
   * answer, and the request that paid for it.
   */
  const footers = new Map<string, readonly TileBox[]>();
  await Promise.all(
    addressing.types
      .filter((type) => has(type.type, 'x') && has(type.type, 'y'))
      .map(async (type) => {
        footers.set(type.type, await tileBoxes(type.type));
      }),
  );

  /**
   * **What a set of files weighs**, from the same footers the boxes come from, read once per set.
   *
   * `FrameCost.bytes` counted the vertex tiles and nothing else, while `frame` was also opening the
   * adjacency for every answer that asked for links: at a million vertices that is 13 MB of
   * `by_source` against a 21 MB report, so the ledger was short by 38% of what it read. A cost
   * report that omits a file it opened is worse than no cost report, because it is believed.
   *
   * **The COLUMNS the query projects, not the whole tile.** Parquet is columnar and the engine
   * reads the chunks it needs: a vertex tile of this corpus is 21 MB and the four columns a view
   * draws with are 12.7 MB of it, the rest being `subject` — the widest column and one no frame
   * ever paints. A ledger that charged a frame for it would be as wrong as one that omitted the
   * adjacency, in the other direction, and the two errors nearly cancelling is worse than either.
   *
   * **The ROW GROUPS the read selected, not the whole file.** Under `rowgroups` every tile of a
   * set names one URL, so a sum over `parquet_metadata(urls)` weighs the whole FILE: a frame
   * reading three tiles of sixty-two was charged for all sixty-two, and the byte column did not
   * fall as the camera closed in — measured on the bench corpus, 21.76 MB reported against a
   * 21.40 MB payload the frame did not read. So the weighing is bounded the same way the READ is,
   * by the `dense_id` intervals {@link TileRun} already names, matched against each row group's
   * own statistics on the column those intervals are stated over. A row group whose footer
   * declares none is counted — the conservative answer, and the one {@link tileBoxes} makes for a
   * tile with no box.
   *
   * Under `files` the URL list already names the selected tiles and every interval intersects, so
   * the bound is a no-op there rather than a second convention.
   *
   * Keyed on the URL list, the column list AND the bound, because a set read two ways is two reads.
   */
  const weights = new Map<string, Promise<number>>();

  const bytesOf = (
    urls: readonly string[],
    columns: readonly string[],
    within?: ByteBound,
  ): Promise<number> => {
    if (urls.length === 0) return Promise.resolve(0);
    if (within !== undefined && within.runs.length === 0) return Promise.resolve(0);
    const bound =
      within === undefined
        ? ''
        : `${within.column}:${String(within.span)}:` +
          within.runs.map((r) => `${r.first}-${r.last}`).join(',');
    const key = `${columns.join(',')}\u0000${bound}\u0000${urls.join('\u0000')}`;
    const cached = weights.get(key);
    if (cached) return cached;
    const wanted = `path_in_schema IN (${columns.map((c) => lit(c)).join(', ')})`;
    const stat = (fn: string, field: string): string =>
      `${fn}(CASE WHEN path_in_schema = ${lit(within!.column)} THEN ` +
      `TRY_CAST(coalesce(stats_${field}_value, stats_${field}) AS DOUBLE) END)`;
    const sql =
      within === undefined
        ? `SELECT sum(total_compressed_size) AS bytes FROM parquet_metadata(${list([...urls])}) ` +
          `WHERE ${wanted}`
        : // One footer read, grouped per row group: the projected columns' bytes beside that
          // group's own bounds on the range column. `TRY_CAST` because the statistics arrive as
          // the writer wrote them and a group that declares none must be kept rather than throw.
          `SELECT sum(bytes) AS bytes FROM (\n` +
          `  SELECT sum(CASE WHEN ${wanted} THEN total_compressed_size ELSE 0 END) AS bytes,\n` +
          `         ${stat('min', 'min')} AS lo, ${stat('max', 'max')} AS hi\n` +
          `  FROM parquet_metadata(${list([...urls])}) GROUP BY file_name, row_group_id\n` +
          `) WHERE lo IS NULL OR hi IS NULL OR (` +
          within.runs
            .map(
              (run) =>
                `lo <= ${BigInt(run.last + 1) * within.span - 1n} ` +
                `AND hi >= ${BigInt(run.first) * within.span}`,
            )
            .join(' OR ') +
          `)`;
    const loading = query(sql).then((rows) => Number(rows[0]?.['bytes'] ?? 0));
    weights.set(key, loading);
    return loading;
  };

  return {
    tileBoxes,
    footers,
    bytesOf,
    async extent(type) {
      const address = addressing.vertexType(type);
      if (extents.has(address.type)) return extents.get(address.type)!;
      // The same footers a window needs, read once for both. This used to sweep
      // `parquet_metadata` over every tile on its own, which made opening a corpus and framing it
      // two O(N) passes over the same bytes — 2,450 range requests each at five million, measured
      // over a plain HTTP origin. The boxes are per tile; the extent is their union.
      let answer: Extent | null = null;
      if (has(address.type, 'x') && has(address.type, 'y')) {
        const boxed = await tileBoxes(address.type);
        if (boxed.length > 0) {
          answer = {
            minX: Math.min(...boxed.map((b) => b.x0)),
            maxX: Math.max(...boxed.map((b) => b.x1)),
            minY: Math.min(...boxed.map((b) => b.y0)),
            maxY: Math.max(...boxed.map((b) => b.y1)),
          };
        }
      }
      extents.set(address.type, answer);
      return answer;
    },
  };
}
