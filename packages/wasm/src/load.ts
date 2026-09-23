// '../pkg/fossil_wasm.js' is a wasm-bindgen --target web output emitted by `pnpm run build:wasm`.
// It is gitignored (per 08-01 .gitignore + repo .gitignore `packages/wasm/pkg/`) but always present at build time.
// The accompanying fossil_wasm.d.ts is consumed via the file's `/* @ts-self-types="./fossil_wasm.d.ts" */` pragma —
// TS 5.6 resolves the `import` to that side-channel `.d.ts` automatically (no `@ts-expect-error` needed).
import init from '../pkg/fossil_wasm.js';

/** Options for {@link initFossilWasm}. */
export interface InitFossilWasmOpts {
  /**
   * URL or path to `fossil_wasm_bg.wasm`. Consumers control resolution:
   *  - Vite: `import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url'`
   *  - Next.js: serve from `public/` and pass the static URL (e.g. `'/wasm/fossil_wasm_bg.wasm'`)
   *  - Web Worker: `new URL('@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm', import.meta.url)`
   *  - Node test: a `file://` URL resolved from `import.meta.url`
   *
   * Accepts `string` (a URL string), `URL`, a `Request`, or a `Response` —
   * matching `wasm-bindgen --target web`'s init signature.
   */
  wasmUrl: string | URL | Request | Response;
}

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-wasm module. MUST be awaited before calling any of
 * {@link tokenize}, {@link semanticLegend}, or instantiating
 * {@link FossilPlayground}. Memoised — subsequent calls return the same promise.
 *
 * With `wasm-bindgen --target web`, consumers control
 * the `.wasm` URL — this avoids tying library users to a specific bundler's
 * `.wasm` import magic. See `packages/wasm/README.md` for consumer patterns.
 */
export function initFossilWasm(opts: InitFossilWasmOpts): Promise<unknown> {
  if (!_initPromise) {
    // wasm-bindgen 0.2.120 init() prefers the `{ module_or_path }` object form;
    // passing the URL/Request/Response/BufferSource as a positional argument
    // still works but logs a deprecation warning ("pass a single object
    // instead"). We use the object form to stay on the maintained path.
    _initPromise = init({ module_or_path: opts.wasmUrl as never });
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
