import { defineConfig } from 'vite';

// Throwaway spike Vite config — only purpose is to host the @codemirror/lsp-client
// experiment + a Web Worker that pretends to be an LSP server. NOT a pnpm workspace
// member; the spike installs its own deps in isolation (`npm install` inside this
// directory). The dev server is fixed to port 5174 so `curl http://localhost:5174`
// in CI can smoke-test that the spike at least boots.
export default defineConfig({
  server: { port: 5174 },
  worker: { format: 'es' },
});
