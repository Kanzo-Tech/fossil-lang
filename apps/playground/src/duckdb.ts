/**
 * The engine the host brings.
 *
 * `@fossil-lang/corpus` asks a host for exactly one capability — `query(sql) => rows` — and
 * takes no engine dependency itself. This is that capability, wired to DuckDB-WASM. It is
 * used for two unrelated jobs and it is the same connection for both:
 *
 * 1. **Introspection**, before the check: `DESCRIBE SELECT * FROM read_csv_auto(…)` is what
 *    tells the compiler what columns `users.csv` has. That is `fossil-introspect`'s job
 *    natively and `@fossil-lang/introspect`'s in JS.
 * 2. **Reading the corpus back**, after the run.
 *
 * ## Self-hosted, because a CDN would be a server
 *
 * duckdb-wasm's `getJsDelivrBundles()` is the documented quick start and it fetches the
 * worker and the `.wasm` from jsdelivr. This app's entire claim is that nothing leaves the
 * machine, and a page that phones a CDN on boot has already broken it in the only way a
 * user could observe. So the bundles are `?url` imports out of `node_modules` and Vite
 * emits them as assets of this app.
 *
 * The `eh` build (exception handling) is the one picked, unconditionally: every browser
 * that can run the fossil bundles has had wasm exception-handling since 2021, and the
 * `mvp` fallback exists for browsers this app does not otherwise support. If that stops
 * being true, `duckdb.selectBundle` is the two-line replacement.
 */
import * as duckdb from '@duckdb/duckdb-wasm';
import ehWasm from '@duckdb/duckdb-wasm/dist/duckdb-eh.wasm?url';
import ehWorker from '@duckdb/duckdb-wasm/dist/duckdb-browser-eh.worker.js?url';
import type { QueryFn, QueryRow } from '@fossil-lang/corpus';
import type { Engine } from '@fossil-lang/types';

import type { BundleCost } from './check.js';

// Re-exported so the rest of the app names the capability once, from here, rather than
// each module reaching for the graph package to spell one callback type.
export type { QueryFn, QueryRow };

let db: duckdb.AsyncDuckDB | null = null;
let conn: duckdb.AsyncDuckDBConnection | null = null;
let cost: BundleCost | null = null;

/** The measured cost of the DuckDB bundle, or `null` before {@link boot}. */
export function duckdbCost(): BundleCost | null {
  return cost;
}

/** Boot DuckDB-WASM in a worker. Idempotent. */
export async function boot(): Promise<void> {
  if (db) return;
  const started = performance.now();
  const wasmBytes = await (await fetch(ehWasm)).arrayBuffer();
  const worker = new Worker(ehWorker, { type: 'module' });
  // `VOID` rather than `ConsoleLogger`: the console is where the app's own diagnostics go,
  // and duckdb's per-query chatter buries them.
  const instance = new duckdb.AsyncDuckDB(new duckdb.VoidLogger(), worker);
  await instance.instantiate(URL.createObjectURL(new Blob([wasmBytes], { type: 'application/wasm' })));
  db = instance;
  conn = await instance.connect();
  cost = { bytes: wasmBytes.byteLength, ms: Math.round(performance.now() - started) };
}

/**
 * The booted instance and its connection, for a consumer that needs the OBJECTS and not the SQL.
 *
 * Exactly one such consumer exists: `src/mosaic.ts`, which hands both to Mosaic's `wasmConnector`
 * so the crossfilter runs on this engine instead of booting a second one. Everything else in the
 * app goes through {@link query}, and should — handing out the connection is handing out the
 * ability to bypass the one capability `@fossil-lang/corpus` asks a host for.
 *
 * They throw rather than returning `null` because every caller is downstream of {@link boot} and a
 * `null` here would surface as a coordinator that silently answers nothing.
 */
export function instance(): duckdb.AsyncDuckDB {
  if (!db) throw new Error('duckdb not booted');
  return db;
}

export function connection(): duckdb.AsyncDuckDBConnection {
  if (!conn) throw new Error('duckdb not booted');
  return conn;
}

/** Whether {@link boot} has run — so a consumer can wait rather than throw. */
export function booted(): boolean {
  return db !== null && conn !== null;
}

/**
 * Stage a file in DuckDB's virtual filesystem under the name SQL will call it.
 *
 * This is the seam that makes the whole corpus half work without a server. The executor
 * hands back `{rel_path, bytes}`; registering each one under `<dest>/<rel_path>` means
 * `read_parquet('corpus/vertex/Person/tiles.parquet')` resolves to bytes that were never
 * written anywhere. `open`'s tile arithmetic produces those same names, so the
 * reader cannot tell the difference between this and an HTTP corpus — which is the point.
 */
export async function register(path: string, bytes: Uint8Array): Promise<void> {
  if (!db) throw new Error('duckdb not booted');
  // A re-run re-registers the same names. `registerFileBuffer` overwrites, but a handle
  // DuckDB has already opened in this session keeps the old bytes, so it is dropped first.
  try {
    await db.dropFile(path);
  } catch {
    // Not registered yet — the first run. Nothing to drop.
  }
  // `new Uint8Array(bytes)` is a COPY, and it is not defensive style — it is the fix for a
  // real crash. `registerFileBuffer` posts the buffer to the DuckDB worker as a TRANSFER,
  // which detaches it in this thread. `users.csv` is registered here for introspection and
  // then handed to `FossilExecutor.run` afterwards, so without the copy the executor
  // receives a detached view and dies inside wasm-bindgen's memcpy with
  // "Cannot perform %TypedArray%.prototype.set on a detached or out-of-bounds ArrayBuffer".
  // Copying costs one memcpy per file and keeps the caller's bytes usable.
  await db.registerFileBuffer(path, new Uint8Array(bytes));
}

export const query: QueryFn = async (sql: string): Promise<QueryRow[]> => {
  if (!conn) throw new Error('duckdb not booted');
  const table = await conn.query(sql);
  return table.toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());
};

/**
 * The same query, for the app's own result table.
 *
 * A `bigint` cannot be rendered by React and a `Uint8Array` should not be, so this is the
 * lossy display path and `query` above stays the honest one.
 */
export async function queryForDisplay(sql: string): Promise<{ columns: string[]; rows: string[][] }> {
  const rows = await query(sql);
  const columns = rows.length > 0 ? Object.keys(rows[0]!) : [];
  return {
    columns,
    rows: rows.map((row) => columns.map((c) => (row[c] === null || row[c] === undefined ? '' : String(row[c])))),
  };
}

const lent = new Map<string, string>();

/**
 * This page's engine, as `@fossil-lang/types` asks for one — what `@kanzo-tech/mosaic`'s
 * `engine()` is for a host that has adopted it.
 *
 * `lend` is the drop-then-register this file always did, done only when the URL behind a name
 * changed: DuckDB-WASM refuses a second URL for a registered name, and a lease renewed under a
 * fresh signature is exactly a second URL. `DuckDBDataProtocol.HTTP` makes the engine issue its
 * own `Range` requests, and `directIO: false` lets it keep what it has read.
 */
export const engine: Engine = {
  query,
  async lend(files) {
    for (const [name, url] of Object.entries(files)) {
      if (lent.get(name) === url) continue;
      if (lent.has(name)) await instance().dropFile(name);
      await instance().registerFileURL(name, url, duckdb.DuckDBDataProtocol.HTTP, false);
      lent.set(name, url);
    }
  },
  async drop(names) {
    for (const name of names) {
      if (!lent.delete(name)) continue;
      await instance().dropFile(name);
    }
  },
};
