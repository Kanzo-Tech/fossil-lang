/**
 * How the reader reaches the host's engine — **run SQL and hand back columns.**
 *
 * The host brings `@fossil-lang/types`' `Engine` and nothing else: `Engine.query` takes an
 * `AbortSignal` and answers in Arrow-shaped columns, so a stale statement is stopped rather than
 * dropped and no answer is turned into objects and back. This package links no engine and decodes
 * no Parquet; DuckDB-WASM in the browser is what `tests/engine.ts` drives.
 *
 * **Binding is absent, deliberately**: every value reaches SQL as a literal through the one escaper
 * each in `sql.ts`.
 */

import type { Engine, Table } from '@fossil-lang/types';

/** One row of a result, as a plain `{ column: value }` object. */
export type QueryRow = Record<string, unknown>;

/**
 * Rows as plain objects — what the verbs' WASM core and the members that walk objects take. Internal:
 * a host hands in an {@link Engine}, and this is that engine's answer turned into rows.
 */
export type QueryFn = (sql: string) => Promise<QueryRow[]>;

/**
 * **A read's answer, in columns** — what `scan.read` hands back. apache-arrow's `Table` is one, so
 * an engine's answer passes through untouched.
 */
export interface Batch {
  readonly numRows: number;
  getChild(name: string): { toArray(): ArrayLike<unknown> } | null;
}

/**
 * **How every module reads**: rows for the members that walk objects, batches for the ones a view
 * draws.
 */
export interface Reads {
  readonly rows: QueryFn;
  /**
   * One statement's answer as a {@link Batch}, carrying `columns` even when it has no row. An
   * aborted signal rejects with its reason — an `AbortError` — whether or not the engine stopped
   * the statement, so a stale answer never reaches a caller that moved on.
   */
  batch(sql: string, columns: readonly string[], signal?: AbortSignal): Promise<Batch>;
}

/** Rows out of an engine's columns — for the members that still walk objects. */
export function rowsOfTable(table: Table): QueryRow[] {
  const names = table.schema.fields.map((f) => f.name);
  const columns = names.map((name) => table.getChild(name));
  return Array.from({ length: table.numRows }, (_, i) => {
    const row: QueryRow = {};
    names.forEach((name, k) => {
      row[name] = columns[k]?.get(i) ?? null;
    });
    return row;
  });
}

/** Columns out of rows, `columns` named even when there is no row. */
export function batchOf(rows: readonly QueryRow[], columns: readonly string[]): Batch {
  const names = new Set([...columns, ...rows.flatMap((row) => Object.keys(row))]);
  return {
    numRows: rows.length,
    getChild: (name) => (names.has(name) ? { toArray: () => rows.map((row) => row[name]) } : null),
  };
}

/** Reads through the host's {@link Engine}: the signal reaches the running statement. */
export function engineReads(engine: Engine): Reads {
  return {
    rows: async (sql) => rowsOfTable(await engine.query(sql)),
    async batch(sql, _columns, signal) {
      signal?.throwIfAborted();
      const table = await (signal === undefined ? engine.query(sql) : engine.query(sql, { signal }));
      signal?.throwIfAborted();
      return table;
    },
  };
}

/** Maximal runs of consecutive tiles in a sorted list; a repeated tile stays in its run. */
export function runsOf<T extends { readonly tile: number }>(sorted: readonly T[]): T[][] {
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
export function splitByTile(
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
