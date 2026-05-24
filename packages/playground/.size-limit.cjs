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
 * @see decisions/0028-playground-as-react-library.md
 * @see decisions/0031-pnpm-monorepo-restructure.md (PKG-03)
 * @see .planning/phases/08-playground-react-library-v0-1/08-VERIFICATION.md (gap 2)
 */
module.exports = [
  {
    name: '@fossil-lang/playground core (no peers, no workspace data)',
    path: 'dist/index.js',
    limit: '500 KB',
    gzip: true,
    ignore: [
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
    ],
  },
];
