/**
 * size-limit config for @fossil-lang/ui — Phase 10.
 *
 * Per CONTEXT.md bundle budget: 8 Radix primitives (~6 KB each gzipped) +
 * Fossil tokens (~2 KB) target ≤ 50 KB total. This config locks the cap.
 *
 * Why these ignores:
 *  - react / react-dom: peerDependencies (host provides)
 *
 * Why NO Radix ignores on the primary entry: each @radix-ui/react-*
 * package is added as a normal `dependencies` entry (NOT peerDep) in
 * plans 10-03/04/05 so the primitives are bundled into the published
 * surface. They MUST count against the 50 KB cap; that is the whole
 * point of the budget.
 *
 * Diagnostic entry (Radix excluded): added in plan 10-08 to track
 * our-own-code growth separately now that the primary entry is at 49.29
 * kB / 50 kB cap (0.71 kB headroom). Useful for Phase 14 + future
 * primitive additions — if our wrappers grow but Radix bumps account
 * for the delta, the diagnostic entry stays stable; if our wrappers
 * grow, the diagnostic entry catches it before the primary cap.
 *
 * @see decisions/0033-fossil-lang-ui-package.md
 * @see decisions/0034-css-variable-naming-mechanical-flatten.md
 * @see decisions/0035-visual-ownership-separation.md
 * @see .planning/phases/10-visual-foundation-radix-ide-theme/10-CONTEXT.md (bundle budget)
 */
module.exports = [
  {
    name: '@fossil-lang/ui (everything bundled — primary cap)',
    path: 'dist/index.js',
    limit: '50 KB',
    gzip: true,
    ignore: ['react', 'react-dom'],
  },
  {
    name: '@fossil-lang/ui core (Radix + react-resizable-panels excluded — diagnostic)',
    path: 'dist/index.js',
    limit: '15 KB',
    gzip: true,
    ignore: [
      'react',
      'react-dom',
      '@radix-ui/react-tabs',
      '@radix-ui/react-dialog',
      '@radix-ui/react-dropdown-menu',
      '@radix-ui/react-tooltip',
      '@radix-ui/react-scroll-area',
      '@radix-ui/react-toggle',
      '@radix-ui/react-toggle-group',
      '@radix-ui/react-separator',
      'react-resizable-panels',
    ],
  },
];
