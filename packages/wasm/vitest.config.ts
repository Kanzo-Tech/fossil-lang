import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/wasm. The base supplies the
// happy-dom environment + v8 coverage; we OVERRIDE to `node` because the
// WASM smoke test needs the Node filesystem APIs (`node:fs`, `node:url`) to
// read fossil_wasm_bg.wasm off disk — happy-dom can't reach those.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
