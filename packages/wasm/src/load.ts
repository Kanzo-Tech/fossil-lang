// '../pkg/fossil_wasm.js' is a wasm-bindgen --target web output emitted by `pnpm run build:wasm`.
// It is gitignored (repo .gitignore `packages/wasm/pkg/`) but always present at build time and in
// the published tarball. Its `.d.ts` is consumed via the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_wasm.js';
import type { InitInput } from '../pkg/fossil_wasm.js';

import { loader } from '@fossil-lang/types';

export type { InitInput };

/** What a call that boots the module on its own takes — {@link openProgram}'s two boot options,
 *  for the calls that need the module and no workspace. */
export interface BootOptions {
  /** The module's `.wasm`, for a host with no bundler — see {@link initFossilWasm}. */
  wasm?: InitInput;
  /** Stops the wait for the boot, rejecting with its reason. The boot itself is shared and goes on. */
  signal?: AbortSignal;
}

/**
 * Boot the fossil-wasm module. MUST be awaited before calling any of
 * {@link tokenize} or instantiating
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
export const initFossilWasm = loader<InitInput>('fossil_wasm_bg.wasm', init);
