/**
 * size-limit config for @fossil-lang/editor — Phase 11.
 *
 * Per CONTEXT.md bundle budget: 100 KB gzipped primary cap (includes CM6
 * wiring; less than playground; more than ui).
 *
 * Diagnostic entry (CM6 + LSP-client excluded) tracks our-own-code growth
 * separately — useful for Phase 14 composition refactor monitoring.
 *
 * Why these ignores:
 *  - react / react-dom: peerDependencies (host provides)
 *  - @codemirror/*: peerDependencies (host/playground provides; bundle
 *    must NOT count these against the primary entry's 100 KB cap)
 *  - @fossil-lang/codemirror-fossil + @fossil-lang/wasm: peerDeps
 *  - @fossil-lang/ui: shared dep across the family
 *
 * @see decisions/0036-transport-superset.md
 * @see .planning/phases/11-fossil-lang-editor-extraction/11-CONTEXT.md
 */
module.exports = [
  {
    name: '@fossil-lang/editor (everything bundled — primary cap)',
    path: 'dist/index.js',
    limit: '100 KB',
    gzip: true,
    ignore: ['react', 'react-dom'],
  },
  {
    name: '@fossil-lang/editor core (CM6 + LSP-client excluded — diagnostic)',
    path: 'dist/index.js',
    limit: '20 KB',
    gzip: true,
    ignore: [
      'react',
      'react-dom',
      '@codemirror/state',
      '@codemirror/view',
      '@codemirror/language',
      '@codemirror/autocomplete',
      '@codemirror/lint',
      '@codemirror/lsp-client',
      '@lezer/highlight',
      '@fossil-lang/codemirror-fossil',
      '@fossil-lang/wasm',
      '@fossil-lang/ui',
    ],
  },
];
