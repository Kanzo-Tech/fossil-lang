import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// The base supplies happy-dom, which is what CodeMirror needs: `EditorView`
// touches `document` on construction even when it is never attached. The tests
// here never instantiate the wasm module — `tokenize` is injected (see
// `TokenSource`), so a fake row stream exercises
// the whole mapping without a 3.5 MB binary in the loop.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ['tests/**/*.test.ts'],
    },
  }),
);
