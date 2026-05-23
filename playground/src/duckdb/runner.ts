// DuckDB-WASM lazy-load runner (07-06 / SC#2 first half).
//
// Pressing the Run button is the FIRST event that loads
// `@duckdb/duckdb-wasm` (~6.4MB MVP bundle). The module is reached via a
// dynamic `import()` so Vite emits it in a separate chunk that the
// initial page load does NOT pay for. The DuckDB Worker spawned by
// `instantiate()` is a separate Worker from the long-lived fossil-wasm
// LSP Worker (07-05) — two Workers, two lifecycles.
//
// Bundle selection: `getJsDelivrBundles()` + `selectBundle()` —
// NEVER hand-picked (Pitfall 3, 07-RESEARCH). The picked bundle's
// `mainWorker` URL points at jsDelivr; the worker module loads the
// `.wasm` from the same CDN.
//
// 07-08 wires the Reset button via `./lifecycle.ts` (terminates the
// Worker + clears the singletons).

// `apache-arrow` is pinned at the top-level (16.1.0) but DuckDB-WASM
// nests its OWN apache-arrow copy under
// `@duckdb/duckdb-wasm/node_modules/apache-arrow`. Two structurally
// equivalent Table types resolve to two different declarations from
// tsc's POV — which it flags as incompatible (StructRow private
// symbols).
//
// To stay decoupled from either nested-module path AND from the
// top-level pin, we re-export the exact return type of DuckDB-WASM's
// `AsyncDuckDBConnection.query`. That's the source-of-truth for what
// downstream code (07-07 Mosaic adapter) actually consumes.
import type { AsyncDuckDBConnection } from '@duckdb/duckdb-wasm';
type ArrowQueryResult = Awaited<ReturnType<AsyncDuckDBConnection['query']>>;

let duckdbModulePromise: Promise<typeof import('@duckdb/duckdb-wasm')> | null = null;
let dbInstance: import('@duckdb/duckdb-wasm').AsyncDuckDB | null = null;
let workerInstance: Worker | null = null;

/** Progress callback: 0..1, monotone non-decreasing within one call. */
export type ProgressFn = (p: number) => void;

/**
 * Lazy-import + instantiate the DuckDB-WASM module. Idempotent: subsequent
 * calls return the cached instance (the Worker stays alive between Runs).
 */
export async function ensureDuckDb(progress: ProgressFn = () => {}): Promise<import('@duckdb/duckdb-wasm').AsyncDuckDB> {
    if (dbInstance) return dbInstance;
    progress(0.1);
    if (!duckdbModulePromise) {
        duckdbModulePromise = import('@duckdb/duckdb-wasm');
    }
    const duckdb = await duckdbModulePromise;
    progress(0.3);
    const bundles = duckdb.getJsDelivrBundles();
    const bundle = await duckdb.selectBundle(bundles); // Pitfall 3 — NEVER hand-pick.
    progress(0.5);
    if (!bundle.mainWorker) {
        throw new Error('DuckDB-WASM: selectBundle returned no mainWorker URL');
    }
    workerInstance = new Worker(bundle.mainWorker, { type: 'module' });
    const logger = new duckdb.ConsoleLogger();
    dbInstance = new duckdb.AsyncDuckDB(logger, workerInstance);
    await dbInstance.instantiate(bundle.mainModule, bundle.pthreadWorker);
    progress(1.0);
    return dbInstance;
}

export interface RunResult {
    vertices: ArrowQueryResult;
    edges: ArrowQueryResult;
    rowCount: number;
}

/**
 * Execute compiled SQL against an in-memory CSV registered as `input.csv`.
 *
 * Pipeline: registerFileText('input.csv', csvText) → conn.query(sql) →
 * `SELECT * FROM "output/vertex.parquet"` + `SELECT * FROM
 * "output/edge.parquet"` → returns Arrow tables. Output is the
 * in-memory virtual-file Parquet emitted by the COPY ... TO statements
 * — no real FS touch.
 */
export async function runCompiledSql(
    sql: string,
    csvText: string,
    progress: ProgressFn = () => {},
): Promise<RunResult> {
    const db = await ensureDuckDb(progress);
    const conn = await db.connect();
    try {
        await db.registerFileText('input.csv', csvText);
        await conn.query(sql);
        const vertices = await conn.query('SELECT * FROM "output/vertex.parquet"');
        const edges = await conn.query('SELECT * FROM "output/edge.parquet"');
        return {
            vertices,
            edges,
            rowCount: vertices.numRows + edges.numRows,
        };
    } finally {
        await conn.close();
    }
}

/**
 * Internal — only `./lifecycle.ts` calls these. Terminates the DuckDB
 * Worker (the only mechanism that reclaims the WASM heap — P-MOD-1).
 */
export function __terminateWorker(): void {
    if (workerInstance) {
        workerInstance.terminate();
        workerInstance = null;
    }
}

/**
 * Internal — only `./lifecycle.ts` calls this. Clears the singletons so
 * the next `ensureDuckDb()` call re-runs the lazy-import + instantiate
 * path from scratch.
 */
export function __clearForReset(): void {
    dbInstance = null;
    workerInstance = null;
    duckdbModulePromise = null;
}
