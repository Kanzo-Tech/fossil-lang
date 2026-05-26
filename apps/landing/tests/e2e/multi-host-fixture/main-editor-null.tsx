/**
 * EDIT-02 mode #3 — isolated NullTransport oracle. Mounts ONE
 * FossilEditor instance with NullTransport. NO other transports on this
 * page, so the spec's non-firing assertion (route counter === 0) is
 * structurally definitive.
 */
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { FossilEditor, NullTransport } from '@fossil-lang/editor';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import { buildResolverExamples } from '@fossil-lang/examples';
import { KanzoThemeProvider } from '@kanzo/theme';
// initFossilWasm must complete on the MAIN thread before any code paths
// hit `tokenize()` — @fossil-lang/codemirror-fossil's StreamParser invokes
// it eagerly at mount.
import { initFossilWasm } from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

const INITIAL_SOURCE = `# Phase 11 EDIT oracle — initial mapping (null)
input csv from "@example/users"
output triples = {
  ?user a :User ;
        :name ?name
}
`;

const resolver = createDefaultResolver({
  examples: buildResolverExamples(),
});
const nullTransport = new NullTransport();

function EditorNullPage() {
  const [src, setSrc] = useState(INITIAL_SOURCE);
  return (
    <KanzoThemeProvider data-testid="editor-root">
      <section className="editor-cell" data-testid="editor-cell-null">
        <h2>NullTransport (read-only static — isolated)</h2>
        <FossilEditor
          value={src}
          onChange={setSrc}
          lspTransport={nullTransport}
          resolver={resolver}
          className="fossil-editor"
        />
      </section>
    </KanzoThemeProvider>
  );
}

async function bootstrap() {
  await initFossilWasm({ wasmUrl: wasmUrl as unknown as string });
  const rootEl = document.getElementById('root');
  if (!rootEl) {
    throw new Error('Multi-host fixture: #root not found in editor-null.html');
  }
  createRoot(rootEl).render(<EditorNullPage />);
}

void bootstrap();
