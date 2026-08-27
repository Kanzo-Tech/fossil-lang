/**
 * The engine the host brings.
 *
 * `@fossil-lang/graph` asks a host for exactly one capability — `query(sql) => rows` — and
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
import type { BundleCost } from './check.js';

/**
 * The query capability, restated rather than imported — and that is a REPORTABLE GAP, not
 * a preference.
 *
 * `@fossil-lang/graph`'s `./corpus` subpath exports `openCorpus` and its result types but
 * not `QueryFn`/`QueryRow`: those are re-exported only from the root barrel, and the root
 * barrel static-imports `../pkg/fossil_graph_wasm.js`. So a host that wants the corpus API
 * WITHOUT the WASM — the whole reason `./corpus` has a subpath — cannot name the one type
 * it has to implement. Structural typing means the callback still fits; naming it does not.
 *
 * The one-line fix is `export type { QueryFn, QueryRow } from './query.js';` in
 * `packages/graph/src/corpus.ts`. That package is another agent's, so it is reported here
 * instead of edited.
 */
export type QueryRow = Record<string, unknown>;
export type QueryFn = (sql: string) => Promise<QueryRow[]>;

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
 * The one capability, as `@fossil-lang/graph` spells it.
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
