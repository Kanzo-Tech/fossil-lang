import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB, type DuckDBBindings } from '@duckdb/duckdb-wasm/blocking';
import type { Engine, Table } from '@fossil-lang/types';


const require = createRequire(import.meta.url);
// The Arrow DuckDB-WASM decodes with, reached through it rather than declared beside it: a second
// copy would be a second `Table` class, and nothing here needs one of its own.
const arrow = createRequire(require.resolve('@duckdb/duckdb-wasm'))('apache-arrow') as {
  tableFromIPC(bytes: Uint8Array): Table;
};

/**
 * A DuckDB-WASM in this process, and the {@link Engine} over it that the page's would be.
 *
 * **The interrupt is real.** The node bindings are blocking, so `conn.query` cannot be stopped from
 * JavaScript at all. This drives the pending-query API underneath — `startPendingQuery`, then
 * `pollPendingQuery` between turns of the event loop, then `cancelPendingQuery` on abort — which is
 * what `AsyncDuckDBConnection.send` and `cancelSent` do across a worker, so an abort here stops the
 * statement DuckDB is executing and the connection answers the next one.
 */
export async function duckdb(spill?: string): Promise<{
  readonly db: DuckDBBindings;
  readonly engine: Engine;
  /** Rows as plain objects, for a test that asks the engine something directly. */
  readonly query: (sql: string) => Promise<Record<string, unknown>[]>;
}> {
  const dist = dirname(require.resolve('@duckdb/duckdb-wasm'));
  const db = await createDuckDB(
    {
      mvp: { mainModule: resolve(dist, './duckdb-mvp.wasm'), mainWorker: resolve(dist, './duckdb-node-mvp.worker.cjs') },
      eh: { mainModule: resolve(dist, './duckdb-eh.wasm'), mainWorker: resolve(dist, './duckdb-node-eh.worker.cjs') },
    },
    new ConsoleLogger(),
    NODE_RUNTIME,
  );
  await db.instantiate();
  const conn = db.connect();
  // What `Engine` asks of a host: the footer is read once per file, not once per statement.
  conn.query(`SET parquet_metadata_cache = true`);
  if (spill !== undefined) conn.query(`SET temp_directory = '${spill}'`);
  const engine: Engine = {
    query: (sql, options = {}) =>
      conn.useUnsafe(async (bindings, id): Promise<Table> => {
        const { signal } = options;
        signal?.throwIfAborted();
        let header = bindings.startPendingQuery(id, sql, false);
        while (header === null) {
          await new Promise((next) => setImmediate(next));
          if (signal?.aborted) {
            bindings.cancelPendingQuery(id);
            throw signal.reason;
          }
          header = bindings.pollPendingQuery(id);
        }
        const chunks: Uint8Array[] = [header];
        for (;;) {
          const chunk = bindings.fetchQueryResults(id);
          if (chunk === null) continue;
          if (chunk.length === 0) break;
          chunks.push(chunk);
        }
        const bytes = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
        let at = 0;
        for (const chunk of chunks) {
          bytes.set(chunk, at);
          at += chunk.length;
        }
        return arrow.tableFromIPC(bytes);
      }),
    lend: async () => {},
    drop: async () => {},
  };
  const query = async (sql: string): Promise<Record<string, unknown>[]> =>
    conn.query(sql).toArray().map((row: { toJSON(): Record<string, unknown> }) => row.toJSON());
  return { db, engine, query };
}
