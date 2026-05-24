import { defineConfig } from 'vitest/config';

// Shared base for every @fossil-lang/* package's vitest.config.ts.
// Per-package configs extend this via:
//
//   import { defineConfig, mergeConfig } from 'vitest/config';
//   import base from '../../vitest.config.base';
//   export default mergeConfig(base, defineConfig({ /* per-package overrides */ }));
//
// happy-dom is chosen over jsdom for speed (~3x faster); switch to jsdom
// per-package if a happy-dom incompat surfaces (most React Testing Library
// tests pass under both).
export default defineConfig({
  test: {
    environment: 'happy-dom',
    globals: true,
    reporters: ['default'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'html'],
    },
  },
});
