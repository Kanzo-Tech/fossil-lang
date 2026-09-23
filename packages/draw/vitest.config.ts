import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/draw. `node` rather than the base's
// happy-dom: nothing here touches a document, and the point of both modules is
// that they can be checked without one.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
