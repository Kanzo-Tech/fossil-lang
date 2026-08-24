/**
 * @fossil-lang/executor — the DataFusion executor that runs fossil mappings in
 * the browser (the heavy, lazy-loaded counterpart to the LSP @fossil-lang/wasm).
 *
 * Flow (design §E2 — browser-driven, no mapping runtime on the server):
 *
 *   import { initFossilExecutor, FossilExecutor } from '@fossil-lang/executor';
 *   import wasmUrl from '@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm?url'; // Vite
 *
 *   await initFossilExecutor({ wasmUrl });           // lazy — only when running a job
 *   const exec = new FossilExecutor();
 *   const srcs = exec.sources(program, shex);        // what to fetch
 *   const sources = await Promise.all(srcs.map(async (s) => ({
 *     ...s, bytes: new Uint8Array(await (await fetch(signedUrlFor(s.uri))).arrayBuffer()),
 *   })));
 *   const { files, runStatus } = await exec.run(program, sources, jobDest, shex);
 *   // signed-PUT each file.path ← file.bytes, then PATCH the job with runStatus.
 */

// The wasm-bindgen glue (`../pkg/fossil_df_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `client.ts` (the class) — never here in the
// entry. See `client.ts` for why: importing the stateful glue from the package
// entry lets a `sideEffects:false` bundler duplicate it, splitting `init()`'s
// wasm instance from the one the class uses. This entry only re-exports.
export { FossilExecutor } from './client.js';

export { initFossilExecutor } from './load.js';
export type { InitFossilExecutorOpts } from './load.js';

export { runJob } from './run-job.js';
export type { JobTransport, CompletePayload, RunJobOptions } from './run-job.js';

import type { DataRow } from './catalogue.generated.js';

/**
 * The fetch strategy for a source — how the host must stage its bytes.
 *
 * It is the catalogue ROW's name: what the program wrote after `io.`, what
 * {@link FossilExecutor.sources} emits, and what `fossil-df-wasm` reads back
 * with `source_row`. It was a hand-written union of four literals; it is
 * generated from `catalogue.bnf` now, by `cargo xtask catalogue`, from the same
 * file the Rust side reads.
 */
export type SourceFormat = DataRow;

export { DATA_ROWS } from './catalogue.generated.js';

/**
 * The connection ref-map `{ name: baseUrl }` (the host's connections). A
 * `@name/path` source alias in the program resolves to `{baseUrl}/path`. Pass
 * the SAME map to {@link FossilExecutor.sources} and {@link FossilExecutor.run}
 * so the resolved URIs line up. Empty/omitted ⇒ every source URI is concrete.
 */
export type ConnectionRefs = Record<string, string>;

/** A source the program reads, as enumerated by {@link FossilExecutor.sources}. */
export interface SourceDescriptor {
  /** The program URI (`io.csv("…")`) — resolve it to a signed URL to fetch. */
  uri: string;
  format: SourceFormat;
}

/** A source the host fetched, fed to {@link FossilExecutor.run}. */
export interface SourceInput extends SourceDescriptor {
  /** The fetched bytes (CSV/JSON/Parquet content, or RDF Turtle text). */
  bytes: Uint8Array;
}

/** One GraphAr output file: a dataset-relative path + its encoded bytes. */
export interface GraphArFile {
  /** e.g. `vertex/Person.parquet`, `edge/<dir>/by_source.parquet`, `graph.graph.yml`. */
  path: string;
  bytes: Uint8Array;
}

/** A vertex property's wire status (the governance/DCAT metadata). */
export interface ColumnStatus {
  name: string;
  data_type: string;
  rdf_uri?: string | null;
  xsd_datatype?: string | null;
}

/** One vertex type's wire status. */
export interface VertexStatus {
  type: string;
  rdf_type?: string | null;
  file: string;
  count?: number | null;
  columns: ColumnStatus[];
}

/** One edge type's wire status (CSR + CSC adjacency file paths). */
export interface EdgeStatus {
  edge_type: string;
  src_type: string;
  dst_type: string;
  by_source: string;
  by_target: string;
  count?: number | null;
}

/**
 * The run report keasy turns into a DCAT catalog — the wire shape of
 * `fossil_df::run_status::RunStatus`.
 *
 * It carried a required `version: number`, and the Rust has had no such field
 * since `3db248e` deleted `WIRE_VERSION` and `is_compatible` as write-only.
 * `fossil-df-wasm` serialises the struct straight to JS, so every reader of
 * `runStatus.version` got `undefined` against a type that says it cannot be.
 * Nothing read it, which is why three days passed. Keep this interface derived
 * from the Rust by hand only until something generates it.
 */
export interface RunStatus {
  dest: string;
  vertices: VertexStatus[];
  edges: EdgeStatus[];
}

/** The result of {@link FossilExecutor.run}. */
export interface ExecutorResult {
  /** The W0b GraphAr tree as bytes — signed-PUT each `path` ← `bytes`. */
  files: GraphArFile[];
  /** The run report to PATCH back to the job. */
  runStatus: RunStatus;
}
