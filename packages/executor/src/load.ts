// '../pkg/fossil_df_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (`packages/executor/pkg/`) but always
// present at build time. Its .d.ts is consumed via the file's
// `/* @ts-self-types */` pragma — TS 5.6 resolves it automatically.
import init from '../pkg/fossil_df_wasm.js';

/** Options for {@link initFossilExecutor}. */
export interface InitFossilExecutorOpts {
  /**
   * URL or path to `fossil_df_wasm_bg.wasm`. Consumers control resolution:
   *  - Vite: `import wasmUrl from '@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm?url'`
   *  - Next.js: serve from `public/` and pass the static URL
   *  - Web Worker: `new URL('@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm', import.meta.url)`
   *  - Node test: the `.wasm` bytes (BufferSource) read off disk
   *
   * Accepts `string` / `URL` / `Request` / `Response` (the
   * `wasm-bindgen --target web` init signature).
   */
  wasmUrl: string | URL | Request | Response;
}

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-df-wasm executor module. MUST be awaited before constructing
 * {@link FossilExecutor}. Memoised — subsequent calls return the same promise.
 *
 * This is the HEAVY artefact (datafusion + arrow + parquet-rs). Lazy-load it
 * only when the user actually runs a job — do NOT call it on page-load (that's
 * what the light `@fossil-lang/wasm` LSP module is for).
 */
export function initFossilExecutor(opts: InitFossilExecutorOpts): Promise<unknown> {
  if (!_initPromise) {
    _initPromise = init({ module_or_path: opts.wasmUrl as never });
  }
  return _initPromise;
}

/** For tests + hot-reload — reset the memoised promise. Not exported from index. */
export function __resetForTests(): void {
  _initPromise = null;
}
