import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// `node`, not the base's happy-dom: the guards read `grammar.bnf` and the conformance programs
// off disk, and check the forbidden forms through the real wasm module.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
