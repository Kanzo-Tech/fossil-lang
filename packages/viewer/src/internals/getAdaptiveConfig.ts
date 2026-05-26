/**
 * getAdaptiveConfig — log10-scaled simulation parameters for cosmos.gl.
 *
 * Port of Keasy `web/src/components/discovery/graph-view-v2.tsx` L22-67
 * (Phase 12 plan 12-03). Verbatim formula — every constant is
 * production-tuned across 10 to 100,000 node graphs and divergence is a
 * defect. The continuous-lerp design avoids breakpoint snap (which
 * Cosmograph 1.x exhibited).
 *
 * Two exports:
 *   - `getAdaptiveConfig(nodeCount)` — the live function.
 *   - `DEFAULT_GRAPH_CONFIG` — a static snapshot at nodeCount=500 for
 *     callers that need a config literal (Keasy line 66 backwards-compat
 *     pattern).
 *
 * @see Keasy keasy/web/src/components/discovery/graph-view-v2.tsx L22-67
 * @see .planning/phases/12-fossil-lang-viewer-port/12-03-PLAN.md
 */

import type { GraphConfigInterface } from '@cosmos.gl/graph';

const clamp = (v: number, lo: number, hi: number): number =>
  Math.min(hi, Math.max(lo, v));

const lerp = (a: number, b: number, t: number): number =>
  a + (b - a) * clamp(t, 0, 1);

/** t = 0 at ~10 nodes, t = 1 at ~100k nodes (log10 scale). */
function graphScale(n: number): number {
  return clamp((Math.log10(Math.max(n, 1)) - 1) / 4, 0, 1);
}

/** Simulation params that scale continuously with node count. */
export function getAdaptiveConfig(nodeCount: number): GraphConfigInterface {
  const t = graphScale(nodeCount);
  return {
    // Visual (constant)
    backgroundColor: 'transparent',
    enableDrag: true,
    fitViewOnInit: false,
    pointGreyoutOpacity: 0.3,
    linkGreyoutOpacity: 0.1,
    simulationLinkDistRandomVariationRange: [1, 1.3],

    // Adaptive simulation
    spaceSize: lerp(2048, 8192, t),
    simulationRepulsion: lerp(1.2, 0.4, t),
    simulationFriction: lerp(0.7, 0.92, t),
    simulationLinkSpring: lerp(0.5, 0.25, t),
    simulationLinkDistance: lerp(30, 12, t),
    simulationGravity: lerp(0.35, 0.08, t),
    simulationDecay: lerp(800, 2500, t),
    simulationCluster: 0.15,
    simulationCenter: lerp(0.1, 0.02, t),

    // Adaptive rendering
    pointSizeScale: lerp(1.5, 0.5, t),
    renderLinks: nodeCount < 250_000,
    scalePointsOnZoom: nodeCount < 100_000,
    renderHoveredPointRing: nodeCount < 100_000,
    ...(nodeCount > 5000 && {
      linkVisibilityDistanceRange: [50, 200],
      linkVisibilityMinTransparency: 0.05,
    }),
  };
}

/** Backwards-compatible static default for callers that need a config literal. */
export const DEFAULT_GRAPH_CONFIG: GraphConfigInterface = getAdaptiveConfig(500);
