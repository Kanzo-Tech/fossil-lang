/**
 * EDIT-01/02/03 oracle — Worker + HTTP page. Mounts FossilEditor TWICE
 * with the WorkerTransport + HttpTransport (Playwright-mocked).
 * NullTransport lives on /editor-null.html (isolated).
 *
 * NO Next.js, NO Tailwind, NO shadcn — proves @fossil-lang/editor is
 * framework-agnostic + transport-pluggable.
 *
 * IMPORTANT: FossilEditorProps does NOT include data-testid (per 11-02
 * LOCKED prop surface). Spec selectors target the wrapping
 * <div data-testid="editor-cell-{worker,http}"> only.
 */
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import {
  FossilEditor,
  WorkerTransport,
  HttpTransport,
} from '@fossil-lang/editor';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import { buildResolverExamples } from '@fossil-lang/examples';
import { KanzoThemeProvider } from '@kanzo/theme';
// initFossilWasm must complete on the MAIN thread before any code paths
// hit `tokenize()` — @fossil-lang/codemirror-fossil's StreamParser invokes
// it eagerly at mount. The playground gates its render on a wasmReady
// state; here we await it once at module top before createRoot fires.
import { initFossilWasm } from '@fossil-lang/wasm';
// Vite's ?worker import idiom — resolves the Worker entry at build time,
// threads its dependency graph, and returns a Worker constructor. Per
// Vite docs (Web Workers section); replaces the fragile
// `new URL(specifier, import.meta.url)` pattern that does NOT resolve npm
// specifiers in Vite. The source path is the playground's worker entry,
// NOT the built dist artefact — Vite handles the source-to-output mapping.
import LspWorker from '@fossil-lang/playground/src/workers/lsp.worker?worker';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

const INITIAL_SOURCE = `# Phase 11 EDIT oracle — initial mapping
input csv from "@example/users"
output triples = {
  ?user a :User ;
        :name ?name
}
`;

// EDIT-03 requires ConnectionResolver with ≥ 2 connectors so the
// @-autocomplete test (spec test #7) can assert a populated popup.
const resolver = createDefaultResolver({
  examples: buildResolverExamples(),
});

function makeWorkerTransport(): WorkerTransport {
  const worker = new LspWorker();
  // Forward the wasmUrl via the __boot message (same protocol as useLspWorker).
  worker.postMessage({ type: '__boot', wasmUrl });
  return new WorkerTransport({ worker });
}

const workerTransport = makeWorkerTransport();
const httpTransport = new HttpTransport({
  endpoint: '/api/fossil/analyze',
  // Playwright route fulfill (editor.spec.ts) intercepts this endpoint.
});

function EditorPage() {
  const [src1, setSrc1] = useState(INITIAL_SOURCE);
  const [src2, setSrc2] = useState(INITIAL_SOURCE);
  return (
    <KanzoThemeProvider data-testid="editor-root">
      <div className="editor-grid">
        <section className="editor-cell" data-testid="editor-cell-worker">
          <h2>WorkerTransport (browser WASM LSP)</h2>
          <FossilEditor
            value={src1}
            onChange={setSrc1}
            lspTransport={workerTransport}
            resolver={resolver}
            className="fossil-editor"
          />
        </section>
        <section className="editor-cell" data-testid="editor-cell-http">
          <h2>HttpTransport (backend POST — Playwright-mocked)</h2>
          <FossilEditor
            value={src2}
            onChange={setSrc2}
            lspTransport={httpTransport}
            resolver={resolver}
            className="fossil-editor"
          />
        </section>
      </div>
    </KanzoThemeProvider>
  );
}

async function bootstrap() {
  await initFossilWasm({ wasmUrl: wasmUrl as unknown as string });
  const rootEl = document.getElementById('root');
  if (!rootEl) {
    throw new Error('Multi-host fixture: #root not found in editor.html');
  }
  createRoot(rootEl).render(<EditorPage />);
}

void bootstrap();
