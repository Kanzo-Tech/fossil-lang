// '../pkg/fossil_graph_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (repo .gitignore `packages/corpus/pkg/`) but
// always present at build time; its `.d.ts` is resolved via the file's
// `@ts-self-types` pragma — same pattern as packages/wasm.
import init from '../pkg/fossil_graph_wasm.js';

/** Options for {@link initFossilGraphWasm}. */
export interface InitFossilGraphWasmOpts {
  /**
   * URL or path to `fossil_graph_wasm_bg.wasm`. Consumers control resolution
   * (mirrors @fossil-lang/wasm):
   *  - Vite: `import wasmUrl from '@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm?url'`
   *  - Next.js: serve from `public/` and pass the static URL
   *  - Web Worker: `new URL('@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm', import.meta.url)`
   *  - Node test: a `file://` URL resolved from `import.meta.url`
   */
  wasmUrl: string | URL | Request | Response;
}

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-graph-wasm module. MUST be awaited before any
 * {@link createGraphClient} verb call (the underlying `dispatch_graph` throws
 * if the WASM module is not yet instantiated). Memoised — subsequent calls
 * return the same promise.
 */
export function initFossilGraphWasm(opts: InitFossilGraphWasmOpts): Promise<unknown> {
  if (!_initPromise) {
    _initPromise = init({ module_or_path: opts.wasmUrl as never });
  }
  return _initPromise;
}

/** For tests + hot-reload — reset the memoised init promise. NOT re-exported. */
export function __resetForTests(): void {
  _initPromise = null;
}
