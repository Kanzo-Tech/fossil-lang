import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/storage. Like @fossil-lang/corpus, the
// WASM smoke test reads fossil_storage_wasm_bg.wasm off disk via node:fs/node:url,
// so we OVERRIDE the base happy-dom environment to `node`.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
