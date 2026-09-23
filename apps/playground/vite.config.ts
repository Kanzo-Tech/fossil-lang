import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const repoRoot = fileURLToPath(new URL('../..', import.meta.url));

// The playground is a static bundle. There is no dev server API, no proxy and no
// backend — that is the architectural claim it exists to make clickable, so a
// server entry appearing in this file is the thing to argue about, not to add.
export default defineConfig({
  plugins: [react()],
  server: {
    // `src/example.ts` imports `examples/hello.fossil` and its two siblings with
    // `?raw`, from OUTSIDE this app. That is deliberate — the browser must run the
    // same bytes the walking-skeleton test runs, and a copy under `src/` would be a
    // second source that drifts. Vite's dev server refuses to serve outside its root
    // unless the root is allowed, so it is allowed.
    fs: { allow: [repoRoot] },
  },
  optimizeDeps: {
    // duckdb-wasm ships its workers as separate entry files that must not be
    // pre-bundled into the dep optimizer's single chunk — `?url` imports of them
    // resolve to the real files only if esbuild leaves them alone.
    exclude: ['@duckdb/duckdb-wasm'],
  },
  worker: { format: 'es' },
  build: {
    // The three wasm binaries are large and are fetched, not inlined. Vite's default
    // 4 kB inline threshold already excludes them; this only silences the chunk-size
    // warning that a wasm-heavy app trips on every build.
    chunkSizeWarningLimit: 4096,
  },
});
