// '../pkg/fossil_df_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (`packages/executor/pkg/`) but always
// present at build time and in the published tarball. Its .d.ts is consumed via
// the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_df_wasm.js';
import type { InitInput } from '../pkg/fossil_df_wasm.js';

import { boot, loader } from '@fossil-lang/types/internal';

export type { InitInput };

/** The executor's module, as wasm-bindgen's glue names it. */
const MODULE = 'fossil_df_wasm_bg.wasm';

/**
 * Whether a run here goes to a worker: where `globalThis.Worker` is a constructor — a browser, Deno,
 * Bun — and not in Node. The platform decides, not an option (`/docs/design/off-the-page`).
 */
export const hasWorker = (): boolean => typeof (globalThis as { Worker?: unknown }).Worker === 'function';

/** Instantiate the executor in THIS thread: Node's run, and the worker's. */
export const bootHere = loader<InitInput>(MODULE, init);

let compiled: Promise<WebAssembly.Module> | null = null;

/**
 * The executor's module, compiled once on the page — `WebAssembly.compileStreaming` — and posted to
 * each run's worker, which instantiates it without fetching or compiling again: a `Module` is
 * structured-cloneable for exactly this. A compile that failed is forgotten, so the next run tries
 * again.
 */
export function compiledExecutor(): Promise<WebAssembly.Module> {
  if (compiled === null) {
    const url = new URL(`../pkg/${MODULE}`, import.meta.url);
    // eslint-disable-next-line no-restricted-syntax -- `boot` holds this fetch to MODULE_MS, as it holds the glue's
    const pending = boot(MODULE, () => WebAssembly.compileStreaming(fetch(url)));
    compiled = pending;
    pending.catch(() => {
      if (compiled === pending) compiled = null;
    });
  }
  return compiled;
}

/**
 * Boot the fossil-df-wasm executor module ahead of the first run. {@link run} boots it itself; this
 * is how a host pays for it early, and how Node, whose `fetch` rejects `file://`, hands over the
 * bytes first.
 *
 * Where runs go to a worker it COMPILES the module on the page, which every run's worker then
 * instantiates; elsewhere — Node, or given the bytes — it instantiates it here. A boot that
 * succeeded is kept; one that failed is not, so the next call tries again — `module/unreachable`
 * for a module that could not be fetched within 60 s, `internal/bug` for one that would not compile
 * or instantiate.
 *
 * This is the HEAVY artefact (datafusion + arrow + parquet-rs): `run` fetches it on the first run,
 * not on page-load.
 */
export function initFossilExecutor(wasm?: InitInput): Promise<unknown> {
  return wasm === undefined && hasWorker() ? compiledExecutor() : bootHere(wasm);
}
