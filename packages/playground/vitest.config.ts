import { defineConfig, mergeConfig } from 'vitest/config';
import base from '../../vitest.config.base';

// Per-package vitest config — extends the workspace base (happy-dom).
// happy-dom is sufficient here because:
//   - the FossilPlayground component is rendered via @testing-library/react;
//   - we mock the LSP Worker + DuckDB-WASM in tests/setup.ts (Worker class is
//     stubbed so no real worker boot happens);
//   - the CONN-01 invariant test walks the rendered DOM, which happy-dom
//     supports out of the box.
//
// The `tsx` include extension is added so RTL tests in .tsx files are picked up
// (the base only lists .ts via vitest's default).
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ['tests/**/*.test.{ts,tsx}'],
      setupFiles: ['./tests/setup.ts'],
    },
    // happy-dom 15 needs JSX runtime for RTL renders — esbuild handles this
    // when the file extension is .tsx; vitest picks up tsconfig.json's
    // `jsx: "react-jsx"` automatically.
  }),
);
