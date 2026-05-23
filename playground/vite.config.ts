import { defineConfig } from 'vite';
import compression from 'vite-plugin-compression';

// Vite 7 config for the Fossil playground (per 07-RESEARCH §"Vite config"
// + 07-04 plan interfaces). The four load-bearing knobs:
//
//   1. `vite-plugin-compression` emits a `.br` sidecar next to each chunk
//      >1KB. The Phase 7 SC#3 size budget (<10MB post-brotli) is measured
//      against these sidecars in plan 07-09.
//   2. `worker: { format: 'es' }` makes Vite emit native ES-module workers
//      (needed by monaco-languageclient's LSP worker + DuckDB-WASM).
//   3. `manualChunks.monaco` splits the heavy Monaco bundle into its own
//      chunk so a code-only edit doesn't bust the editor cache.
//   4. `optimizeDeps.exclude: ['@duckdb/duckdb-wasm']` keeps DuckDB out of
//      the dev-server pre-bundle — it's lazy-loaded at first Run click
//      (per ADR plans + Anti-Pattern note in this plan's interfaces).
//
// Sourcemaps off: production size budget. Devs get sourcemaps in `dev`
// mode by default; this only affects `build`.
export default defineConfig({
    plugins: [
        compression({
            algorithm: 'brotliCompress',
            ext: '.br',
            threshold: 1024,
            deleteOriginFile: false,
        }),
    ],
    build: {
        target: 'es2022',
        sourcemap: false,
        rollupOptions: {
            output: {
                manualChunks: {
                    monaco: [
                        'monaco-editor',
                        'monaco-languageclient',
                        'vscode-languageserver-protocol',
                    ],
                },
            },
        },
    },
    worker: { format: 'es' },
    optimizeDeps: {
        // DuckDB-WASM lazy-loads — never pre-bundle (Anti-Pattern §07-04).
        exclude: ['@duckdb/duckdb-wasm'],
    },
});
