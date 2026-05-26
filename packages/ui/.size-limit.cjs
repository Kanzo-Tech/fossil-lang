/**
 * size-limit config for @fossil-lang/ui — Phase 10 plan 10-01 scaffold.
 *
 * Per CONTEXT.md bundle budget: 8 Radix primitives (~6 KB each gzipped) +
 * Fossil tokens (~2 KB) target ≤ 50 KB total. This config locks the cap
 * BEFORE primitives land so plan 10-08's bundle assertion has a baseline.
 *
 * Why these ignores:
 *  - react / react-dom: peerDependencies (host provides)
 *
 * Why NO Radix ignores: each @radix-ui/react-* package is added as a
 * normal `dependencies` entry (NOT peerDep) in plans 10-03/04/05 so the
 * primitives are bundled into the published surface. They MUST count
 * against the 50 KB cap; that is the whole point of the budget.
 *
 * @see decisions/0033-fossil-lang-ui-package.md
 * @see decisions/0034-css-variable-naming-mechanical-flatten.md
 * @see .planning/phases/10-visual-foundation-radix-ide-theme/10-CONTEXT.md (bundle budget)
 */
module.exports = [
  {
    name: '@fossil-lang/ui (no peers)',
    path: 'dist/index.js',
    limit: '50 KB',
    gzip: true,
    ignore: ['react', 'react-dom'],
  },
];
