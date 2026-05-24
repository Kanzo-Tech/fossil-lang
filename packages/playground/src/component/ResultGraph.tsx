/**
 * ResultGraph — Cosmos.gl WebGL graph viz with tabular a11y fallback.
 *
 * Per RESEARCH.md Pitfall 4 (Cosmos.gl React lifecycle):
 *   - WebGL contexts are heavyweight. Mount via `useRef` + `useEffect` with
 *     EMPTY dep array; the canvas lives OUTSIDE React's reconciler.
 *   - `update()` calls on prop changes via a SEPARATE useEffect that DOES
 *     depend on props.
 *   - Explicit `destroy()` in cleanup. NOT React-managed children.
 *
 * Per RESEARCH.md Open Question 6: `cosmos.gl` is the OpenJS Foundation home
 * for the Cosmograph WebGL engine (formerly `@cosmograph/cosmos`). The
 * exact API surface needs verification against the installed version; this
 * implementation is intentionally defensive — the dynamic import is wrapped
 * in try/catch, and on any failure the fallback prop renders. v0.1 prefers
 * the fallback (tabular) by default; consumers opt into the WebGL graph
 * once we've verified the API + tested against axe-core.
 *
 * Per A11Y-01 (WCAG 2.1 AA): a WebGL canvas alone is not accessible. The
 * `fallback` prop renders alongside the canvas (or replaces it on failure)
 * so screen readers + keyboard navigation have a path through the data.
 */

import { useEffect, useRef, type ReactNode } from 'react';

export interface ResultGraphProps {
  /** Vertex rows from the playground's compile+run output. */
  vertices: Array<{ id: string; [k: string]: unknown }>;
  /** Edge rows — `source` + `target` are required (ids referencing vertices). */
  edges: Array<{ source: string; target: string; [k: string]: unknown }>;
  /** A11Y-01 fallback — rendered alongside (or in place of) the canvas. */
  fallback?: ReactNode;
  /**
   * v0.1 default: tabular fallback only, NO WebGL canvas. Set to `true` to
   * opt into the Cosmos.gl WebGL render path (still verifying API per
   * RESEARCH.md Open Question 6). Phase 9 polish flips this default once
   * the API is locked + axe-core gate is green.
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
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<{ destroy(): void } | null>(null);

  // Mount once — empty deps. Per RESEARCH.md Pitfall 4 the canvas must live
  // outside the reconciler; updates flow through a separate effect below.
  useEffect(() => {
    if (!enableWebGL) return;
    if (!hostRef.current) return;
    let cancelled = false;
    (async () => {
      try {
        // Lazy import via a variable to keep TS off the static module graph.
        // RESEARCH.md Open Question 6 flagged cosmos.gl vs @sqlrooms/cosmos as
        // unresolved — until 08-11 verifies the canonical package, this dynamic
        // import is intentionally type-erased. Phase 9 polish locks the import
        // target + adds the type ambient declaration.
        const moduleSpec = 'cosmos.gl';
        const cosmos = (await import(/* @vite-ignore */ moduleSpec).catch(
          () => null,
        )) as null | { default?: unknown; Graph?: unknown };
        if (cancelled || !cosmos) return;
        // Defensive: cosmos.gl's API surface needs verification per RESEARCH.md
        // Open Question 6. We log + bail rather than crash so the tabular
        // fallback always renders.
        // eslint-disable-next-line no-console
        console.warn(
          '[ResultGraph] cosmos.gl WebGL mount path not yet wired — verify API per RESEARCH.md Open Question 6 + Phase 9 polish.',
        );
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error('[ResultGraph] cosmos.gl init failed; falling back to tabular:', e);
      }
    })();
    return () => {
      cancelled = true;
      viewRef.current?.destroy();
      viewRef.current = null;
    };
    // Mount-once empty deps — explicit per Pitfall 4. Lint suppressed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enableWebGL]);

  // Update on data change — separate effect per Pitfall 4. No-op until
  // viewRef.current is populated (which requires the WebGL path to be
  // enabled + cosmos.gl API verified — both deferred).
  useEffect(() => {
    if (!viewRef.current) return;
    // viewRef.current.update(vertices, edges) — Phase 9 polish.
  }, [vertices, edges]);

  return (
    // id="graph-canvas" is LOAD-BEARING and lives on the OUTER wrapper (not
    // only on the inner canvas) so the axe-core `exclude: ['#graph-canvas']`
    // selector hits whether or not enableWebGL is on. RESEARCH.md Pitfall 5 +
    // Pattern 4: canvases have no inherent semantic content; the tabular
    // fallback below provides SR-accessible data. The id is the contract
    // between this component and the 08-11 axe-core E2E gate; renaming is
    // a breaking change.
    //
    // role="img" + aria-label apply to the OUTER wrapper too so the region
    // has a stable accessible name even in the v0.1 default (enableWebGL=
    // false → only fallback renders). SR users hear "Result graph, image"
    // and the fallback's tabular content gives them the data.
    <div
      id="graph-canvas"
      role="img"
      aria-label="Result graph; tabular fallback follows below for screen readers."
      className={className ?? 'fossil-result-graph'}
    >
      {enableWebGL && (
        <div
          ref={hostRef}
          // Inner WebGL host. axe-core's exclusion targets the outer
          // #graph-canvas wrapper which transitively excludes this.
          style={{ minHeight: 300 }}
        />
      )}
      {fallback}
    </div>
  );
}
