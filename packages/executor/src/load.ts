// '../pkg/fossil_df_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (`packages/executor/pkg/`) but always
// present at build time and in the published tarball. Its .d.ts is consumed via
// the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_df_wasm.js';
import type { InitInput } from '../pkg/fossil_df_wasm.js';

import { boot } from '@fossil-lang/types';

export type { InitInput };

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-df-wasm executor module. MUST be awaited before constructing
 * {@link FossilExecutor}. A boot that succeeded is kept; one that failed is
 * not, so the next call tries again — `module/unreachable` for a module that could not be
 * fetched within 60 s, `internal/bug` for one that would not instantiate.
 *
 * Called with nothing, the glue resolves `new URL('fossil_df_wasm_bg.wasm',
 * import.meta.url)` and the host's bundler emits that file as an asset. `wasm`
 * is for a host with no bundler (Node: the bytes, since its `fetch` rejects
 * `file://`).
 *
 * This is the HEAVY artefact (datafusion + arrow + parquet-rs). Lazy-load it
 * only when the user actually runs a job — do NOT call it on page-load (that's
 * what the light `@fossil-lang/wasm` module is for).
 */
export function initFossilExecutor(wasm?: InitInput): Promise<unknown> {
  if (_initPromise === null) {
    // The object form: the positional one still works but logs a deprecation warning.
    const pending = boot('fossil_df_wasm_bg.wasm', () => init(wasm === undefined ? undefined : { module_or_path: wasm }));
    _initPromise = pending;
    pending.catch(() => {
      if (_initPromise === pending) _initPromise = null; // forgotten, so the next call boots again
    });
  }
  return _initPromise;
}

/** For tests + hot-reload — reset the memoised promise. Not exported from index. */
export function __resetForTests(): void {
  _initPromise = null;
}
