// '../pkg/fossil_wasm.js' is a wasm-bindgen --target web output emitted by `pnpm run build:wasm`.
// It is gitignored (repo .gitignore `packages/wasm/pkg/`) but always present at build time and in
// the published tarball. Its `.d.ts` is consumed via the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_wasm.js';
import type { InitInput } from '../pkg/fossil_wasm.js';

import { boot } from '@fossil-lang/types';

export type { InitInput };

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-wasm module. MUST be awaited before calling any of
 * {@link tokenize}, {@link tokenKinds}, or instantiating
 * {@link FossilWorkspace}. A boot that succeeded is kept; one that failed is
 * not, so the next call tries again — `module/unreachable` for a module that could not be
 * fetched within 60 s, `internal/bug` for one that would not instantiate.
 *
 * **Called with nothing, the module finds its own `.wasm`.** The glue resolves
 * `new URL('fossil_wasm_bg.wasm', import.meta.url)`, which is the pattern Vite,
 * webpack 5 and Turbopack all recognise and emit as an asset — so a bundled host
 * writes `await initFossilWasm()` and copies nothing.
 *
 * `wasm` is for the host with no bundler to do that: Node, whose `fetch` rejects
 * `file://`, hands the bytes (`BufferSource`) or a `Response`.
 */
export function initFossilWasm(wasm?: InitInput): Promise<unknown> {
  if (_initPromise === null) {
    // The object form: the positional one still works but logs a deprecation warning.
    const pending = boot('fossil_wasm_bg.wasm', () => init(wasm === undefined ? undefined : { module_or_path: wasm }));
    _initPromise = pending;
    pending.catch(() => {
      if (_initPromise === pending) _initPromise = null; // forgotten, so the next call boots again
    });
  }
  return _initPromise;
}

/**
 * For tests + hot-reload — reset the memoised promise. NOT exported from the
 * package index (test-internal helper).
 */
export function __resetForTests(): void {
  _initPromise = null;
}
