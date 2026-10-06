/**
 * @fossil-lang/executor — the DataFusion executor that runs fossil mappings in
 * the browser (the heavy, lazy-loaded counterpart to the LSP @fossil-lang/wasm).
 *
 * A run, end to end — documents, sources, run, write:
 *
 *   import { initFossilExecutor, run } from '@fossil-lang/executor';
 *
 *   await initFossilExecutor();                      // lazy — only when running a job
 *   const report = await run(program, { host, job });
 *
 * `host` is the `Host` from `@fossil-lang/types`: it vends `read` per connection
 * and `write` on the job. Recording the outcome is the host's. {@link FossilExecutor}
 * is the step-by-step surface `run` drives.
 */

// The wasm-bindgen glue (`../pkg/fossil_df_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `client.ts` (the class) — never here in the
// entry. See `client.ts` for why: importing the stateful glue from the package
// entry lets a `sideEffects:false` bundler duplicate it, splitting `init()`'s
// wasm instance from the one the class uses. This entry only re-exports.
export { FossilExecutor } from './client.js';

export { initFossilExecutor } from './load.js';
export type { InitInput } from './load.js';

export { run } from './run.js';
export type { RunOptions } from './run.js';

import type { RunReport } from '@fossil-lang/types';

/** One file of the written corpus: a path relative to its root + its encoded bytes. */
export interface CorpusFile {
  /** `fossil.json`, `vertex/<Type>.parquet` or `edge/<Src>_<label>_<Dst>.parquet`. */
  path: string;
  bytes: Uint8Array;
}

/** The result of {@link FossilExecutor.runInMemory}. */
export interface ExecutorResult {
  /** The corpus as bytes, each `path` relative to `dest`. */
  files: CorpusFile[];
  report: RunReport;
}
