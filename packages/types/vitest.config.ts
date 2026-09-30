import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/types: `FossilError` is plain TypeScript, so `node`.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
