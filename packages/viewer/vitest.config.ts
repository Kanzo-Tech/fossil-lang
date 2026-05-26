import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package vitest config for @fossil-lang/viewer. Extends the workspace
// base (happy-dom + globals). FossilGraphView / FossilViewer / hooks /
// fallback tests land across plans 12-02..04; this scaffold (12-01) only
// runs a barrel smoke test that asserts VIEWER_PACKAGE_VERSION resolves.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ['tests/**/*.test.{ts,tsx}'],
      setupFiles: ['./tests/setup.ts'],
    },
  }),
);
