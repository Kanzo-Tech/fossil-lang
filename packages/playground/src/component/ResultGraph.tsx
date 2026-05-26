/**
 * ResultGraph — v0.2.x deprecated alias of FossilGraphView.
 *
 * @deprecated use `@fossil-lang/viewer` `<FossilGraphView/>` directly.
 *
 * The v0.1.x prop surface is preserved byte-for-byte so consumers
 * upgrading from v0.1 to v0.2 do not need to change import sites.
 * The prop translation:
 *   - `enableWebGL` (v0.1 default: false — opt-IN) → `webgl`
 *     (FossilGraphView default: true — opt-OUT). The alias honors the
 *     v0.1 default-OFF semantic via `props.enableWebGL ?? false` so
 *     `<ResultGraph/>` with no `enableWebGL` prop still renders the
 *     accessible tabular path, just like v0.1.x shipped.
 *   - `fallback` prop is now IGNORED — `FossilGraphView` renders its
 *     own `<TabularFallback/>` accessibility path. We emit a one-time
 *     `console.warn` in dev when callers pass a non-null `fallback`,
 *     gated by a `useRef` sentinel + `useEffect` with empty deps so
 *     it fires ONCE per component mount (Phase 12 plan 12-04 W1
 *     deviation — prevents log-spam on re-renders).
 *
 * Per Phase 12 plan 12-04 + ROADMAP Phase 18 migration guide.
 */

import { useEffect, useRef, type ReactNode } from 'react';
import { FossilGraphView } from '@fossil-lang/viewer';

export interface ResultGraphProps {
  /** Vertex rows from the playground's compile+run output. */
  vertices: Array<{ id: string; [k: string]: unknown }>;
  /** Edge rows — `source` + `target` are required (ids referencing vertices). */
  edges: Array<{ source: string; target: string; [k: string]: unknown }>;
  /**
   * @deprecated v0.2.x ignores this prop — FossilGraphView ships its
   * own TabularFallback. Will be removed in v0.3.x.
   */
  fallback?: ReactNode;
  /**
   * v0.1 default: tabular fallback only, NO WebGL canvas. The alias
   * preserves this default-OFF behaviour by passing
   * `webgl={enableWebGL ?? false}` to FossilGraphView (whose own
   * default is true).
   */
  enableWebGL?: boolean;
  /** Optional class for theme hooks. */
  className?: string;
}

export function ResultGraph({
  vertices,
  edges,
  fallback,
  enableWebGL = false,
  className,
}: ResultGraphProps): JSX.Element {
  // W1 (12-04 plan-checker iteration 1): gate the deprecation warning to
  // ONCE-per-mount. A naive `if (...) console.warn(...)` in the render
  // body fires on every re-render — spammy in dev. The sentinel ref +
  // empty-deps useEffect ensures exactly one log per mount, regardless
  // of how many times the parent re-renders.
  const warnedRef = useRef(false);
  useEffect(() => {
    if (
      !warnedRef.current &&
      fallback != null &&
      typeof process !== 'undefined' &&
      process.env?.NODE_ENV !== 'production'
    ) {
      warnedRef.current = true;
      // eslint-disable-next-line no-console
      console.warn(
        '[ResultGraph] `fallback` prop ignored in v0.2.x — FossilGraphView provides its own TabularFallback. See MIGRATION-v0.2.md.',
      );
    }
    // Empty deps — we want the gating logic to evaluate once per mount,
    // not on every prop change. The ref guards against re-fires even if
    // `fallback` flips in a future re-render (the warning has already
    // been delivered for this mount).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Coerce the v0.1.x loose row shapes to FossilGraphView's stricter
  // VertexRow / EdgeRow. Type + label are derived if missing. Spread
  // the original row FIRST so explicit overrides (type/label coercions)
  // win — preserves any extra columns from the v0.1.x runtime output.
  const adaptedVertices = vertices.map((v) => ({
    ...v,
    id: v.id,
    type: String(v.type ?? 'Unknown'),
    label: String(v.label ?? v.id),
  }));
  const adaptedEdges = edges.map((e) => ({
    ...e,
    source: e.source,
    target: e.target,
    predicate:
      typeof e.predicate === 'string' ? e.predicate : undefined,
  }));

  // The outer `id="graph-canvas"` + `role="img"` + `aria-label` MUST be
  // preserved — they form the LOAD-BEARING contract with the
  // apps/landing/ axe-core gate (Phase 8 plan 08-11) which excludes
  // `#graph-canvas` from a11y rules (WebGL canvases have no inherent
  // semantic content; see RESEARCH.md Pitfall 5). Renaming or removing
  // is a breaking change to the v0.1.x A11Y-01 invariant.
  return (
    <div
      id="graph-canvas"
      role="img"
      aria-label="Result graph; tabular fallback follows below for screen readers."
      className={className ?? 'fossil-result-graph'}
    >
      <FossilGraphView
        vertices={adaptedVertices}
        edges={adaptedEdges}
        webgl={enableWebGL}
      />
    </div>
  );
}
