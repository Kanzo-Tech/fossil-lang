/// <reference lib="webworker" />
/**
 * DuckDB-WASM Worker entry placeholder.
 *
 * `@duckdb/duckdb-wasm@1.32.0` ships its own worker bootstrap inside the
 * package (`@duckdb/duckdb-wasm/dist/duckdb-browser-mvp.worker.js` and
 * sibling variants). The lazy-loaded path in {@link useDuckDb} uses
 * `selectBundle` + a Blob-URL importScripts() shim — NOT this file —
 * because the bundle selection happens on the main thread and the actual
 * Worker code is byte-shipped from the duckdb-wasm package.
 *
 * This module exists so consumers' bundlers (Vite, Webpack, Rspack) that
 * scan `new Worker(new URL('../workers/duckdb.worker.ts', import.meta.url))`
 * patterns still get a discoverable import target. Kept intentionally empty;
 * the real Worker code is the duckdb-wasm bundle.
 *
 * Per ADR-0026 — terminate+recreate on Reset. The hook owns that bookkeeping.
 */

// Intentional no-op; duckdb-wasm provides its own Worker code.
export {};
