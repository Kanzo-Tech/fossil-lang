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
 * undefined (reading '__wbindgen_malloc…')`). Mirrors `@fossil-lang/corpus`'s
 * `client.ts` split, which is the known-good shape.
 */
import type { DocumentWorkspace, Host, MissingDocument } from '@fossil-lang/types';

import { FossilExecutor as RawFossilExecutor } from '../pkg/fossil_df_wasm.js';
import type { ExecutorResult, RunReport, SourceDescriptor } from './index.js';

/**
 * One compiled fossil program, run on DataFusion in the browser. Call
 * {@link free} when done to release the wasm-side handle.
 *
 * It is a {@link DocumentWorkspace}: hand it to `resolveDocuments` from
 * `@fossil-lang/storage` before {@link sources} or {@link run}, so every document
 * the program names — its output shape among them — is registered. The run
 * decodes its output contract from those registrations and refuses without them.
 *
 * MUST be constructed only after {@link initFossilExecutor} has resolved.
 */
export class FossilExecutor implements DocumentWorkspace {
  readonly #raw: RawFossilExecutor;

  constructor(program: string) {
    this.#raw = new RawFossilExecutor(program);
  }

  setConnections(connections: Record<string, string>): void {
    this.#raw.setConnections(connections);
  }

  missingDocuments(): MissingDocument[] {
    return this.#raw.missingDocuments() as MissingDocument[];
  }

  registerDocument(key: string, text: string): void {
    this.#raw.registerDocument(key, text);
  }

  /** The sources to read — `uri` is the locator fossil resolved. */
  sources(): SourceDescriptor[] {
    return this.#raw.sources() as SourceDescriptor[];
  }

  /**
   * Run with the storage `host` vends: each source read through its connection's credential —
   * by range requests, not whole — and the GraphAr graph written under the one prefix `host`
   * vends `write` on for `job`. Answers the manifest it wrote.
   */
  run(host: Host, job: string): Promise<RunReport> {
    return this.#raw.run(host, job) as Promise<RunReport>;
  }

  /**
   * Run over files held in memory, for a host with no storage: `sources` maps each locator the
   * program reads to its bytes, and the output stays in memory under `dest`.
   */
  runInMemory(sources: Record<string, Uint8Array>, dest: string): Promise<ExecutorResult> {
    return this.#raw.runInMemory(sources, dest) as Promise<ExecutorResult>;
  }

  /** Release the wasm-side handle. */
  free(): void {
    this.#raw.free();
  }
}
