/**
 * @fossil-lang/executor — the DataFusion executor that runs fossil mappings in
 * the browser (the heavy, lazy-loaded counterpart to the LSP @fossil-lang/wasm).
 *
 * A job, end to end — documents, sources, run, write, completion:
 *
 *   import { initFossilExecutor, runJob } from '@fossil-lang/executor';
 *
 *   await initFossilExecutor();                      // lazy — only when running a job
 *   const report = await runJob(program, { id, host, complete });
 *
 * `host` is the `Host` from `@fossil-lang/types`: it vends `read` per connection
 * and `write` on the job. `complete` records the outcome. {@link FossilExecutor} is the step-by-step surface
 * `runJob` drives.
 */

// The wasm-bindgen glue (`../pkg/fossil_df_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `client.ts` (the class) — never here in the
// entry. See `client.ts` for why: importing the stateful glue from the package
// entry lets a `sideEffects:false` bundler duplicate it, splitting `init()`'s
// wasm instance from the one the class uses. This entry only re-exports.
export { FossilExecutor } from './client.js';

export { initFossilExecutor } from './load.js';
export type { InitInput } from './load.js';

export { runJob } from './run-job.js';
export type { Job, CompletePayload } from './run-job.js';

import type { DataRow } from './catalogue.generated.js';

/**
 * How a source is read — the reader `DataFusion` scans it with, or the provider that decodes it.
 *
 * It is the catalogue ROW's name: what the program wrote after `io.`, what
 * {@link FossilExecutor.sources} emits, and what `fossil-df-wasm` reads back
 * with `source_row`. It was a hand-written union of four literals; it is
 * generated from `catalogue.bnf` now, by `cargo xtask catalogue`, from the same
 * file the Rust side reads.
 */
export type SourceFormat = DataRow;

export { DATA_ROWS } from './catalogue.generated.js';

/** A source the program reads, as enumerated by {@link FossilExecutor.sources}. */
export interface SourceDescriptor {
  /** The locator fossil resolved from what the program wrote. */
  uri: string;
  format: SourceFormat;
  /** The connection the locator lies under, when the program wrote `@name/…`. */
  connection?: string;
}

/** One file of the written corpus: a path relative to its root + its encoded bytes. */
export interface CorpusFile {
  /** `fossil.json`, `vertex/<Type>.parquet` or `edge/<Src>_<label>_<Dst>.parquet`. */
  path: string;
  bytes: Uint8Array;
}

/** How many rows of one relation's input resolved no endpoint pair. */
export interface EdgeDrops {
  /** The edge table's `name` in `fossil.json`, `<Src>_<label>_<Dst>`. */
  table: string;
  /** Always present, `0` included: «nothing dropped» is not «this writer does not count». */
  dropped: number;
}

/**
 * What a run tells its caller — the wire shape of `fossil_df::RunReport`: the
 * two facts the corpus cannot hold about itself.
 *
 * **The manifest is not here.** It is `<dest>fossil.json`, written last; open
 * the corpus (`@fossil-lang/corpus`) or read that file to learn what was
 * written. The report used to carry the whole manifest again, typed here by
 * hand, and a second description of one dataset is one that can disagree with
 * it.
 */
export interface RunReport {
  /** The prefix the corpus was written under, `fossil.json` at its root. */
  dest: string;
  /** One entry per edge table, in the order `fossil.json` lists them. */
  dropped: EdgeDrops[];
}

/** The result of {@link FossilExecutor.runInMemory}. */
export interface ExecutorResult {
  /** The corpus as bytes, each `path` relative to `dest`. */
  files: CorpusFile[];
  report: RunReport;
}
