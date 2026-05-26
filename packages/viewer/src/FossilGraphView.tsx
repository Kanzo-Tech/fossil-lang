/**
 * FossilGraphView — top-level public component of `@fossil-lang/viewer`.
 *
 * Wraps `GraphCanvas` (the WebGL composition layer from plan 12-03) with
 * an automatic accessibility fallback: when `webgl={false}` is passed
 * explicitly, OR when `canvas.getContext('webgl2')` returns null at
 * mount, render the `<TabularFallback/>` semantic-tables component
 * instead.
 *
 * The LOCKED public-API prop surface per CONTEXT.md:
 *   - vertices, edges — the raw input rows (string-id keyed).
 *   - selection? — Mosaic Selection for crossfilter (opt-in).
 *   - onSelectVertex? — click-to-select callback.
 *   - webgl? — default true (modern hosts); explicit false bypasses
 *     detection and forces TabularFallback.
 *   - className? — optional override for the root div.
 *
 * Phase 12 plan 12-04 — VIEW-01 (public API surface), VIEW-04
 * (TabularFallback wire-up).
 */

import * as React from 'react';
import { useMemo, useRef } from 'react';
import type { Selection } from '@uwdata/mosaic-core';

import { GraphCanvas } from './internals/GraphCanvas.js';
import type { CosmosGraphHandle } from './internals/CosmosGraph.js';
import { TabularFallback } from './accessibility/TabularFallback.js';
import type { VertexRow, EdgeRow } from './hooks/useGraphData.js';

export interface FossilGraphViewProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
  selection?: Selection | null;
  onSelectVertex?: (v: VertexRow | null) => void;
  /**
   * Whether to attempt the WebGL/Cosmos.gl render. Default: true.
   *
   * - `false` → skip detection, always render `<TabularFallback/>`.
   * - `true` or omitted → run `detectWebGL()` at mount; render
   *   `<GraphCanvas/>` only when WebGL2 is available, otherwise fall
   *   back to TabularFallback.
   */
  webgl?: boolean;
  className?: string;
}

/** Pure-function WebGL2 detection — SSR-safe, exception-safe. */
function detectWebGL(): boolean {
  if (typeof document === 'undefined') return false;
  try {
    const canvas = document.createElement('canvas');
    return !!canvas.getContext('webgl2');
  } catch {
    return false;
  }
}

export function FossilGraphView(props: FossilGraphViewProps): JSX.Element {
  const graphRef = useRef<CosmosGraphHandle | null>(null);

  // Render-path decision: explicit-false bypasses detection; otherwise detect.
  const shouldRenderWebGL = useMemo(() => {
    if (props.webgl === false) return false;
    return detectWebGL();
  }, [props.webgl]);

  return (
    <div
      className={props.className ?? 'fossil-viewer-graph-view'}
      data-testid="fossil-viewer-graph-view"
      style={{
        position: 'relative',
        width: '100%',
        height: '100%',
        minHeight: 300,
      }}
    >
      {shouldRenderWebGL ? (
        <GraphCanvas
          vertices={props.vertices}
          edges={props.edges}
          graphRef={graphRef as React.RefObject<CosmosGraphHandle | null>}
          selection={props.selection}
          onSelectVertex={
            props.onSelectVertex as
              | ((v: { id: string; type: string; label: string } | null) => void)
              | undefined
          }
        />
      ) : (
        <TabularFallback vertices={props.vertices} edges={props.edges} />
      )}
    </div>
  );
}
