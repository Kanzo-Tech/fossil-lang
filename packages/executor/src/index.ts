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
 *   const { files, report } = await exec.run(program, sources, jobDest, shex);
 *   // signed-PUT each file.path ← file.bytes, then PATCH the job with report.
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

/**
 * One column a vertex type's manifest declares — the wire shape of
 * `fossil_sinks::manifest::Property`.
 *
 * **It carried `rdf_uri` and `xsd_datatype` and no longer does**, because the
 * manifest never did: those two lived on the deleted `RunStatus` alone. The
 * type IRI survives on {@link VertexInfo.iri} and {@link EdgeInfo.iri}; a
 * property's predicate has no field in the format today.
 */
export interface Property {
  name: string;
  /** The GraphAr storage spelling: `string`, `int64`, `double`, `bool`, … */
  data_type: string;
  is_primary: boolean;
  is_nullable?: boolean;
}

/** A group of columns stored together, one file per group per tile. */
export interface PropertyGroup {
  file_type: string;
  properties: Property[];
}

/** One vertex type's manifest document (`vertex/<Type>.vertex.yml`). */
export interface VertexInfo {
  type: string;
  /** The RDF type IRI; absent for a non-RDF graph. */
  iri?: string;
  /** Rows across every tile under {@link prefix}. */
  vertex_count: number;
  /** Rows per tile; tile `k` is `dense_id` in `[k·chunk_size, (k+1)·chunk_size)`. */
  chunk_size: number;
  /** Where the tiles are, e.g. `vertex/Person/` — the trailing separator is part of it. */
  prefix: string;
  property_groups: PropertyGroup[];
  version: string;
}

/** One orientation of an edge's adjacency, and how it is addressed. */
export interface AdjList {
  ordered: boolean;
  /** `src` or `dst` — the endpoint column whose tile addresses this half. */
  aligned_by: string;
  /** Relative to {@link EdgeInfo.prefix}: `by_source/`, `by_target/`. */
  prefix: string;
  file_type: string;
}

/** One edge type's manifest document (`edge/<dir>/<dir>.edge.yml`). */
export interface EdgeInfo {
  src_type: string;
  edge_type: string;
  /** The predicate IRI; absent for a non-RDF graph. */
  iri?: string;
  dst_type: string;
  /** Rows of ONE orientation — the two are one relation stored twice. */
  edge_count: number;
  chunk_size: number;
  src_chunk_size: number;
  dst_chunk_size: number;
  directed: boolean;
  prefix: string;
  adj_lists: AdjList[];
  property_groups: PropertyGroup[];
  version: string;
}

/** The `graph.graph.yml` index — the one entry point, naming every other document. */
export interface GraphInfo {
  name: string;
  prefix: string;
  /** Rel-paths of the per-type vertex documents, positionally {@link RunReport.vertices}. */
  vertices: string[];
  /** Rel-paths of the per-type edge documents, positionally {@link RunReport.edges}. */
  edges: string[];
  version: string;
}

/** How many rows of one edge type's input resolved no endpoint pair. */
export interface EdgeDrops {
  /** The edge's manifest `prefix` — a pointer into {@link RunReport.edges}. */
  prefix: string;
  /** Always present, `0` included: «nothing dropped» is not «this writer does not count». */
  dropped: number;
}

/**
 * What a run tells its caller — the wire shape of `fossil_df::RunReport`.
 *
 * **It is the manifest.** `graph`/`vertices`/`edges` are the same values as the
 * YAML documents in {@link ExecutorResult.files}, already parsed. Only `dest`
 * and `dropped` are not in the corpus: the caller chose the first, and the
 * second is a fact about the write.
 *
 * This replaced a `RunStatus` that was a second account of the same dataset —
 * seven of its nine fields respelled `graph.yaml`, and it named
 * `vertex/<Type>.parquet`, a file the native layout pass deletes. Keep this
 * interface derived from the Rust by hand only until something generates it.
 */
export interface RunReport {
  dest: string;
  graph: GraphInfo;
  vertices: VertexInfo[];
  edges: EdgeInfo[];
  dropped: EdgeDrops[];
}

/** The result of {@link FossilExecutor.run}. */
export interface ExecutorResult {
  /** The W0b GraphAr tree as bytes — signed-PUT each `path` ← `bytes`. */
  files: GraphArFile[];
  /** The manifest that tree carries, plus `dest` and the drops. */
  report: RunReport;
}
