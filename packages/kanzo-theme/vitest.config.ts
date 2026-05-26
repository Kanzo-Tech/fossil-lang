import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package vitest config for @kanzo/theme. Extends the workspace base
// (happy-dom + globals). The brand tokens + KanzoThemeProvider need
// happy-dom for the rendered-component assertion that the wrapping div
// carries the inline-style --fossil-* CSS vars.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ['tests/**/*.test.{ts,tsx}'],
      setupFiles: ['./tests/setup.ts'],
    },
  }),
);
