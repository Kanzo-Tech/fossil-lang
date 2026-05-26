import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package vitest config for @fossil-lang/ui. Extends the workspace base
// (happy-dom + globals). Primitives + setup hooks land in plans 10-03/04/05;
// this scaffold only runs cx() + barrel smoke tests.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ['tests/**/*.test.{ts,tsx}'],
      setupFiles: ['./tests/setup.ts'],
    },
  }),
);
