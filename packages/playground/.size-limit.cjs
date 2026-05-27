/**
 * size-limit config for @fossil-lang/playground — PKG-03 JS half.
 *
 * Per CONTEXT.md non-functional requirements + SC#4 + 08-VERIFICATION.md
 * gap 2: the playground's CORE gzipped bundle (excluding peer-provided
 * React + CodeMirror + WASM + DuckDB-WASM stacks AND the now-lazy-loaded
 * @fossil-lang/viewer) must stay under the cold-load target. This file
 * is read by `size-limit` (root devDep, also declared per-package below)
 * when `pnpm --filter @fossil-lang/playground size` is invoked.
 *
 * Why these ignores:
 *  - react / react-dom: peerDependencies (host provides)
 *  - @codemirror/* + @lezer/highlight: peerDependencies (host provides)
 *  - @fossil-lang/wasm + @fossil-lang/codemirror-fossil: peerDependencies
 *  - @duckdb/duckdb-wasm + apache-arrow: peerDependenciesMeta.optional
 *    (host opts in for Run; transitive Arrow comes with DuckDB)
 *  - @fossil-lang/{types, resolvers, examples}: workspace deps. Resolvers
 *    + examples ARE part of the published surface but consumers typically
 *    bring their own resolver impl + example bundle, so we measure the
 *    CORE playground only. Types is zero-runtime.
 *  - @fossil-lang/viewer: 15-04 (BUG-02). Lazy-loaded via React.lazy()
 *    inside OutputPanel.tsx + ResultGraph.tsx, so it ships as a SEPARATE
 *    JS chunk the browser only fetches when the user actually mounts the
 *    output panel for the first time. Treat as `external` for the cold-
 *    load budget, on par with the peer-optional DuckDB-WASM (also
 *    deferred until first Run). The static re-exports of FossilViewer /
 *    FossilGraphView / rowsToTurtle / TurtleTab from src/index.tsx are
 *    kept for v0.2 API compatibility; real-world consumer bundlers
 *    (Vite/Rollup/webpack with sideEffects:false) tree-shake them away
 *    when the consumer doesn't reference them. size-limit's esbuild
 *    aggregates all output chunks regardless, so we measure cold-load
 *    by externalising the viewer entirely.
 *
 * Limit rationale (15-04 plan; supersedes the Phase 8 200 KB target):
 *  - The Phase 8 200 KB target was set BEFORE Phase 12 added the
 *    Cosmos.gl WebGL viewer (~138 KB gzipped of the playground's 230 KB
 *    Phase-14 measurement). The original target assumed the viewer
 *    didn't exist; the honest fix is to defer the viewer (which is
 *    deferrable — it has no meaningful work to do until a user clicks
 *    Run for the first time) and measure cold-load WITHOUT it.
 *  - Cold-load measurement with @fossil-lang/viewer externalised:
 *    ~92.3 KB (Phase 15 plan 15-04 Task 1 probe). The 120 KB limit
 *    leaves ~27 KB headroom for normal Phase 16/17 growth without
 *    needing another budget renegotiation.
 *  - The PKG-03 hard cap of 500 KB remains the absolute ceiling for the
 *    aggregate bundle (entry + every dynamic chunk). The current
 *    aggregate is ~237 KB — well under hard cap.
 *
 * @see decisions/0028-playground-as-react-library.md
 * @see decisions/0031-pnpm-monorepo-restructure.md (PKG-03)
 * @see .planning/phases/08-playground-react-library-v0-1/08-VERIFICATION.md (gap 2)
 * @see .planning/phases/15-bug-sweep-visual-baselines/15-04-SUMMARY.md (option D)
 */

const PEER_IGNORE = [
  // React peers
  'react',
  'react-dom',
  // CodeMirror peers
  '@codemirror/state',
  '@codemirror/view',
  '@codemirror/language',
  '@codemirror/autocomplete',
  '@codemirror/lint',
  '@codemirror/lsp-client',
  '@lezer/highlight',
  // WASM + LSP-client peers
  '@fossil-lang/wasm',
  '@fossil-lang/codemirror-fossil',
  '@fossil-lang/types',
  // Workspace data deps consumers typically bring themselves
  '@fossil-lang/resolvers',
  '@fossil-lang/examples',
  // DuckDB-WASM is peer-optional; Arrow comes transitively with it
  '@duckdb/duckdb-wasm',
  'apache-arrow',
  // 15-04 (BUG-02): viewer is lazy-loaded — measure cold-load without it
  '@fossil-lang/viewer',
];

module.exports = [
  {
    name: '@fossil-lang/playground cold-load (no peers, no lazy viewer)',
    path: 'dist/index.js',
    limit: '120 KB',
    gzip: true,
    ignore: PEER_IGNORE,
  },
];
