/**
 * @fossil-lang/executor — the DataFusion executor that runs fossil mappings in the browser and in
 * Node (the heavy, lazy-loaded counterpart to `@fossil-lang/wasm`), and the one writer of a corpus.
 *
 *   import { run } from '@fossil-lang/executor';
 *
 *   const report = await run(program, { host, into: { job } });                   // under the job's prefix
 *   const { report, files } = await run(program, { host, into: 'memory', limit }); // a sample, in memory
 *   const { report, files } = await run(program, { files, path });               // bytes in hand, for Node
 *
 * `host` is the `Host` from `@fossil-lang/types`: it vends `read` per connection and, into a job,
 * `write` on it. Recording the outcome is the host's. In a browser the run is in a worker the
 * executor starts (`./worker.js`, emitted beside this entry); a host's CSP must allow it.
 */

// The wasm-bindgen glue (`../pkg/fossil_df_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `here.ts` (the executor) — never here in the
// entry: importing the stateful glue from the package entry lets a
// `sideEffects:false` bundler duplicate it, splitting `init()`'s wasm instance
// from the one the executor uses. This entry only re-exports.
export { initFossilExecutor, type InitInput } from './load.js';
export { run, type FilesRun, type HostRun, type InMemoryRun, type RunOptions } from './run.js';
// What a run into memory answers, and what a reader registers with the engine (`registerFileBuffer`): one type,
// in `@fossil-lang/types`, so the reader needs no executor.
export type { CorpusFile } from '@fossil-lang/types';
