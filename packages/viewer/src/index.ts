/**
 * @fossil-lang/viewer — public entry. Cosmos.gl WebGL graph viewer +
 * IDE-style tabs composition (Graph / Turtle / Vertices / Edges) with
 * Mosaic crossfilter + accessible tabular fallback.
 *
 * Phase 12 of v0.2 — promoted from Keasy `discovery/cosmos-graph.tsx` +
 * `graph-view-v2.tsx` byte-equivalent salvo CSS-var-replace. See
 * .planning/phases/12-fossil-lang-viewer-port/12-CONTEXT.md for locked
 * decisions.
 *
 * Section-marker append contract: plans 12-02..04 ship in disjoint
 * waves. Each appends its exports under a // === 12-NN === marker so
 * parallel-wave executors don't conflict on this file. Plan 12-01
 * owns the header + the version stamp.
 */

export const VIEWER_PACKAGE_VERSION = '0.1.0';

// === 12-02 — internals/CosmosGraph.tsx exports ===
export { CosmosGraph } from './internals/CosmosGraph.js';
export type { CosmosGraphProps, CosmosGraphHandle } from './internals/CosmosGraph.js';
// === 12-03 — internals/getAdaptiveConfig + GraphCanvas + hooks/useGraphData + hooks/useGraphCrossfilter ===
export { getAdaptiveConfig, DEFAULT_GRAPH_CONFIG } from './internals/getAdaptiveConfig.js';
export { GraphCanvas } from './internals/GraphCanvas.js';
export type { GraphCanvasProps } from './internals/GraphCanvas.js';
export { useGraphData, GROUP_CSS_COLORS, hashPos } from './hooks/useGraphData.js';
export type { VertexRow, EdgeRow, KGGraphData } from './hooks/useGraphData.js';
export { useGraphCrossfilter } from './hooks/useGraphCrossfilter.js';
// === 12-04 — accessibility/TabularFallback ===
export { TabularFallback } from './accessibility/TabularFallback.js';
export type { TabularFallbackProps } from './accessibility/TabularFallback.js';

// === 12-04 — turtle (moved from @fossil-lang/playground) ===
export { rowsToTurtle, TurtleTab } from './turtle/index.js';
export type { TurtleVertexRow, TurtleEdgeRow, TurtleTabProps } from './turtle/index.js';

// === 12-04 — FossilGraphView + FossilViewer (public API) ===
// Filled in by Task 2 of plan 12-04.
