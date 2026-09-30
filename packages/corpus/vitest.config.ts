import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/corpus: every suite opens a corpus off disk through
// DuckDB-WASM's node runtime, so the base happy-dom environment is OVERRIDDEN to `node`.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
