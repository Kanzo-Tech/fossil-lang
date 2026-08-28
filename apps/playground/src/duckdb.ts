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
 * Stage a file in DuckDB's virtual filesystem under the name SQL will call it.
 *
 * This is the seam that makes the whole corpus half work without a server. The executor
 * hands back `{rel_path, bytes}`; registering each one under `<dest>/<rel_path>` means
 * `read_parquet('corpus/vertex/Person.parquet')` resolves to bytes that were never
 * written anywhere. `openCorpus`'s tile arithmetic produces those same names, so the
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

/**
 * Point DuckDB at a URL on this app's own origin, without downloading it here.
 *
 * The counterpart to {@link register}: that one hands DuckDB bytes, this one hands it an
 * address and lets DuckDB fetch what it needs. `DuckDBDataProtocol.HTTP` makes the engine
 * issue its own `Range` requests, which is the only way to read a footer out of a 21 MB
 * Parquet file without pulling the 21 MB.
 *
 * Used by the streaming panel for exactly one job — reading the bench corpus's footer, once —
 * and deliberately not for the payload. See the note in `src/stream.ts`: requests the engine
 * makes happen inside a Worker where this thread cannot weigh them, and a panel about bytes
 * has to weigh its own bytes.
 *
 * `false` is `directIO`: DuckDB caches what it has already read, which is what makes the
 * footer bought-once rather than bought-per-query.
 */
export async function registerUrl(path: string, url: string): Promise<void> {
  if (!db) throw new Error('duckdb not booted');
  try {
    await db.dropFile(path);
  } catch {
    // Not registered yet.
  }
  await db.registerFileURL(path, url, duckdb.DuckDBDataProtocol.HTTP, false);
}

/**
 * The one capability, as `@fossil-lang/corpus` spells it.
 *
 * Arrow in, plain row objects out — the binding's contract is `Record<string, unknown>`
 * and it coerces widths at its own boundary, precisely because hosts disagree about
 * whether a `UINTEGER` arrives as a `Number` or a `BigInt`. So nothing is normalised here.
 */
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
