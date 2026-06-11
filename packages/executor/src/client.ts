/**
 * The browser DataFusion executor handle — the `wasm-bindgen` class wrapped with
 * an ergonomic, typed surface.
 *
 * Kept in its OWN module (NOT the package entry) so the wasm-bindgen glue
 * (`../pkg/fossil_df_wasm.js`) is imported only from leaf modules — `load.ts`
 * (for `init`) and here (for the class). Under a bundler with
 * `"sideEffects": false`, importing the stateful glue from the entry chunk can
 * duplicate it: `init()` then populates the `wasm` binding in one instance while
 * the class reads `undefined` from another (→ `Cannot read properties of
 * undefined (reading '__wbindgen_malloc…')`). Mirrors `@fossil-lang/graph`'s
 * `client.ts` split, which is the known-good shape.
 */
import { FossilExecutor as RawFossilExecutor } from '../pkg/fossil_df_wasm.js';
import type {
  ConnectionRefs,
  ExecutorResult,
  SourceDescriptor,
  SourceInput,
} from './index.js';

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
