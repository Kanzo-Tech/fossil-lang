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

// '../pkg/fossil_df_wasm.js' is a wasm-bindgen --target web output emitted by
// `pnpm run build:wasm`. Gitignored but present at build time; its .d.ts rides
// the file's `/* @ts-self-types */` pragma.
import { FossilExecutor as RawFossilExecutor } from '../pkg/fossil_df_wasm.js';

export { initFossilExecutor } from './load.js';
export type { InitFossilExecutorOpts } from './load.js';

/** The fetch strategy for a source — how the host must stage its bytes. */
export type SourceFormat = 'csv' | 'json' | 'parquet' | 'rdf';

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

/** The run report keasy turns into a DCAT catalog (`fossil-run-status` wire). */
export interface RunStatus {
  version: number;
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

/**
 * Runs a fossil program on DataFusion in the browser. One instance per job;
 * cheap to construct (the heavy state lives per `run` call). Call {@link free}
 * when done to release the wasm-side handle.
 *
 * MUST be constructed only after {@link initFossilExecutor} has resolved.
 */
export class FossilExecutor {
  readonly #raw: RawFossilExecutor;

  constructor() {
    this.#raw = new RawFossilExecutor();
  }

  /**
   * Enumerate the program's sources so the host knows what to fetch + how to
   * stage. Pure (no IO) — call it first, resolve each `uri` to a signed URL,
   * fetch the bytes, then pass them to {@link run}. `refs` resolves `@conn`
   * aliases (pass the same map to {@link run}).
   */
  sources(program: string, refs?: ConnectionRefs, shex?: string): SourceDescriptor[] {
    return this.#raw.sources(program, shex, refs ?? {}) as SourceDescriptor[];
  }

  /**
   * Execute `program` against the host-fetched `sources`, materialising the
   * GraphAr graph. `dest` labels the run (the job's object-storage prefix);
   * `shex` is the optional output schema text.
   *
   * Returns the output files (Parquet + manifest YAML, as bytes) plus the
   * `RunStatus`. The caller signed-PUTs each file and PATCHes the job.
   */
  run(
    program: string,
    sources: SourceInput[],
    dest: string,
    refs?: ConnectionRefs,
    shex?: string,
  ): Promise<ExecutorResult> {
    return this.#raw.run(program, shex, sources, dest, refs ?? {}) as Promise<ExecutorResult>;
  }

  /** Release the wasm-side handle. */
  free(): void {
    this.#raw.free();
  }
}
