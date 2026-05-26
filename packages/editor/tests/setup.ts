/**
 * Test setup for @fossil-lang/editor.
 *
 * Mocks the @fossil-lang/wasm transitively imported through
 * @fossil-lang/codemirror-fossil — happy-dom can't boot the real .wasm
 * (no bundler-resolvable URL + no Worker context). This mirrors the
 * playground's tests/setup.ts pattern verbatim.
 *
 * Also mocks Worker globally so transports/Worker tests can substitute
 * a controllable stub at the per-test level via the makeStubWorker helper.
 */
import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';

// Mock @fossil-lang/wasm — same shape the playground's setup.ts uses.
// Editor tests don't exercise the wasm boot; we just need the imports to
// resolve to no-op stubs so @fossil-lang/codemirror-fossil's `fossil()`
// extension can compose without throwing.
vi.mock('@fossil-lang/wasm', () => {
  return {
    initFossilWasm: vi.fn().mockResolvedValue(undefined),
    start_lsp_worker: vi.fn(),
    tokenize: vi.fn().mockReturnValue([]),
    semanticLegend: vi
      .fn()
      .mockReturnValue({ tokenTypes: [], tokenModifiers: [] }),
    FossilPlayground: class StubFossilPlayground {
      free(): void {}
    },
  };
});

afterEach(() => {
  cleanup();
});
