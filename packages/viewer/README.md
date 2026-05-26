# @fossil-lang/viewer

Cosmos.gl WebGL graph viewer + IDE-style tabs composition for Fossil
result graphs — promoted from Keasy's `discovery/cosmos-graph.tsx` +
`graph-view-v2.tsx` to a shared library.

## What this is

`@fossil-lang/viewer` provides two top-level React components:

- **`<FossilGraphView/>`** — the lowest-level wrapper around
  [`@cosmos.gl/graph`](https://github.com/cosmos-gl/graph)'s `Graph`
  class. Owns canvas mount + destroy lifecycle, adaptive simulation
  config (scales with node count), legend with per-type toggle, optional
  Mosaic crossfilter selection, and a tabular accessibility fallback
  for hosts without WebGL.
- **`<FossilViewer/>`** — IDE-style tabs composition layering Turtle
  serialization (via `n3.Writer`), Vertices table, and Edges table on
  top of `<FossilGraphView/>`. Uses `@fossil-lang/ui` Tabs primitives.

## Quick start

```tsx
import { FossilGraphView } from '@fossil-lang/viewer';

export function MyApp() {
  return (
    <FossilGraphView
      vertices={[
        { id: 'v1', type: 'Person', label: 'Alice' },
        { id: 'v2', type: 'Person', label: 'Bob' },
      ]}
      edges={[{ source: 'v1', target: 'v2', predicate: 'knows' }]}
    />
  );
}
```

For the full tabs experience (Graph / Turtle / Vertices / Edges):

```tsx
import { FossilViewer } from '@fossil-lang/viewer';

<FossilViewer vertices={vertices} edges={edges} />;
```

## Phase 12

This package is the deliverable of Phase 12 of the v0.2 milestone — see
[`.planning/phases/12-fossil-lang-viewer-port/12-CONTEXT.md`](../../.planning/phases/12-fossil-lang-viewer-port/12-CONTEXT.md)
for the locked decisions trail. The port is byte-equivalent to Keasy's
production viewer salvo CSS-utility-class replacement (Tailwind classes
become inline-style objects + `--fossil-*` CSS variables with literal
fallbacks).

## Bundle budget

Primary cap: **300 KB gzipped** (locked per CONTEXT.md — Cosmos.gl's
WebGL machinery dominates). Diagnostic cap (our-own-code, excluding
Cosmos.gl + Mosaic + n3): 30 KB gzipped. Enforced via `size-limit` in
CI.

## License

Apache-2.0
