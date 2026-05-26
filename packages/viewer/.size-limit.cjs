/**
 * size-limit config for @fossil-lang/viewer — Phase 12.
 *
 * Per CONTEXT.md bundle budget: 300 KB gzipped primary cap. Cosmos.gl's
 * WebGL machinery dominates the bundle — this cap is intentional and
 * higher than other @fossil-lang/* packages (editor 100 KB; ui 30 KB).
 *
 * Diagnostic entry (Cosmos.gl + Mosaic + n3 excluded) tracks our-own-code
 * growth — FossilGraphView + FossilViewer + hooks + adapters + turtle
 * serializer. Useful for monitoring our viewer surface in isolation from
 * the heavy vendor deps.
 *
 * Why these ignores:
 *  - react / react-dom: peerDependencies (host provides)
 *  - @cosmos.gl/graph: the dominant vendor dep (WebGL + WASM machinery)
 *  - @uwdata/mosaic-core: Mosaic Selection / crossfilter
 *  - n3: Turtle serializer (rowsToTurtle moved here in plan 12-04)
 *  - @fossil-lang/ui + @fossil-lang/types: shared deps across the family
 *
 * @see .planning/phases/12-fossil-lang-viewer-port/12-CONTEXT.md
 */
module.exports = [
  {
    name: '@fossil-lang/viewer (everything bundled — primary cap)',
    path: 'dist/index.js',
    limit: '300 KB',
    gzip: true,
    ignore: ['react', 'react-dom'],
  },
  {
    name: '@fossil-lang/viewer core (Cosmos.gl + Mosaic + n3 excluded — diagnostic)',
    path: 'dist/index.js',
    limit: '30 KB',
    gzip: true,
    ignore: [
      'react',
      'react-dom',
      '@cosmos.gl/graph',
      '@uwdata/mosaic-core',
      '@uwdata/mosaic-sql',
      'n3',
      '@fossil-lang/ui',
      '@fossil-lang/types',
    ],
  },
];
