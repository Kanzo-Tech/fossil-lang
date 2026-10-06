// '../pkg/fossil_df_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (`packages/executor/pkg/`) but always
// present at build time and in the published tarball. Its .d.ts is consumed via
// the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_df_wasm.js';
import type { InitInput } from '../pkg/fossil_df_wasm.js';

import { loader } from '@fossil-lang/types';

export type { InitInput };

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
export const initFossilExecutor = loader<InitInput>('fossil_df_wasm_bg.wasm', init);
