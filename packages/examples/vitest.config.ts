import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package overrides for @fossil-lang/examples. The base supplies the
// happy-dom environment + v8 coverage; here we:
//   - point at tests/ (default base looks at any include glob)
//   - switch the environment to `node` because nothing in this package's
//     test surface touches DOM APIs; using node is ~30% faster and avoids
//     any happy-dom quirks for a pure-string assertion suite
//
// Vitest IS Vite under the hood, so `import './hello.fossil?raw'` resolves
// natively in tests — no additional plugin needed.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      environment: 'node',
      include: ['tests/**/*.test.ts'],
    },
  }),
);
