// '../pkg/fossil_wasm.js' is a wasm-bindgen --target web output emitted by `pnpm run build:wasm`.
// It is gitignored (repo .gitignore `packages/wasm/pkg/`) but always present at build time and in
// the published tarball. Its `.d.ts` is consumed via the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_wasm.js';
import type { InitInput } from '../pkg/fossil_wasm.js';

import { loader } from '@fossil-lang/types/internal';

export type { InitInput };

/**
 * Boot the fossil-wasm module — **for a host with no bundler, and nothing else**. Every door
 * (`openProgram`, `inputs`, `formats`) boots it itself; this is how Node, whose `fetch` rejects
 * `file://`, hands over the bytes (`BufferSource`) or a `Response` first.
 *
 * A bundled host never calls it: the glue resolves `new URL('fossil_wasm_bg.wasm',
 * import.meta.url)`, the pattern Vite, webpack 5 and Turbopack all emit as an asset.
 *
 * A boot that succeeded is kept; one that failed is not, so the next call tries again —
 * `module/unreachable` for a module that could not be fetched within 60 s, `internal/bug` for one
 * that would not instantiate.
 */
export const initFossilWasm = loader<InitInput>('fossil_wasm_bg.wasm', init);
