/**
 * size-limit config for @kanzo/theme.
 *
 * Per plan 10-09 (ownership inversion): the kanzo brand surface is intentionally
 * tiny — a single FossilTheme object (kanzoTheme), a ~30-line React Context
 * Provider, and a vendored mechanical-flatten helper (~25 lines per ADR-0034).
 * 20 KB gzipped is a comfortable cap; the realistic footprint is <5 KB.
 *
 * Why these ignores:
 *  - react / react-dom: peerDependencies (host provides)
 *
 * No Radix or DOM-heavy code here — this package is text-token + a thin
 * provider, nothing else.
 *
 * @see decisions/0035-visual-ownership-separation.md
 * @see decisions/0034-css-variable-naming-mechanical-flatten.md
 */
module.exports = [
  {
    name: '@kanzo/theme (no peers)',
    path: 'dist/index.js',
    limit: '20 KB',
    gzip: true,
    ignore: ['react', 'react-dom'],
  },
];
