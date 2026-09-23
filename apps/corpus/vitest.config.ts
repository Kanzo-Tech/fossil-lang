import { defineConfig } from 'vitest/config';

// `integration/` and nothing else.
//
// This app's `pnpm test` is `guards/self-test.mjs` and the conformance chain — plain `node`, no
// runner, no `node_modules`. This config is the OTHER half: the suites that write a corpus with
// the `duckdb` binary and read it back through `packages/corpus/src`, which is why they are here
// and not next door. `vitest.config.base.ts` is not extended because its `happy-dom` environment
// is the wrong default for every file under this directory — each of them opens a file off disk.
export default defineConfig({
  test: {
    environment: 'node',
    globals: true,
    reporters: ['default'],
    include: ['integration/**/*.test.ts'],
  },
});
