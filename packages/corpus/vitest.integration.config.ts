import { defineConfig } from 'vitest/config';

// `integration/` and nothing else, behind `pnpm test:integration`.
//
// `vitest.config.ts` beside this is `tests/`, and it is what `pnpm test` runs — including in
// `release.yml`'s gate, on a runner with no `duckdb`. These suites write a corpus with the
// `duckdb` binary and read it back through `src/`, which is why they have a config of their own
// rather than a glob in that one. `vitest.config.base.ts` is not extended because its `happy-dom`
// environment is the wrong default for every file here — each of them opens a file off disk.
export default defineConfig({
  test: {
    environment: 'node',
    globals: true,
    reporters: ['default'],
    include: ['integration/**/*.test.ts'],
  },
});
