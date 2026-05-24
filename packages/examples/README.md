# @fossil-lang/examples

Bundled fixture examples for [`@fossil-lang/playground`](../playground/). Shipped as a
publishable npm package so React hosts (and the OSS `apps/landing/`) get a working
default example out of the box without separate `fetch()`s.

## v0.1 contents

One example — the canonical walking-skeleton mirror of the cargo CLI's
`examples/hello.fossil`:

```
src/hello/
  hello.fossil      ← Fossil mapping (uses @examples/ paths per CONN-03)
  hello.csv         ← input data
  hello.csvw.json   ← CSVW JSON-LD descriptor
  hello.shex        ← target ShEx shape
```

Phase 9 adds the curated PLAY-05 examples gallery (6–10 examples with variations);
v0.1 ships only `hello` so the React component renders an end-to-end working
playground on first mount.

## Consumer pattern

```typescript
import { examples, buildResolverExamples, helloExample } from '@fossil-lang/examples';
import { createDefaultResolver } from '@fossil-lang/resolvers';

// `examples` is statically populated at import time via Vite `?raw` imports.
// Tree-shakeable: importing only `helloExample` skips the array machinery.
console.log(examples.length, examples[0]?.id); // 1, 'hello'

// Pass the resolver-ready map to createDefaultResolver:
const resolver = createDefaultResolver({
  examples: buildResolverExamples(),
});

// Use helloExample.mapping as the initial editor content:
const initialMapping = helloExample.mapping;
```

## Why TS module + `?raw` imports?

RESEARCH.md Open Question 3 ("examples as data files vs TS module strings")
resolved to **TS module strings via Vite `?raw`**:

- **Tree-shakeable** — consumers that only need `helloExample.mapping` (e.g. the
  landing page's initial editor value) skip everything else.
- **Zero offline complexity** — bundled into the consumer's main JS chunk, no
  separate `fetch()` needed, no Service Worker entry for fixture files.
- **Bundle cost** — ~10 KB per example; ~60 KB for Phase 9's eventual 6–10
  examples (acceptable; preserves OFFLINE-01 invariant).

## Mapping-source pinning (smoke test)

The package's `src/hello/hello.fossil` is a copy of the cargo CLI's
`examples/hello.fossil` with ONE rewrite: `io.csv("examples/users.csv")` →
`io.csv("@examples/hello.csv")` (resolver-driven path per CONN-03 / ADR-0029).

A Vitest smoke test in `tests/examples.test.ts` asserts the prefix declarations
match byte-for-byte between the two copies — catches grammar drift when the
cargo-side example evolves.

## Package layout (post-build)

```
packages/examples/
├── package.json
├── tsconfig.json
├── vitest.config.ts
├── scripts/
│   └── copy-fixtures.mjs       ← postbuild: mirrors src/hello/*.{fossil,csv,csvw.json,shex} to dist/hello/
├── src/
│   ├── index.ts                 ← examples + buildResolverExamples + Example type
│   ├── types.d.ts               ← ambient `*?raw` module declaration
│   └── hello/
│       ├── index.ts
│       ├── hello.fossil
│       ├── hello.csv
│       ├── hello.csvw.json
│       └── hello.shex
├── tests/
│   └── examples.test.ts
└── dist/                        ← published
    ├── index.{js,d.ts}
    └── hello/
        ├── index.{js,d.ts}
        └── hello.{fossil,csv,csvw.json,shex}   ← copied by copy-fixtures.mjs
```

## License

Apache-2.0 — same as the rest of the `@fossil-lang/*` family.
