import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// @fossil-lang/executor smoke runs in `node` (not happy-dom): it reads
// fossil_df_wasm_bg.wasm off disk via node:fs and exercises the real WASM
// executor end-to-end.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
      // The executor wasm is large + datafusion plans take real CPU; give the
      // single integration test room beyond the default 5s.
      testTimeout: 30_000,
    },
  }),
);
