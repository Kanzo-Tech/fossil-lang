import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/codemirror-fossil. The base supplies
// happy-dom + v8 coverage. We need:
//   - `node` environment (NOT happy-dom): the parser smoke test boots the
//     wasm module via node:fs to load fossil_wasm_bg.wasm off disk, same as
//     @fossil-lang/wasm's vitest config.
//   - tests/**/*.test.ts include glob.
//
// happy-dom can't supply node:fs, and the autocomplete tests don't actually
// touch DOM APIs (they exercise the CompletionSource function directly), so
// the node environment is sufficient for both test files.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
      setupFiles: ['./tests/setup.ts'],
    },
  }),
);
