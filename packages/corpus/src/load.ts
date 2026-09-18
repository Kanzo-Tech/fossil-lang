// '../pkg/fossil_graph_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (repo .gitignore `packages/corpus/pkg/`) but
// always present at build time; its `.d.ts` is resolved via the file's
// `@ts-self-types` pragma — same pattern as packages/wasm.
import init from '../pkg/fossil_graph_wasm.js';

/**
 * Options for {@link initFossilGraphWasm}.
 *
 * The four resolutions a bundler can want are documented where a consumer can still say one —
 * `OpenOptions.wasmUrl` in `./corpus.ts`. This type is not re-exported.
 */
export interface InitFossilGraphWasmOpts {
  /** URL or path to `fossil_graph_wasm_bg.wasm`. */
  wasmUrl: string | URL | Request | Response;
}

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-graph-wasm module. Memoised — subsequent calls return the
 * same promise, so the second corpus in a process costs the check and nothing
 * else.
 *
 * **Internal, and that is the change.** It was exported from `./index.ts`
 * beside `open` and had to be awaited before it, which is the largest
 * thing the module surface leaked: a consumer had to know there IS a wasm
 * module, and had to sequence two calls in the right order against a package
 * whose whole claim is that a corpus is a URL. `open` awaits it now and
 * `OpenOptions.wasmUrl` is the one thing about it a caller can still
 * need to say. The rejected alternative was keeping it exported "for a caller
 * that wants to pay the boot up front" — which is a second door onto a
 * memoised promise, i.e. the thing `@fossil-lang/corpus` has twice already
 * removed. It stays a module export because `tests/boot.ts` instantiates from
 * a `BufferSource` that no bundler resolution names, and because
 * `./address.ts` needs it up before `addressManifests` can address anything.
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
