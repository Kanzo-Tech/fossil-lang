/**
 * Iceberg's `Table.scan` over a tile matrix set: a scan is a type, a filter and a projection,
 * bound once; `plan` is `plan_files` over the published statistics at every zoom; `read` answers a
 * batch per tile, and reads each run of consecutive tiles in one statement.
 */

import {
  bind,
  columnsOf,
  filterOf,
  residual,
  sqlOf,
  type Bound,
  type Box,
  type Filter,
} from './expression.js';
import { batchOf, type Batch, type Reads } from './query.js';
import { CorpusReadError, ident, list, lit } from './sql.js';
import type { TileAddress, Zoom, Zooms } from './tile-matrix.js';
import { CELL_NAMES } from './vocabulary.generated.js';

/** What {@link Corpus.scan} takes: Iceberg's `row_filter` and `selected_fields`, and no zoom. */
export interface ScanParams {
  readonly type: string;
  /** Rows to keep, as data. Bound when the scan is built. */
  readonly filter?: Filter;
  /**
   * The payload columns a row at `z = Z` carries — all of them when absent. A cell row below `Z`
   * carries the writer's cell columns whatever this says: they are not a projection of the
   * payload's, so there is nothing of them to select.
   */
  readonly select?: readonly string[];
}

/** One planned read — iceberg-rust's `FileScanTask`, with a tile where it has a file range. */
export interface ScanTask extends TileAddress {
  /** The tile's rows before the residual — the most a read of it returns. */
  readonly rows: number;
  readonly bbox: Box | null;
  /** What the tile's statistics could not settle, or `null` where they prove every row matches. */
  readonly residual: Filter | null;
}

/** A scan, bound. See {@link Corpus.scan}. */
export interface Scan {
  readonly params: ScanParams;
  /**
   * **Every task the scan needs, at every zoom**, coarsest first — Iceberg's `plan_files`, pruned
   * by the inclusive metrics evaluator over the tile manifest and each task carrying its residual.
   * Synchronous: the manifest was read while the corpus opened, and no byte is read here.
   *
   * A zoom whose rows do not carry every column the filter names has no task: a filter on a
   * payload column has none below `Z`, because a cell summarises every member and not the members
   * that pass.
   */
  plan(): readonly ScanTask[];
  /**
   * **One columnar batch per address, in the order given.** Any address in the matrix, planned or
   * not — a view builds `{ type, z, tile }` from what it sees. The filter's residual on each tile is
   * applied; a tile its statistics exclude answers with no rows and no request.
   *
   * **A run of consecutive tiles is one statement.** DuckDB's Parquet reader has no read by row
   * group, so a tile is selected by ONE conjunctive range on the column its rows are a range of —
   * `dense_id` at `Z`, `cell_id` below — which the engine prunes against every row group's
   * statistics in the footer. Tiles `t … t + n − 1` of one zoom are one range, so they are read in
   * one statement and split back into a batch per tile on that column: a statement pays a footer
   * and a round trip whatever it reads, and a view's window falls in a few runs of the curve.
   * Never a disjunction of ranges, which the engine evaluates per row.
   *
   * @throws {CorpusReadError} for an address outside the matrix, or at a zoom the filter cannot
   *   apply to — before any statement runs. An abort rejects with the signal's reason.
   */
  read(
    addresses: readonly TileAddress[],
    options?: { readonly signal?: AbortSignal },
  ): Promise<readonly Batch[]>;
}

export function scanOf(reads: {
  readonly reads: Reads;
  readonly zooms: (type: string) => Zooms;
}): (params: ScanParams) => Scan {
  return (params) => {
    const zooms = reads.zooms(params.type);
    const payload = zooms.zooms[zooms.zooms.length - 1]!;
    const select = params.select ?? [...payload.columns.keys()];
    for (const column of select) {
      if (!payload.columns.has(column)) {
        throw new CorpusReadError(
          `select names ${column}, which ${params.type} does not carry — its columns are ` +
            `${[...payload.columns.keys()].join(', ')}`,
        );
      }
    }
    const { filter } = params;
    // Bound against every column any zoom carries first, so an unknown column fails HERE and not
    // at a zoom a caller happens to read — Iceberg binds before it plans.
    if (filter !== undefined) {
      bind(filter, new Map(zooms.zooms.flatMap((z) => [...z.columns])));
    }
    const bounds: readonly (Bound | null)[] = zooms.zooms.map((zoom) =>
      filter === undefined
        ? true
        : columnsOf(filter).every((c) => zoom.columns.has(c))
          ? bind(filter, zoom.columns)
          : null,
    );

    let planned: readonly ScanTask[] | undefined;
    const plan = (): readonly ScanTask[] => {
      if (planned !== undefined) return planned;
      const tasks: ScanTask[] = [];
      zooms.zooms.forEach((zoom, z) => {
        const bound = bounds[z]!;
        if (bound === null) return;
        for (const entry of zoom.entries) {
          const left = residual(bound, entry);
          if (left === false) continue;
          const info = zoom.matrix.tiles[entry.tile]!;
          tasks.push({
            type: params.type,
            z,
            tile: entry.tile,
            rows: info.rows,
            bbox: info.bbox,
            residual: filterOf(left),
          });
        }
      });
      planned = tasks;
      return tasks;
    };

    const read = async (
      addresses: readonly TileAddress[],
      options: { readonly signal?: AbortSignal } = {},
    ): Promise<readonly Batch[]> => {
      const out: Batch[] = new Array(addresses.length);
      // Every address is checked before a statement is sent, so a bad one costs no read.
      const byZoom = new Map<number, { tile: number; at: number; left: Bound }[]>();
      addresses.forEach((address, at) => {
        const zoom = zooms.zoom(address);
        const bound = bounds[address.z]!;
        if (bound === null) {
          throw new CorpusReadError(
            `the filter names ${columnsOf(filter!).filter((c) => !zoom.columns.has(c)).join(', ')}, ` +
              `which a row at z = ${address.z} does not carry: a filter on a payload column has no ` +
              `answer below z = ${zooms.zooms.length - 1}`,
          );
        }
        const left = residual(bound, zoom.entries[address.tile]!);
        if (left === false) {
          out[at] = batchOf([], zoom.rung === null ? select : CELL_NAMES);
          return;
        }
        const wanted = byZoom.get(address.z) ?? [];
        wanted.push({ tile: address.tile, at, left });
        byZoom.set(address.z, wanted);
      });

      const statements: Promise<void>[] = [];
      for (const [z, wanted] of byZoom) {
        const zoom = zooms.zooms[z]!;
        const columns = zoom.rung === null ? select : CELL_NAMES;
        wanted.sort((a, b) => a.tile - b.tile);
        for (const run of runsOf(wanted)) {
          statements.push(readRun(zoom, columns, run, options.signal, out));
        }
      }
      await Promise.all(statements);
      return out;
    };

    /**
     * One statement for a run of consecutive tiles, split back into a batch per tile. The predicate
     * beyond the range is the tiles' shared residual, or the whole filter where they differ — which
     * is the same rows on every tile, since a residual is the filter with what the tile's statistics
     * settled taken out.
     */
    const readRun = async (
      zoom: Zoom,
      columns: readonly string[],
      run: readonly { tile: number; at: number; left: Bound }[],
      signal: AbortSignal | undefined,
      out: Batch[],
    ): Promise<void> => {
      const first = run[0]!.tile;
      const last = run[run.length - 1]!.tile;
      const span = BigInt(zoom.matrix.tileRows);
      const lo = BigInt(first) * span;
      const hi = (BigInt(last) + 1n) * span;
      const where = run.every((r) => sqlOf(r.left) === sqlOf(run[0]!.left)) ? run[0]!.left : bounds[zoom.matrix.z]!;
      const single = first === last;
      const keyed = single || columns.includes(zoom.key) ? columns : [...columns, zoom.key];
      const urls = [...new Set(run.map((r) => zoom.tileUrl(r.tile)))];
      const key = ident(zoom.key);
      const sql =
        `SELECT ${keyed.map(ident).join(', ')} FROM read_parquet(${urls.length === 1 ? lit(urls[0]!) : list(urls)}) ` +
        `WHERE ${key} >= ${lo} AND ${key} < ${hi}` +
        (where === true ? '' : ` AND ${sqlOf(where as Exclude<Bound, boolean>)}`);
      const batch = await reads.reads.batch(sql, keyed, signal);
      if (single) {
        for (const r of run) out[r.at] = batch;
        return;
      }
      const tiles = splitByTile(batch, zoom.key, Number(span), columns);
      for (const r of run) out[r.at] = tiles.get(r.tile) ?? batchOf([], columns);
    };

    return { params, plan, read };
  };
}

/** Maximal runs of consecutive tiles in a sorted list; a repeated tile stays in its run. */
function runsOf<T extends { readonly tile: number }>(sorted: readonly T[]): T[][] {
  const runs: T[][] = [];
  for (const item of sorted) {
    const run = runs[runs.length - 1];
    if (run !== undefined && item.tile - run[run.length - 1]!.tile <= 1) run.push(item);
    else runs.push([item]);
  }
  return runs;
}

/**
 * A run's answer as a batch per tile, by the aligned column: `tile = key / span`. The rows arrive
 * in key order — the file is written in it and DuckDB preserves insertion order — so each tile is a
 * slice; an engine that reorders is gathered instead.
 */
function splitByTile(
  batch: Batch,
  key: string,
  span: number,
  columns: readonly string[],
): Map<number, Batch> {
  const keys = batch.getChild(key)!.toArray();
  const tiles = Array.from({ length: batch.numRows }, (_, i) => Math.floor(Number(keys[i]) / span));
  const sorted = tiles.every((t, i) => i === 0 || tiles[i - 1]! <= t);
  const rows = new Map<number, number[]>();
  tiles.forEach((t, i) => {
    const at = rows.get(t);
    if (at === undefined) rows.set(t, [i]);
    else at.push(i);
  });
  const arrays = new Map(columns.map((name) => [name, batch.getChild(name)?.toArray()]));
  const pick = (values: ArrayLike<unknown>, at: readonly number[]): ArrayLike<unknown> => {
    const typed = ArrayBuffer.isView(values) ? (values as unknown as Uint8Array) : null;
    if (sorted) {
      const [from, to] = [at[0]!, at[at.length - 1]! + 1];
      return typed !== null ? typed.subarray(from, to) : Array.prototype.slice.call(values, from, to);
    }
    if (typed === null) return at.map((i) => values[i]);
    const into = new (typed.constructor as new (n: number) => Uint8Array)(at.length);
    at.forEach((i, k) => (into[k] = typed[i]!));
    return into;
  };
  const out = new Map<number, Batch>();
  for (const [tile, at] of rows) {
    out.set(tile, {
      numRows: at.length,
      getChild: (name) => {
        const values = arrays.get(name);
        return values === undefined || values === null ? null : { toArray: () => pick(values, at) };
      },
    });
  }
  return out;
}
