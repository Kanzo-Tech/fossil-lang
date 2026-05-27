/**
 * OutputPanel — the run-result viewer pane.
 *
 * Lives inside the IDE tabs layout's right panel (TabsContent value="output").
 * Delegates to `<FossilViewer/>` from `@fossil-lang/viewer` (Phase 12), which
 * provides its own Graph / Turtle / Vertices / Edges sub-tabs.
 *
 * The outer wrapper preserves the `id="graph-canvas"` + `role="img"` +
 * `aria-label` contract from Phase 8 plan 08-11 — the apps/landing/ axe-core
 * E2E gate excludes `#graph-canvas` from a11y rules (WebGL canvases have
 * no inherent semantic content; RESEARCH.md Pitfall 5). Renaming or removing
 * the id is a breaking change to the v0.1.x A11Y-01 invariant — that's why
 * we preserve it here even though FossilViewer's internal `<FossilGraphView/>`
 * already owns the canvas.
 *
 * Phase 14 plan 14-03.
 *
 * Phase 15 plan 15-04 (BUG-02): `<FossilViewer/>` is lazy-loaded via
 * `React.lazy()`. The viewer (Cosmos.gl WebGL + N3 turtle writer + worker
 * scaffolding) is ~138 KB gzipped — nearly 60% of the playground's pre-fix
 * cold-load bundle. Lazy-loading defers that cost until the user actually
 * mounts `<OutputPanel/>` (which happens AFTER they click Run for the first
 * time, since the Output tab pane only renders meaningful content once
 * vertices/edges exist). Cosmos.gl ships its own internal loading shader, so
 * the Suspense fallback only needs a minimal "Loading viewer…" placeholder.
 */

import { Suspense, lazy } from 'react';
import type { VertexRow, EdgeRow } from './FossilPlayground.js';

// 15-04 (BUG-02): defer the @fossil-lang/viewer import so the Cosmos.gl
// WebGL bundle (~138 KB gzipped) is NOT in the cold-load critical path.
// The dynamic import() yields a separate JS chunk that the browser fetches
// only when the LazyFossilViewer component first renders — i.e., when the
// user mounts OutputPanel for the first time. React.lazy returns a
// Component whose default export is the named import we want; we adapt
// the module shape inline with the conventional `.then(m => ({ default: m.X }))`
// pattern documented in the React docs.
const LazyFossilViewer = lazy(() =>
  import('@fossil-lang/viewer').then((m) => ({ default: m.FossilViewer })),
);

export interface OutputPanelProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
}

export function OutputPanel(props: OutputPanelProps): JSX.Element {
  const { vertices, edges } = props;
  // Phase 14 plan 14-03 — adapt the playground's loose `VertexRow` /
  // `EdgeRow` shapes (which only guarantee `id` / `source`+`target`) into
  // FossilViewer's stricter shape (`id` + `type` + `label`). Default the
  // missing fields the same way the legacy `ResultGraph` deprecation
  // adapter did, so the migration is byte-equivalent in behaviour.
  const adaptedVertices = vertices.map((v) => ({
    ...v,
    id: v.id,
    type: String((v as Record<string, unknown>).type ?? 'Unknown'),
    label: String((v as Record<string, unknown>).label ?? v.id),
  }));
  const adaptedEdges = edges.map((e) => ({
    ...e,
    source: e.source,
    target: e.target,
    predicate:
      typeof (e as Record<string, unknown>).predicate === 'string'
        ? ((e as Record<string, unknown>).predicate as string)
        : undefined,
  }));
  return (
    <section
      id="graph-canvas"
      role="img"
      aria-label="Result graph; tabular fallback follows below for screen readers."
      data-testid="output-panel"
    >
      {/* 15-04 (BUG-02): Suspense boundary for the lazy-loaded FossilViewer.
        * The fallback is intentionally minimal — Cosmos.gl ships its own
        * internal loading shader once the chunk resolves, and the surrounding
        * `role="img"` + aria-label already provide the SR-meaningful name for
        * this region. Using a neutral placeholder rather than null so that
        * users on slow networks see SOMETHING during the chunk fetch. */}
      <Suspense
        fallback={
          <div
            data-testid="output-panel-loading"
            style={{
              padding: '1rem',
              textAlign: 'center',
              color: 'var(--fossil-color-fg-muted, #888)',
            }}
          >
            Loading viewer…
          </div>
        }
      >
        <LazyFossilViewer
          vertices={adaptedVertices as never}
          edges={adaptedEdges as never}
        />
      </Suspense>
    </section>
  );
}
