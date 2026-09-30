/**
 * @fossil-lang/corpus — open a `fossil/1` corpus and read it.
 *
 * A corpus is `fossil.json` and one Parquet file per table: one per vertex type, one per relation.
 * This package reads the manifest with `JSON.parse`, registers a view per table on the host's
 * engine, and composes the SQL a scan runs. It links no engine, decodes no Parquet and loads no
 * WASM: DuckDB-WASM in a browser is the host's, and it prunes row groups from the footers.
 *
 * ```ts
 * import { open } from '@fossil-lang/corpus';
 *
 * const corpus = await open(url, { engine });
 * for (const table of corpus.manifest.vertex_tables) { … }
 * const scan = corpus.scan({ table: 'Person', select: ['dense_id', 'x', 'y'] });
 * const [batch] = await scan.read(scan.plan());
 * ```
 *
 * `open · manifest · scan · sql · close` is the whole door; `/docs/design/one-door` has why.
 */

export { open } from './open.js';
export { FOSSIL_FORMAT } from './manifest.js';

export type { Corpus, SqlCorpus, SqlResult } from './corpus.js';
export type { Filter, Literal } from './filter.js';
export type { EdgeTable, Endpoint, Manifest, Position, Property, VertexTable } from './manifest.js';
export type { OpenOptions } from './open.js';
export type { Batch, Scan, ScanParams, ScanTask } from './scan.js';
