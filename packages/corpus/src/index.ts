/**
 * @fossil-lang/corpus — attach a `fossil/1` corpus to the host's engine.
 *
 * A corpus is `fossil.json` and one Parquet file per table: one per vertex type, one per relation.
 * `attach` reads the manifest, creates a view per table, two relations of the manifest itself and the
 * corpus as RDF, and answers the attachment that detaches them. Everything after that is SQL —
 * Mosaic's, the host's — over `"<name>"."<Table>"`, `"<name>".fossil_tables`,
 * `"<name>".fossil_columns` and `"<name>".triples`.
 *
 * ```ts
 * import { attach } from '@fossil-lang/corpus';
 *
 * await using corpus = await attach(job, { engine, host });
 * // SELECT * FROM "<job>".fossil_tables WHERE kind = 'vertex'
 * ```
 *
 * `triples` is the corpus's RDF meaning, one row per triple, for a SHACL engine or any reader of
 * triples to read it with no fossil code (`/docs/format/reading/rdf`).
 *
 * It links no engine, decodes no Parquet and loads no WASM: DuckDB-WASM in a browser is the host's,
 * and it prunes row groups from the footers. `/docs/design/one-door` has why the door is this small.
 */

export { attach } from './attach.js';
export { FOSSIL_FORMAT } from './manifest.js';

export type { Attachment, AttachOptions } from './attach.js';
