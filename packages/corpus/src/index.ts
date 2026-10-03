/**
 * @fossil-lang/corpus — attach a `fossil/1` corpus to the host's engine.
 *
 * A corpus is `fossil.json` and one Parquet file per table: one per vertex type, one per relation.
 * `open` reads the manifest, creates a view per table and two relations of the manifest itself, and
 * answers the function that detaches them. Everything after that is SQL — Mosaic's, the host's —
 * over `"<name>"."<Table>"`, `"<name>".fossil_tables` and `"<name>".fossil_columns`.
 *
 * ```ts
 * import { open } from '@fossil-lang/corpus';
 *
 * const close = await open(job, { engine, host });
 * // SELECT * FROM "<job>".fossil_tables WHERE kind = 'vertex'
 * await close();
 * ```
 *
 * `mapping` answers the same manifest's RDF meaning as an RML mapping in Turtle — a pure function of
 * `fossil.json`'s text, for a SHACL engine, a triplestore or any RML processor to read the corpus as
 * RDF with no fossil code (`/docs/format/reading/rdf`).
 *
 * It links no engine, decodes no Parquet and loads no WASM: DuckDB-WASM in a browser is the host's,
 * and it prunes row groups from the footers. `/docs/design/one-door` has why the door is this small.
 */

export { open } from './open.js';
export { mapping } from './mapping.js';
export { FOSSIL_FORMAT } from './manifest.js';

export type { Close, OpenOptions } from './open.js';
