/**
 * Vitest setup — stubs the Worker constructor + cleans up module singletons.
 *
 * The LSP and DuckDB Workers boot real .wasm artefacts in production. In the
 * unit test environment that's both prohibitively slow AND requires bundler
 * magic happy-dom can't provide (resolving `new URL('../workers/lsp.worker.ts',
 * import.meta.url)` to a runnable Worker). The mock Worker:
 *   - accepts every postMessage as a no-op
 *   - terminate() is observable so the Reset assertions can verify it
 *
 * Production behaviour is exercised end-to-end in apps/landing/ via the
 * Playwright suite (08-11).
 */

import { beforeAll, afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';
import { __resetLspForTests } from '../src/hooks/useLspWorker.js';
import { resetDuckDb } from '../src/hooks/useDuckDb.js';

beforeAll(() => {
  /**
   * Mock Worker — drops every message + records terminate(). Tests that
   * need to assert termination spy on this constructor via `vi.spyOn`.
   */
  class MockWorker {
    public onmessage: ((e: MessageEvent) => void) | null = null;
    public onerror: ((e: ErrorEvent) => void) | null = null;
    constructor(_url: string | URL, _opts?: WorkerOptions) {
      // Record construction for tests that want to count Workers.
      MockWorker.constructed += 1;
    }
    static constructed = 0;
    static terminated = 0;
    postMessage(_msg: unknown): void {
      // no-op
    }
    terminate(): void {
      MockWorker.terminated += 1;
    }
    addEventListener(_type: string, _listener: EventListenerOrEventListenerObject): void {
      // no-op
    }
    removeEventListener(_type: string, _listener: EventListenerOrEventListenerObject): void {
      // no-op
    }
    dispatchEvent(_e: Event): boolean {
      return true;
    }
  }
  // Expose on globalThis so tests can reach the constructor counters.
  (globalThis as unknown as { __MockWorker: typeof MockWorker }).__MockWorker = MockWorker;
  globalThis.Worker = MockWorker as unknown as typeof Worker;

});

// Mock @fossil-lang/wasm GLOBALLY (top-level so it runs before any test file
// imports it transitively via @fossil-lang/codemirror-fossil's StreamParser).
//
// The real WASM boot requires a bundler-resolvable URL (Vite's `?url` magic
// + a Worker context); unit tests can't provide either. We replace:
//   - initFossilWasm: resolves immediately (useEffect chains progress)
//   - start_lsp_worker: no-op (Worker-scope-only in production)
//   - tokenize: returns [] (the StreamParser handles empty token streams
//     gracefully — the editor renders unstyled but functional)
//   - semanticLegend: returns a stub legend (LSP semantic-tokens layer
//     reads this; null breaks; empty arrays are safe)
//   - FossilPlayground: a stub class with the methods the playground might
//     call. compileFile returns empty SQL so the Run path is exercisable.
//
// Production behaviour is verified end-to-end in apps/landing/ Playwright.
vi.mock('@fossil-lang/wasm', () => {
  class StubFossilPlayground {
    free(): void {}
    compile(): { sql: string; manifest_yaml: string } {
      return { sql: '', manifest_yaml: '' };
    }
    classification(): Array<{ name: string; wasm_class: string }> {
      return [];
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    openFile(_path: string, _contents: string): any {
      return 0;
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    updateFile(_handle: any, _contents: string): void {}
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    closeFile(_handle: any): void {}
    check(): Array<unknown> {
      return [];
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    diagnosticsFor(_handle: any): Array<unknown> {
      return [];
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    compileFile(_handle: any): { sql: string; manifest_yaml: string } {
      return { sql: '', manifest_yaml: '' };
    }
    setTargetShex(_text: string): void {}
  }
  return {
    initFossilWasm: vi.fn().mockResolvedValue(undefined),
    start_lsp_worker: vi.fn(),
    tokenize: vi.fn().mockReturnValue([]),
    semanticLegend: vi.fn().mockReturnValue({ tokenTypes: [], tokenModifiers: [] }),
    FossilPlayground: StubFossilPlayground,
  };
});

afterEach(() => {
  cleanup();
  // Tear down module singletons so each test starts fresh.
  __resetLspForTests();
  resetDuckDb();
});
