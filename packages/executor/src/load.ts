// '../pkg/fossil_df_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored (`packages/executor/pkg/`) but always
// present at build time and in the published tarball. Its .d.ts is consumed via
// the file's `/* @ts-self-types */` pragma.
import init from '../pkg/fossil_df_wasm.js';
import type { InitInput } from '../pkg/fossil_df_wasm.js';

import { loader } from '@fossil-lang/types/internal';

export type { InitInput };

/**
 * Boot the fossil-df-wasm executor module — **for a host with no bundler, and nothing else**:
 * {@link run} boots it itself, and this is how Node, whose `fetch` rejects `file://`, hands over the
 * bytes first. A bundled host's glue resolves `new URL('fossil_df_wasm_bg.wasm', import.meta.url)`
 * and its bundler emits that file as an asset.
 *
 * A boot that succeeded is kept; one that failed is not, so the next call tries again —
 * `module/unreachable` for a module that could not be fetched within 60 s, `internal/bug` for one
 * that would not instantiate.
 *
 * This is the HEAVY artefact (datafusion + arrow + parquet-rs): `run` fetches it on the first run,
 * not on page-load.
 */
export const initFossilExecutor = loader<InitInput>('fossil_df_wasm_bg.wasm', init);
