/**
 * useDuckDb — lazy-loaded DuckDB-WASM Worker + Run hook.
 *
 * Per ADR-0026 (Two Workers, Two Lifecycles — DuckDB Worker is terminate +
 * recreate on Reset) + ADR-0025 (`@duckdb/duckdb-wasm` exact-pinned to
 * 1.32.0 across the workspace) + RESEARCH.md Pattern 4 (WebGL/heap-heavy
 * resources live OUTSIDE React state — the reconciler churns React state).
 *
 * Lazy-load: the ~6.4 MB duckdb-wasm bundle is dynamically imported on
 * first call to {@link getDuckDb}. Subsequent calls return the memoised
 * instance. {@link resetDuckDb} terminates the Worker; the next
 * {@link getDuckDb} call re-loads (browser cache typically keeps the
 * payload around so the network cost is zero, but WASM compile + instantiate
 * is real — ~hundreds of ms on a 2020-era MacBook per ADR-0026's budget).
 */

import { useCallback, useState } from 'react';

/**
 * Module-scope Worker + DB instance — survives React unmount/remount.
 * `any` types because @duckdb/duckdb-wasm exposes a class-heavy surface
 * that's bulky to import statically (the whole point of lazy-loading is
 * to keep its types off the initial-render path). Specific call sites
 * narrow as needed.
 */
let _worker: Worker | null = null;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let _db: any | null = null;

/**
 * Lazy-load `@duckdb/duckdb-wasm` and return a memoised `AsyncDuckDB`.
 *
 * The first call pays:
 *   1. The dynamic `import('@duckdb/duckdb-wasm')` (~6.4 MB; browser cache
 *      typically zero-cost after first load).
 *   2. `selectBundle` (~ms; just picks the bundle variant for this UA).
 *   3. The Worker spawn + `instantiate(mainModule, pthreadWorker)` — the
 *      real cost (~hundreds of ms).
 *
 * Subsequent calls return the same instance until {@link resetDuckDb} is
 * called.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export async function getDuckDb(): Promise<any> {
  if (_db) return _db;
  const duckdb = await import('@duckdb/duckdb-wasm');
  const bundles = duckdb.getJsDelivrBundles();
  const bundle = await duckdb.selectBundle(bundles);
  if (!bundle.mainWorker) {
    throw new Error('DuckDB-WASM: selectBundle returned no mainWorker (unsupported UA?)');
  }
  // The Blob-URL importScripts() shim is the @duckdb/duckdb-wasm-documented
  // pattern for spawning the bundled worker without bundler magic.
  const worker_url = URL.createObjectURL(
    new Blob([`importScripts("${bundle.mainWorker}");`], { type: 'text/javascript' }),
  );
  _worker = new Worker(worker_url);
  URL.revokeObjectURL(worker_url);
  const logger = new duckdb.ConsoleLogger();
  _db = new duckdb.AsyncDuckDB(logger, _worker);
  await _db.instantiate(bundle.mainModule, bundle.pthreadWorker);
  return _db;
}

/**
 * Terminate the DuckDB Worker + clear the memoised DB. Per ADR-0026 +
 * PLAY-12 — the only reliable mechanism to reclaim the ~6.4 MB DuckDB-WASM
 * heap (research §"Don't Hand-Roll" — DROP TABLE / CHECKPOINT / pragma
 * memory_limit do NOT release the WASM linear memory back to the OS).
 *
 * Idempotent — calling on a never-loaded resolver is a no-op.
 */
export function resetDuckDb(): void {
  if (_worker) {
    _worker.terminate();
    _worker = null;
    _db = null;
  }
}

/**
 * React hook surface. Returns `{ run, loading, error }` — `run(sql)` lazily
 * boots DuckDB on first call, executes the query, and returns the Arrow
 * result. `loading` flips true during execution; `error` captures the most
 * recent failure (consumers can surface it via `role="alert"`).
 */
export function useDuckDb(): {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  run: (sql: string) => Promise<any>;
  loading: boolean;
  error: Error | null;
} {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);

  const run = useCallback(async (sql: string) => {
    setLoading(true);
    setError(null);
    try {
      const db = await getDuckDb();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const conn = await (db as any).connect();
      const result = await conn.query(sql);
      await conn.close();
      return result;
    } catch (e) {
      const err = e instanceof Error ? e : new Error(String(e));
      setError(err);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  return { run, loading, error };
}
