/**
 * useLspWorker — module-singleton LSP Worker + @codemirror/lsp-client wiring.
 *
 * Per ADR-0026 (Two Workers, Two Lifecycles — LSP Worker is long-lived,
 * host-singleton) + RESEARCH.md Pattern 7 (module-scope, NOT React state —
 * the reconciler churns React state across renders, but Workers are
 * heavyweight resources that MUST persist).
 *
 * Asymmetric API enforcement: there is NO `resetLsp()` export anywhere in
 * this package. {@link useResetPlayground} terminates DuckDB only; the LSP
 * Worker survives. Adding LSP reset would require a deliberate code change
 * that ADR-0026 calls out for code review.
 *
 * Test-internal escape hatch: {@link __resetLspForTests} exposes the
 * module-state teardown so Vitest can isolate test cases. NOT re-exported
 * from `src/index.ts`.
 */

import { useEffect, useState } from 'react';
import { LSPClient } from '@codemirror/lsp-client';
import { createWorkerTransport } from '@fossil-lang/editor';

/**
 * Module-singleton Worker + LSPClient. Survives every React unmount/remount
 * inside the host's tab (e.g., React 18 Strict Mode double-mount,
 * route-level remounts). Only torn down on tab close OR explicit
 * {@link __resetLspForTests} call.
 */
let _worker: Worker | null = null;
let _client: LSPClient | null = null;

/**
 * Options the host passes via {@link useLspWorker}.
 */
export interface UseLspWorkerOpts {
  /**
   * URL to the fossil-wasm artefact. Forwarded to the LSP Worker via the
   * `__boot` bootstrap message — the Worker awaits `initFossilWasm({ wasmUrl })`
   * before installing the Rust dispatcher.
   */
  wasmUrl: string | URL;
  /**
   * Optional override for the LSP Worker URL. Useful in tests that want to
   * inject a stub Worker via `new URL(...)`. Defaults to the bundled
   * `../workers/lsp.worker.js` resolved relative to this module.
   *
   * NOTE: the default uses `.js` (not `.ts`) so the `new URL(...,
   * import.meta.url)` pattern resolves against the PUBLISHED `dist/` tree
   * in every bundler — webpack 5 (Next.js, Rspack), Vite, Parcel, esbuild.
   * Vite is forgiving of `.ts` extensions in source resolution; webpack and
   * the ESM spec are not. The fix for 08-11's Next.js 15 build was a
   * one-character edit to this string (Rule 1 bug surfaced by 08-11).
   */
  workerUrl?: string | URL;
}

/**
 * Mount + return the LSP client. Returns `null` on first render (before the
 * client is constructed); flips to a concrete `LSPClient` on the next render
 * once `useEffect` runs.
 *
 * The client is shared across every consumer in the host tab — `LSPClient`
 * itself supports multiple `plugin()` consumers per the @codemirror/lsp-client
 * API. The playground component is typically the only consumer, but a host
 * could (in principle) mount multiple editors against the same workspace.
 */
export function useLspWorker(opts: UseLspWorkerOpts): LSPClient | null {
  const [client, setClient] = useState<LSPClient | null>(_client);

  useEffect(() => {
    if (_client) {
      // Already booted in a previous mount cycle — reuse.
      setClient(_client);
      return;
    }
    // Resolve the Worker URL against THIS module's location. Default uses
    // `.js` so it lands on the PUBLISHED `dist/workers/lsp.worker.js` in
    // every bundler (webpack/Vite/Parcel/esbuild). See UseLspWorkerOpts
    // .workerUrl docstring for the full rationale.
    const workerUrl =
      opts.workerUrl ?? new URL('../workers/lsp.worker.js', import.meta.url);
    _worker = new Worker(workerUrl, { type: 'module' });
    // Send the boot message so the Worker can resolve initFossilWasm. The
    // worker entry queues messages until boot completes; this is safe.
    _worker.postMessage({ type: '__boot', wasmUrl: opts.wasmUrl });

    const transport = createWorkerTransport(_worker);
    _client = new LSPClient({ rootUri: 'file:///playground' });
    _client.connect(transport);
    setClient(_client);

    // INTENTIONAL: no cleanup. ADR-0026 — the LSP Worker is long-lived; the
    // browser tears it down on tab close. React Strict Mode's double-mount
    // is safe because the module singleton short-circuits the second mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return client;
}

/**
 * Test-internal — terminate the module-singleton LSP Worker + reset state.
 * NOT exported from `src/index.ts`. Vitest's `tests/setup.ts` calls this in
 * `afterEach` to isolate test cases.
 */
export function __resetLspForTests(): void {
  _worker?.terminate();
  _worker = null;
  _client = null;
}
