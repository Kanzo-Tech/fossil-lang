import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package vitest config for @fossil-lang/editor. Extends the workspace
// base (happy-dom + globals). FossilEditor + transports tests land in
// plans 11-02/11-03; this scaffold only runs the barrel smoke test.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ['tests/**/*.test.{ts,tsx}'],
      setupFiles: ['./tests/setup.ts'],
    },
  }),
);
