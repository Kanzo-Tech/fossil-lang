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
 */

import { FossilViewer } from '@fossil-lang/viewer';
import type { VertexRow, EdgeRow } from './FossilPlayground.js';

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
      <FossilViewer
        vertices={adaptedVertices as never}
        edges={adaptedEdges as never}
      />
    </section>
  );
}
