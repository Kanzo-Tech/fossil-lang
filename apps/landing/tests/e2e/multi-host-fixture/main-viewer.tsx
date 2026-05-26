/**
 * VIEW-01..03 oracle — viewer.html page entry. Mounts <FossilViewer/>
 * with a deterministic 100-vertex + 150-edge sample dataset + a Mosaic
 * `Selection.crossfilter()` so the legend toggle, tab switches, and
 * crossfilter publish/clear paths are all exercised in a real browser.
 *
 * NO Next.js, NO Tailwind, NO shadcn — proves @fossil-lang/viewer is
 * framework-agnostic + theme-less. Kanzo branding via @kanzo/theme
 * Provider, mirroring main-editor.tsx pattern from Phase 11 plan 11-04.
 *
 * The 5-type / 100-vertex split (User × 20 + Project × 20 + Comment ×
 * 20 + Tag × 20 + Repo × 20) gives the legend exactly 5 Toggle buttons
 * — enough for the spec's ≥2 assertion + each toggle has a count badge
 * of "20".
 *
 * data-testid="viewer-cell" lives on the WRAPPING div because the
 * FossilViewer prop surface is LOCKED + minimal (no data-testid prop).
 */
import { createRoot } from 'react-dom/client';
import { FossilViewer, type VertexRow, type EdgeRow } from '@fossil-lang/viewer';
import { Selection } from '@uwdata/mosaic-core';
import { KanzoThemeProvider } from '@kanzo/theme';

const TYPES = ['User', 'Project', 'Comment', 'Tag', 'Repo'] as const;

const vertices: VertexRow[] = Array.from({ length: 100 }, (_, i) => ({
  id: `urn:fossil:v:${i}`,
  type: TYPES[i % TYPES.length]!,
  label: `${TYPES[i % TYPES.length]!} #${i}`,
  createdAt: new Date(2026, 0, 1 + (i % 28)).toISOString(),
}));

const edges: EdgeRow[] = Array.from({ length: 150 }, (_, i) => {
  const src = `urn:fossil:v:${i % 100}`;
  const tgt = `urn:fossil:v:${(i * 7 + 13) % 100}`;
  return { source: src, target: tgt, predicate: 'http://example.org/relatedTo' };
});

// Mosaic Selection — crossfilter strategy. The viewer's
// useGraphCrossfilter hook attaches as a MosaicClient; clicking a
// vertex publishes via clausePoints; external filter updates (none in
// this fixture) would re-highlight.
const selection = Selection.crossfilter();

function ViewerPage(): JSX.Element {
  return (
    <KanzoThemeProvider>
      <div data-testid="viewer-cell" className="viewer-cell">
        <FossilViewer
          vertices={vertices}
          edges={edges}
          selection={selection}
          webgl={true}
        />
      </div>
    </KanzoThemeProvider>
  );
}

const rootEl = document.getElementById('root');
if (!rootEl) {
  throw new Error('Multi-host fixture: #root not found in viewer.html');
}
createRoot(rootEl).render(<ViewerPage />);
