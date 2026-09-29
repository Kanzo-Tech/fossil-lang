/**
 * Iceberg's `Table.scan` over a tile matrix set: a scan is a type, a filter and a projection,
 * bound once; `plan` is `plan_files` over the published statistics at every zoom; `read` is one
 * task, one tile, one statement.
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
import { CorpusReadError, ident, lit } from './sql.js';
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
   * **One tile, as a columnar batch.** Any address in the matrix, planned or not — a view builds
   * `{ type, z, tile }` from what it sees. The filter's residual on that tile is applied; a tile
   * its statistics exclude answers with no rows and no request.
   *
   * **How one tile is addressed.** DuckDB's Parquet reader has no read by row group, so a tile is
   * selected by ONE conjunctive range on the column its rows are a range of — `dense_id` at `Z`,
   * `cell_id` below — which the engine prunes against every row group's statistics in the footer.
   * Under `rowgroups` that is the tile's one row group and no other; under `files` the file is the
   * tile and the range is a no-op. Never a disjunction, which the engine evaluates per row.
   *
   * @throws {CorpusReadError} for an address outside the matrix, or at a zoom the filter cannot
   *   apply to. An abort rejects with the signal's reason.
   */
  read(task: TileAddress, options?: { readonly signal?: AbortSignal }): Promise<Batch>;
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
      task: TileAddress,
      options: { readonly signal?: AbortSignal } = {},
    ): Promise<Batch> => {
      const zoom: Zoom = zooms.zoom(task);
      const bound = bounds[task.z]!;
      if (bound === null) {
        throw new CorpusReadError(
          `the filter names ${columnsOf(filter!).filter((c) => !zoom.columns.has(c)).join(', ')}, ` +
            `which a row at z = ${task.z} does not carry: a filter on a payload column has no ` +
            `answer below z = ${zooms.zooms.length - 1}`,
        );
      }
      const columns = zoom.rung === null ? select : CELL_NAMES;
      const left = residual(bound, zoom.entries[task.tile]!);
      if (left === false) return batchOf([], columns);
      const span = BigInt(zoom.matrix.tileRows);
      const lo = BigInt(task.tile) * span;
      const key = ident(zoom.key);
      const sql =
        `SELECT ${columns.map(ident).join(', ')} FROM read_parquet(${lit(zoom.tileUrl(task.tile))}) ` +
        `WHERE ${key} >= ${lo} AND ${key} < ${lo + span}` +
        (left === true ? '' : ` AND ${sqlOf(left)}`);
      return reads.reads.batch(sql, columns, options.signal);
    };

    return { params, plan, read };
  };
}
