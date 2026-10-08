/**
 * @fossil-lang/executor — the DataFusion executor that runs fossil mappings in the browser and in
 * Node (the heavy, lazy-loaded counterpart to `@fossil-lang/wasm`), and the one writer of a corpus.
 *
 *   import { run } from '@fossil-lang/executor';
 *
 *   const report = await run(program, { host, job });           // under the job's prefix
 *   const { report, files } = await run(program, { files, path }); // in memory, for Node
 *
 * `host` is the `Host` from `@fossil-lang/types`: it vends `read` per connection and `write` on
 * the job. Recording the outcome is the host's.
 */

// The wasm-bindgen glue (`../pkg/fossil_df_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `run.ts` (the executor) — never here in the
// entry: importing the stateful glue from the package entry lets a
// `sideEffects:false` bundler duplicate it, splitting `init()`'s wasm instance
// from the one the executor uses. This entry only re-exports.
export { initFossilExecutor, type InitInput } from './load.js';
export { run, type CorpusFile, type RunOptions } from './run.js';
