/**
 * The capability this package asks a host for — **run SQL and hand back rows.**
 *
 * {@link QueryFn} is the engine and it is the only thing the door needs.
 *
 * It lived in `client.ts` as the verb surface's `QueryFn`, reachable only through a barrel that
 * static-imports the wasm-bindgen output. {@link open} needs the same thing and must not need
 * the WASM, so the type moved here — one spelling, two callers, and `client.ts` re-exports it so
 * nothing downstream changed name.
 *
 * **Why a callback and not a decoder.** Three of the four members of the corpus API have to decode
 * Parquet, and this package has zero runtime dependencies. Bundling a decoder would make it a
 * driver; injecting one makes it a seam. The host already has the engine — DuckDB-WASM in the
 * browser (`tests/e2e.test.ts` boots it). And an engine buys more than
 * decoding: DuckDB prunes Parquet row groups off the footer statistics by itself, which is the one
 * piece of the read path that would otherwise have to be written twice.
 *
 * **One method, and that is deliberate.** A capability with five methods is a driver. Everything
 * the corpus API needs — the manifest YAMLs (`read_text`), the per-tile boxes (`parquet_metadata`)
 * and the payload (`read_parquet`, over a list of paths in one call) — is SQL over the same
 * filesystem layer, so a host that can reach the tiles can already reach the rest. That is measured
 * rather than assumed: `@duckdb/duckdb-wasm@1.32.0` bundles DuckDB v1.4.3 and answers all three
 * against local paths under `NODE_RUNTIME`, which is what `tests/corpus.test.ts` runs on, and the
 * `duckdb` binary the corpus guards use answers them too. **In particular there is no separate
 * `fetch` capability**, because there is nothing a separate one could reach that the engine cannot:
 * it has to see the tiles or it cannot answer a window.
 *
 * **What it does not carry, and what replaced it.** It has no cancellation, no columns and no
 * parameter binding. The first two are {@link Engine}'s now — `Engine.query` takes an
 * `AbortSignal` and answers in Arrow-shaped columns — and a corpus opened with an engine reads
 * through that; a `query` callback is kept for the members `/docs/design/backend` retires in step
 * 5, and a read through it drops a stale answer rather than stopping it. Binding is still absent,
 * and deliberately: every value reaches SQL as a literal through the one escaper each in `sql.ts`.
 */

import type { Engine, Table } from '@fossil-lang/types';

/** One row of a result, as a plain `{ column: value }` object. */
export type QueryRow = Record<string, unknown>;

/**
 * The host's query callback. Runs SQL and resolves the rows as plain objects.
 *
 * In keasy this wraps the Mosaic coordinator, e.g.:
 *
 * ```ts
 * const query: QueryFn = async (sql) => {
 *   const table = await coordinator.query(sql, { type: 'arrow' });
 *   return table.toArray().map((r) => r.toJSON());
 * };
 * ```
 *
 * (Mosaic returns an Arrow table; the binding's WASM core expects row objects, so the host adapts
 * once here — keeping this package free of an Arrow/Mosaic dependency.)
 *
 * **What a value in a row may be is not narrowed, and that is not laziness.** A host decides the width
 * it hands back: DuckDB-WASM gives a `UINTEGER` as a `Number` and a `UBIGINT` as a `BigInt`, and a
 * host that goes through JSON turns both into numbers. So a `dense_id` arrives as a `number`, a
 * `bigint` or a string depending on the host and the column's declared
 * width, and the corpus API coerces it at the boundary rather than trusting any of them.
 */
export type QueryFn = (sql: string) => Promise<QueryRow[]>;

/**
 * **A read's answer, in columns** — what `scan.read` hands back. apache-arrow's `Table` is one, so
 * an engine's answer passes through untouched; a `query` callback's rows are turned into one.
 */
export interface Batch {
  readonly numRows: number;
  getChild(name: string): { toArray(): ArrayLike<unknown> } | null;
}

/**
 * **How every module reads**, whichever the host gave: rows for the members that walk objects,
 * batches for the ones a view draws.
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

/** Reads through a `query` callback, which cannot stop a statement and so drops a stale answer. */
export function callbackReads(query: QueryFn): Reads {
  return {
    rows: query,
    async batch(sql, columns, signal) {
      signal?.throwIfAborted();
      const rows = await query(sql);
      signal?.throwIfAborted();
      return batchOf(rows, columns);
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
