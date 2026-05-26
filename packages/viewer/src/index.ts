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

// === 12-02 — internals/CosmosGraph.tsx export goes here ===
// === 12-03 — internals/GraphCanvas.tsx + hooks/* exports go here ===
// === 12-04 — FossilGraphView.tsx + FossilViewer.tsx + turtle/* exports go here ===
