/**
 * VIEW-04 oracle — viewer-fallback.html page entry. Mounts
 * <FossilGraphView/> with `webgl={false}` to FORCE the TabularFallback
 * accessibility path (per Phase 12 plan 12-04). Same dataset as
 * /viewer.html so the table content can be inspected in parallel.
 *
 * Isolated from /viewer.html so:
 *  - The "no canvas" assertion in the Playwright spec is structurally
 *    clean (no Cosmos.gl mount contaminating the DOM).
 *  - The axe-clean assertion can run without a canvas exclusion (the
 *    canvas-exclusion rule on /viewer.html is the gate-bypass for
 *    WebGL nodes which have no inherent semantic content).
 *
 * Same pattern as Phase 11 plan 11-04's /editor-null.html isolation.
 */
import { createRoot } from 'react-dom/client';
import {
  FossilGraphView,
  type VertexRow,
  type EdgeRow,
} from '@fossil-lang/viewer';
import { KanzoThemeProvider } from '@kanzo/theme';

const TYPES = ['User', 'Project', 'Comment', 'Tag', 'Repo'] as const;

const vertices: VertexRow[] = Array.from({ length: 100 }, (_, i) => ({
  id: `urn:fossil:v:${i}`,
  type: TYPES[i % TYPES.length]!,
  label: `${TYPES[i % TYPES.length]!} #${i}`,
}));

const edges: EdgeRow[] = Array.from({ length: 150 }, (_, i) => {
  const src = `urn:fossil:v:${i % 100}`;
  const tgt = `urn:fossil:v:${(i * 7 + 13) % 100}`;
  return { source: src, target: tgt, predicate: 'http://example.org/relatedTo' };
});

function FallbackPage(): JSX.Element {
  return (
    <KanzoThemeProvider>
      <div data-testid="viewer-cell-fallback" className="viewer-cell">
        <FossilGraphView
          vertices={vertices}
          edges={edges}
          webgl={false}
        />
      </div>
    </KanzoThemeProvider>
  );
}

const rootEl = document.getElementById('root');
if (!rootEl) {
  throw new Error('Multi-host fixture: #root not found in viewer-fallback.html');
}
createRoot(rootEl).render(<FallbackPage />);
