---
"@fossil-lang/viewer": minor
"@fossil-lang/playground": minor
---

Phase 12 — `@fossil-lang/viewer` Cosmos.gl port.

Viewer componente standalone publicable + IDE-style tabs composition:

- **`@fossil-lang/viewer` is a NEW workspace package** (VIEW-01) at the
  `0.1.0` initial release. Exposes `<FossilGraphView/>` (Cosmos.gl WebGL
  graph wrapper + adaptive simulation per nodeCount + Mosaic crossfilter)
  + `<FossilViewer/>` (IDE-tabs composition Graph/Turtle/Vertices/Edges).
  Port of Keasy `web/src/components/discovery/cosmos-graph.tsx` +
  `graph-view-v2.tsx` + `use-graph-data.ts` + `use-graph-crossfilter.ts`,
  byte-equivalent SALVO Tailwind-utility-class to inline-style replacement
  with `--fossil-*` CSS variables. Per Phase 12 CONTEXT.md locked decisions.

- **Adaptive simulation** (VIEW-01) — `getAdaptiveConfig(nodeCount)` is
  the same log-scaled lerp formula Keasy ships; scales repulsion/friction/
  gravity/spaceSize from 10 to 100k node graphs. Exported as a pure
  function for hosts that want to compose their own GraphCanvas-equivalent.

- **Legend + crossfilter** (VIEW-02) — `<FossilGraphView/>` accepts an
  optional `selection?: Selection` from `@uwdata/mosaic-core`. When
  present, the graph publishes selection changes to the Selection
  (`clausePoints` clause) and receives external filter changes back via
  `selectPointsByIndices` (Mosaic FilteringClient pattern). The bottom-
  left legend renders 1 Toggle per unique vertex type with count badge +
  color dot; clicking hides that group.

- **Tabs composition** (VIEW-03) — `<FossilViewer/>` wraps
  `<FossilGraphView/>` in `@fossil-lang/ui` Tabs (variant=`line` for the
  IDE look from Phase 10 plan 10-03). Tabs: Graph / Turtle / Vertices /
  Edges. The Turtle tab reuses `rowsToTurtle` (MOVED in this release
  from `@fossil-lang/playground` — single source of truth in the view
  layer; playground re-exports for v0.1.x compat).

- **Accessible tabular fallback** (VIEW-04) — when `webgl={false}` OR
  `canvas.getContext('webgl2')` returns null, FossilGraphView renders
  `<TabularFallback/>` with semantic vertices + edges tables. Playwright
  axe-clean on both the WebGL page (canvas excluded) and the fallback
  page (no exclusion needed). A11Y-01 carryover from v0.1 Phase 8.

- **Backwards compat preserved** — `@fossil-lang/playground` still
  exports `ResultGraph` (deprecated alias of FossilGraphView) +
  `rowsToTurtle` + `TurtleTab` + `FossilGraphView` + `FossilViewer`
  (re-exported from `@fossil-lang/viewer`). v0.1.x consumers do not need
  code changes. Module-instance dedup via pnpm workspace symlinks. The
  ResultGraph alias PRESERVES the load-bearing `#graph-canvas` id +
  `role="img"` + `aria-label` on its outer wrapper — the contract with
  the Phase 8 apps/landing/ axe-core gate stays intact.

- **Bundle budget** — `@fossil-lang/viewer` ships under 300 KB gzipped
  primary cap (Cosmos.gl is heavy — locked CONTEXT.md cap higher than
  ui+editor); measures 146.72 KB / 300 KB (48.9% used) with 6.3 KB
  diagnostic. `@fossil-lang/playground` measures 230.98 KB / 500 KB (the
  jump from Phase 11's ~95 KB is the re-export of @fossil-lang/viewer
  pulling Cosmos.gl + Mosaic into the bundle graph; Phase 14 composition
  refactor can re-isolate by making host code import @fossil-lang/viewer
  directly instead of via playground).

The viewer is theme-less (consumes `--fossil-*` CSS vars via host theme
provider); Kanzo-branded hosts install `@kanzo/theme` + wrap in
`<KanzoThemeProvider/>` per ADR-0035 (Phase 10 plan 10-09 visual
ownership separation).

No Rust changes — WASM 9-crate gate + walking-skeleton invariant intact
across all 5 plans (12-01..05). No ADR landed in Phase 12 — the port
reuses existing patterns (multi-host fixture from Phase 8 plan 08-11 +
Phase 10 plan 10-07 + Phase 11 plan 11-04; section-marker append from
Phase 11; sole-owner JSON-edit discipline from Phase 11). The Phase 11
ADR-0036 (Transport-superset) was the novel design for the editor; the
viewer's port is the inverse — proving the multi-host React library
family pattern scales to a second new package without architectural
divergence.

One deferred a11y polish item logged as DEF-12-05-01: the
@fossil-lang/ui Toggle active-state background contrast (3.75:1 vs
required 4.5:1) is a pre-existing Phase 10 primitive issue surfaced by
the viewer legend. Fix lives in the next @fossil-lang/ui patch
release. Documented in viewer.spec.ts axe-exclude with full rationale.
