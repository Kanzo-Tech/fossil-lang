/**
 * Iceberg's `Table.scan` over one table of the corpus: a table, a filter, a projection and a limit,
 * bound once; `plan()` says what to read and `read(tasks)` reads it, one statement per task.
 *
 * **One task today, always.** A table is one Parquet file and DuckDB prunes its row groups from the
 * footer, so there is nothing for a planner to split. `plan()` exists so that the day a table is
 * read in pieces — tiles, or a table larger than a tab — the pieces are tasks and no caller changes.
 */

import { FossilError, type Engine } from '@fossil-lang/types';

import { bind, sqlOf, unknownColumn, type Bound, type Filter } from './filter.js';
import type { EdgeTable, VertexTable } from './manifest.js';
import { ident, query } from './sql.js';

/** What {@link Corpus.scan} takes. */
export interface ScanParams {
  /** A `name` from `vertex_tables` or `edge_tables`. */
  readonly table: string;
  /** Rows to keep, as data. Bound when the scan is built. */
  readonly filter?: Filter;
  /** The columns to read — every column when absent. */
  readonly select?: readonly string[];
  readonly limit?: number;
}

/** One planned read. */
export interface ScanTask {
  readonly table: string;
  /** The file it reads, relative to the corpus root. */
  readonly path: string;
  /** The table's `record_count` — the most a read of this task returns. */
  readonly rows: number;
}

/**
 * **A read's answer, in columns.** apache-arrow's `Table` is one, so an engine's answer passes
 * through untouched; a fixed-width column's `toArray()` is a typed array.
 */
export interface Batch {
  readonly numRows: number;
  getChild(name: string): { toArray(): ArrayLike<unknown> } | null;
}

/** A scan, bound. See {@link Corpus.scan}. */
export interface Scan {
  readonly params: ScanParams;
  /** What to read. Synchronous: the manifest was read when the corpus opened. */
  plan(): readonly ScanTask[];
  /**
   * **One batch per task, in the order given**, each one `SELECT` over the table's view.
   *
   * @throws {FossilError} `api/invalid-argument` for a task of another table, `engine/failed` when the
   *   engine refuses the statement, the storage failure of a job's credential that expired and could
   *   not be renewed. An abort rejects with the signal's reason, whether or not the
   *   engine stopped the statement.
   */
  read(tasks: readonly ScanTask[], options?: { readonly signal?: AbortSignal }): Promise<readonly Batch[]>;
}

/** Every table the manifest declares, by name. */
export type Tables = ReadonlyMap<string, VertexTable | EdgeTable>;

/** `live` throws why the corpus can no longer be read — a credential that expired — before a read. */
export function scanOf(engine: Engine, relation: (table: string) => string, tables: Tables, live: () => void) {
  return (params: ScanParams): Scan => {
    const table = tables.get(params.table);
    if (table === undefined) {
      const names = [...tables.keys()];
      throw FossilError.of(
        'corpus/unknown-table',
        { table: params.table, tables: names },
      );
    }
    const declared = new Set(table.properties.map((p) => p.name));
    const { select, limit, filter } = params;
    if (select !== undefined) {
      if (select.length === 0) {
        throw FossilError.of(
          'corpus/empty-projection',
          { table: table.name },
        );
      }
      for (const column of select) {
        if (!declared.has(column)) throw unknownColumn(table.name, column, [...declared]);
      }
    }
    if (limit !== undefined && !(Number.isSafeInteger(limit) && limit >= 0)) throw notACount(limit);
    const position = 'position' in table ? table.position : undefined;
    const bound: Bound | undefined =
      filter === undefined ? undefined : bind(filter, { table: table.name, properties: table.properties, position });
    const sql =
      `SELECT ${select === undefined ? '*' : select.map(ident).join(', ')} FROM ${relation(table.name)}` +
      (bound === undefined ? '' : ` WHERE ${sqlOf(bound)}`) +
      (limit === undefined ? '' : ` LIMIT ${limit}`);
    const task: ScanTask = { table: table.name, path: table.path, rows: table.record_count };

    return {
      params,
      plan: () => [task],
      async read(tasks, options = {}) {
        for (const t of tasks) {
          if (t.table !== table.name) {
            throw FossilError.of(
              'api/invalid-argument',
              { argument: 'tasks', expected: `tasks of ${table.name}` },
            );
          }
        }
        const { signal } = options;
        live();
        const out: Batch[] = [];
        for (let i = 0; i < tasks.length; i += 1) {
          signal?.throwIfAborted();
          out.push(await query(engine, sql, signal));
          signal?.throwIfAborted();
        }
        return out;
      },
    };
  };
}

/** A limit that is not a count of rows. */
export function notACount(limit: number): FossilError<'api/invalid-argument'> {
  return FossilError.of(
    'api/invalid-argument',
    { argument: 'limit', expected: 'a count of rows' },
  );
}
