/**
 * @fossil-lang/executor — the DataFusion executor that runs fossil mappings in
 * the browser (the heavy, lazy-loaded counterpart to the LSP @fossil-lang/wasm).
 *
 * A job, end to end — documents, sources, run, upload, completion:
 *
 *   import { initFossilExecutor, runJob } from '@fossil-lang/executor';
 *   import wasmUrl from '@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm?url'; // Vite
 *
 *   await initFossilExecutor({ wasmUrl });           // lazy — only when running a job
 *   const report = await runJob(program, { host, output });
 *
 * `host` is the `SourceHost` from `@fossil-lang/types`; `output` signs the PUTs
 * and records the outcome. {@link FossilExecutor} is the step-by-step surface
 * `runJob` drives.
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
export type { Job, JobOutput, CompletePayload, RunJobOptions } from './run-job.js';

import type { Channel } from '@fossil-lang/types';

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

/** A source the program reads, as enumerated by {@link FossilExecutor.sources}. */
export interface SourceDescriptor {
  /** The locator fossil resolved from what the program wrote — sign it to fetch. */
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

/**
 * One projection of the corpus's sequence: a scale, a path, and the columns.
 *
 * The payload is the entry at `scale: 1`, a level is the entry at `scale: 4^k`, an adjacency is a
 * `scale: 1` entry with an `aligned_by`, and a level of a relation is a coarser one with it. The
 * four vocabularies this replaced — `property_groups`, a vertex `levels`, `adj_lists` and an edge
 * `levels` — were one rule written four times. `path` and `scale` are OME-NGFF's own spellings.
 */
export interface Projection {
  /** Relative to the type's own `prefix`, with the trailing separator: `""`, `l1/`, `by_source/`. */
  path: string;
  /** How many rows of the sequence one row here stands for — `1` for a payload or an adjacency. */
  scale: number;
  /** `src` or `dst` on an edge projection; absent on a vertex one. */
  aligned_by?: string;
  /** Whether the rows are sorted by `aligned_by`'s column; absent on a vertex projection. */
  ordered?: boolean;
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
  /** Every projection of this type: the payload at `scale: 1`, and one per written level. */
  projections: Projection[];
  /**
   * The identity index — a second copy of the type ordered by subject instead of by position.
   * Absent where the writer wrote none; a lookup then scans instead of seeking.
   */
  index?: VertexIndex;
  /**
   * Which coordinate systems this type's rows carry, and where each one came from.
   *
   * Absent means **not declared**, which is neither "none" nor "derived" — a corpus written
   * before the field existed reads back this way, and inventing a default here is the defect the
   * field exists to remove. An empty array is the third state: the type carries no system.
   */
  coordinates?: CoordinateSystem[];
  /**
   * Which channels this type's rows carry — **what a reader draws them WITH**, rather than where it
   * puts them.
   *
   * The three states are {@link coordinates}' three states, and they are read the same way: absent
   * means **not declared**, which is what a corpus written before the field reads back as and what
   * leaves a reader deriving an encoding for itself; an empty array is the writer saying the type
   * carries no channel; a list is the declaration.
   */
  channels?: Channel[];
  version: string;
}

/** Where a vertex type's identity index lives, and what it is ordered by. */
export interface VertexIndex {
  prefix: string;
  ordered_by: string;
  chunk_size: number;
}

/**
 * One coordinate system over a vertex type's rows: the two columns holding it, and **where the
 * numbers in them came from** — which is the one fact that decides whether a far view of this
 * type means anything.
 */
export interface CoordinateSystem {
  name: string;
  x: string;
  y: string;
  provenance: Provenance;
  /** What derived them, for a `derived` system. Absent where nothing recorded it. */
  derived_by?: string;
}

/**
 * `geographic` and `embedded` are data — the plane is a space and its density is information
 * about that space. `derived` is a drawing algorithm's choice, and its density is a statement
 * about the algorithm rather than about the graph.
 */
export type Provenance = 'geographic' | 'embedded' | 'derived';

/**
 * The `channels:` entry and the scale it dispatches on — **re-exported, not restated.**
 *
 * They live in `@fossil-lang/types` because a channel is the one document of this manifest with a
 * second reader: `@fossil-lang/draw` turns a declared channel into an encoding having run nothing,
 * and making that reader install this package's 22 MB of `fossil_df_wasm` to name one interface is
 * a worse arrangement than the shape sitting where both can reach it. Everything else of the
 * manifest stays here, beside the run that hands it back. See `@fossil-lang/types/src/channel.ts`.
 */
export type { Channel, Scale } from '@fossil-lang/types';

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
  /** One per orientation at `scale: 1` — the adjacency — and one per written level. */
  projections: Projection[];
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
 *
 * **It is partial, and the absences are deliberate rather than the wire's.** A
 * `VertexInfo` on the wire also carries `cells:` — the pyramid of summary rows,
 * its rungs, their quotients and the channel its `mode` names — and this
 * interface does not, because transcribing that tree by hand for no consumer is
 * the kind of speculative mirror that drifts, which is what the paragraph above
 * is about. A reader that needs it reads the manifest YAML, which is what
 * `apps/playground` does for `channels:` today. So read an absence here as
 * «nothing in TypeScript has asked», never as «the wire does not carry it».
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
