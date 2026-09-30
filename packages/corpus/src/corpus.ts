/**
 * A corpus, open: its manifest, a scan over any of its tables, and — where the host allowed it —
 * SQL over the views it registered.
 *
 * ```ts
 * const corpus = await open(url, { engine });
 * corpus.manifest.vertex_tables                         // what is inside, read once
 * const scan = corpus.scan({ table: 'Person', filter: { bbox: [0, 0, 10, 10] }, select: ['dense_id', 'x', 'y'] });
 * const [batch] = await scan.read(scan.plan());
 * ```
 *
 * **Every table is a view** named after it in a catalog named after the corpus —
 * `"<url>"."Person"` — so a host's own SQL, Mosaic's included, reads the same relation `scan` does.
 */

import type { Manifest } from './manifest.js';
import type { Scan, ScanParams } from './scan.js';

/** A corpus, open. */
export interface Corpus {
  /** The name of the DuckDB catalog its views live in: the job or the URL `open` was given. */
  readonly url: string;
  /** `fossil.json`, parsed once when the corpus opened. */
  readonly manifest: Manifest;
  /**
   * **Read one table**, vertices or edges: a filter, a projection and a limit, bound now.
   *
   * @throws {CorpusReadError} for a table the manifest does not declare, a column its table does
   *   not declare, or a box over a table with no position — before any statement runs.
   */
  scan(params: ScanParams): Scan;
  /**
   * Give back what opening took: the catalog — once no other open corpus of the same name on the
   * same engine holds it — and, for a job's corpus, the credential it was read under.
   */
  close(): Promise<void>;
}

/** What {@link SqlCorpus.sql} answers: the columns, the rows as arrays, and whether it stopped short. */
export interface SqlResult {
  readonly columns: readonly string[];
  readonly rows: readonly (readonly unknown[])[];
  /** More rows than `limit` matched, and only `limit` came back. */
  readonly truncated: boolean;
}

/**
 * A corpus opened with `sql: 'allowed'` — {@link Corpus} plus the escape hatch.
 *
 * A second interface rather than an optional member, because the member is not optional: it is
 * present or it is not, and which one is decided at `open` by an argument the host wrote.
 */
export interface SqlCorpus extends Corpus {
  /**
   * **One statement, as written**, over the engine the corpus was opened on — `SUMMARIZE
   * "<url>"."Person"` for a table's column statistics. At most `limit` rows come back (10 000 by
   * default), whatever the statement's own `LIMIT` says. Values are the engine's: a 64-bit integer
   * is a `bigint`.
   */
  sql(statement: string, options?: { readonly limit?: number; readonly signal?: AbortSignal }): Promise<SqlResult>;
}
