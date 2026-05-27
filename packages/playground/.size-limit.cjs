/**
 * size-limit config for @fossil-lang/playground — PKG-03 JS half.
 *
 * Per CONTEXT.md non-functional requirements + SC#4 + 08-VERIFICATION.md
 * gap 2: the playground's CORE gzipped bundle (excluding peer-provided
 * React + CodeMirror + WASM + DuckDB-WASM stacks) must stay under
 * 500 KB. This file is read by `size-limit` (root devDep, also declared
 * per-package below) when `pnpm --filter @fossil-lang/playground size`
 * is invoked.
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
 *
 * Phase 15 plan 15-04 NOTE: the probe entries (lines 73-150 ish) are
 * SCAFFOLDING for the bundle-budget revisit checkpoint. They measure
 * each candidate fix's potential savings in isolation by adding the
 * candidate to the `ignore` list and observing the delta vs the baseline
 * entry. Probe entries are REMOVED after the checkpoint decision in
 * Task 3 (the production config keeps only the baseline `core` entry
 * with whichever `limit:` value the user selects).
 *
 * @see decisions/0028-playground-as-react-library.md
 * @see decisions/0031-pnpm-monorepo-restructure.md (PKG-03)
 * @see .planning/phases/08-playground-react-library-v0-1/08-VERIFICATION.md (gap 2)
 * @see .planning/phases/15-bug-sweep-visual-baselines/15-04-PLAN.md (probes)
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
];

module.exports = [
  {
    name: '@fossil-lang/playground core (no peers, no workspace data)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: PEER_IGNORE,
  },

  // ============================================================
  // PHASE 15 PLAN 15-04 PROBE ENTRIES — scaffolding for decision.
  // Each entry adds ONE candidate to PEER_IGNORE; delta vs baseline
  // = upper bound on savings if that candidate were removed (e.g.,
  // via lazy-load or alt-library swap).
  // REMOVE these entries after checkpoint decision is applied.
  // ============================================================

  {
    name: 'PROBE: baseline minus @codemirror/lang-sql (lazy-load CompiledSqlPanel)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, '@codemirror/lang-sql'],
  },

  {
    name: 'PROBE: baseline minus react-resizable-panels (alt-library swap)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, 'react-resizable-panels'],
  },

  {
    name: 'PROBE: baseline minus @fossil-lang/ui Tabs+Tooltip (tree-shake)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, '@fossil-lang/ui'],
  },

  {
    name: 'PROBE: baseline minus n3 (RDF parser — turtle preview)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, 'n3'],
  },

  {
    name: 'PROBE: baseline minus fflate (permalink compression)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, 'fflate'],
  },

  {
    name: 'PROBE: baseline minus @fossil-lang/viewer (Cosmos.gl graph)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, '@fossil-lang/viewer'],
  },

  {
    name: 'PROBE: baseline minus @fossil-lang/editor',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [...PEER_IGNORE, '@fossil-lang/editor'],
  },
];
