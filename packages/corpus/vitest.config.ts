import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/corpus. Like @fossil-lang/wasm, the
// WASM smoke test reads fossil_graph_wasm_bg.wasm off disk via node:fs/node:url,
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
