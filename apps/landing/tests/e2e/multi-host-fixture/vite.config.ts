/**
 * Vite config for the multi-host SC#2 proof fixture.
 *
 * Intentionally minimal — the point of this fixture is to mount
 * <FossilPlayground/> in a NON-Next.js host. The Vite config does
 * nothing exotic; if it needed to, the SC#2 proof would be undermined
 * (the claim is the component works in any React 18+ host with no
 * special bundler config).
 *
 * `worker.format: 'es'` is the canonical pattern for ESM Web Workers in
 * Vite — the lsp.worker.js file uses `import { ... }` at the top, which
 * an IIFE/Classic worker would reject.
 */
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: { port: 5175 },
  preview: {
    port: 4173,
    strictPort: true,
  },
  worker: {
    format: 'es',
  },
  // The fossil_wasm_bg.wasm artefact lives in the @fossil-lang/wasm pkg
  // directory; Vite's asset handling picks it up via the `?url` import
  // pattern in main.tsx. No copy hook needed (Vite resolves the workspace
  // symlink directly).
  optimizeDeps: {
    // The CodeMirror packages export ESM; pre-bundling them avoids a
    // request-fan-out on first page load.
    include: ['@codemirror/state', '@codemirror/view', '@codemirror/language'],
  },
});
