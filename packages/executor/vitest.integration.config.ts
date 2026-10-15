import { defineConfig } from 'vitest/config';

// `integration/` and nothing else, behind `pnpm test:integration`.
//
// `vitest.config.ts` beside this is `tests/`, and it is what `pnpm test` runs, including in
// `release.yml`'s gate. These suites run the real executor over generated rows and read the graph
// back through DuckDB-WASM and `@kanzo-tech/graphar-duckdb`, so they have a config of their own
// rather than a glob in that one. `vitest.config.base.ts` is not extended because its `happy-dom`
// environment is the wrong default for every file here.
export default defineConfig({
  test: {
    environment: 'node',
    globals: true,
    reporters: ['default'],
    include: ['integration/**/*.test.ts'],
  },
});
