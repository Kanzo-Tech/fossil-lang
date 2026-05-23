// DuckDB-WASM lifecycle helpers — exposes `resetDuckDb()` which 07-08
// wires the Reset button to.
//
// Calling `worker.terminate()` is the ONLY mechanism that reclaims the
// DuckDB-WASM heap (P-MOD-1). The fossil-wasm LSP Worker (07-05) is
// NOT touched here — it has a different lifecycle (long-lived; cleared
// only on `LspChannel.dispose()`).

import { __terminateWorker, __clearForReset } from './runner';

/**
 * Terminate the DuckDB-WASM Worker and drop the singletons. After this
 * call, the next `ensureDuckDb()` / `runCompiledSql()` re-runs the
 * lazy-import + instantiate path (and re-pays the ~6.4MB download cost
 * only if the browser cache evicted it).
 *
 * Safe to call when no Worker has been instantiated (no-op).
 */
export function resetDuckDb(): void {
    __terminateWorker();
    __clearForReset();
}
