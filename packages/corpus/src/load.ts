// '../pkg/fossil_graph_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (repo .gitignore `packages/corpus/pkg/`) but
// always present at build time and in the published tarball; its `.d.ts` is
// resolved via the file's `@ts-self-types` pragma — same pattern as packages/wasm.
import init from '../pkg/fossil_graph_wasm.js';
import type { InitInput } from '../pkg/fossil_graph_wasm.js';

export type { InitInput };

let _initPromise: Promise<unknown> | null = null;

/**
 * Boot the fossil-graph-wasm module. Memoised — subsequent calls return the
 * same promise, so the second corpus in a process costs the check and nothing
 * else.
 *
 * Called with nothing, the glue resolves `new URL('fossil_graph_wasm_bg.wasm',
 * import.meta.url)` and the host's bundler emits that file as an asset;
 * `OpenOptions.wasm` is what reaches `wasm` here, for a host with no bundler.
 *
 * **Internal.** `open` awaits it; a consumer never sequences a boot before the
 * door. It stays a module export because `tests/boot.ts` instantiates from a
 * `BufferSource` and because `./address.ts` needs it up before
 * `addressManifests` can address anything.
 */
export function initFossilGraphWasm(wasm?: InitInput): Promise<unknown> {
  if (!_initPromise) {
    _initPromise = init(wasm === undefined ? undefined : { module_or_path: wasm });
  }
  return _initPromise;
}

/** For tests + hot-reload — reset the memoised init promise. NOT re-exported. */
export function __resetForTests(): void {
  _initPromise = null;
}
