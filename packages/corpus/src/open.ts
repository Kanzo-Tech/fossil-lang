/**
 * The door: `fossil.json` read once, one view per table, and `close`.
 */

import { mount } from '@fossil-lang/storage';
import type { Engine, Host } from '@fossil-lang/types';

import type { Corpus, SqlCorpus, SqlResult } from './corpus.js';
import { CorpusManifestError, parseManifest, type EdgeTable, type VertexTable } from './manifest.js';
import { scanOf } from './scan.js';
import { CorpusReadError, ident, lit } from './sql.js';

/** The file every open reads first. */
const ENTRY_POINT = 'fossil.json';

/** Rows {@link SqlCorpus.sql} answers with when the caller names no limit. */
const SQL_LIMIT = 10_000;

/**
 * What {@link open} takes.
 */
export interface OpenOptions {
  /**
   * The page's engine — `@fossil-lang/types`' `Engine`, DuckDB-WASM in a browser. Every read goes
   * through it, `fossil.json` included, and a scan's signal reaches the statement it is running.
   */
  engine: Engine;
  /**
   * What vends the job's credential. With it, the first argument of {@link open} names a job and
   * not a URL: the corpus is the one prefix the host vends `read` on for `{ job }`, kept fresh by
   * `@fossil-lang/storage` for as long as the corpus is open.
   */
  host?: Host;
  /**
   * Whether this corpus puts a caller's SQL in front of the engine. `'withheld'` by default, and
   * `'allowed'` widens the answer to {@link SqlCorpus}: a withheld corpus does not carry a member
   * that refuses, it does not carry the member. It is not a security boundary — the engine is the
   * host's and so are the files — it is the host writing the decision down.
   */
  sql?: 'withheld' | 'allowed';
}

/**
 * How many open corpora hold each catalog, per engine: two opens of one name share its views, and
 * the first to close must not take them from the second.
 */
const holders = new WeakMap<object, Map<string, number>>();

/**
 * Open a corpus: `GET fossil.json`, check its `format`, and create a view per table. Nothing else
 * is read until a scan or a statement asks.
 *
 * ```ts
 * await open(url, { engine })                          // a corpus at a URL
 * await open(job, { engine, host })                    // a job's corpus, under the credential host vends
 * await open(url, { engine, sql: 'allowed' })          // …with `sql`
 * ```
 *
 * @throws {CorpusManifestError} when `fossil.json` does not read, is not JSON, declares a format
 *   other than `fossil/1`, or names one table twice — before any Parquet is read.
 * @throws {TypeError} when no `engine` is given.
 */
export function open(source: string, options: OpenOptions & { sql: 'allowed' }): Promise<SqlCorpus>;
export function open(source: string, options: OpenOptions): Promise<Corpus>;
export async function open(source: string, options: OpenOptions): Promise<Corpus> {
  const { engine, host } = options;
  if (typeof engine?.query !== 'function') {
    throw new TypeError("open() needs an engine: { engine } for a corpus at a URL, { engine, host } for a job's");
  }

  // What SQL calls each file of the corpus, and what closing gives back. A job's files are lent
  // under the host's credential — an Azure file one by one — and a URL's are named as they are.
  let names: (paths: readonly string[]) => Promise<string[]>;
  let release = async (): Promise<void> => {};
  if (host !== undefined) {
    const storage = await mount(engine, host, { job: source }, 'read');
    release = () => storage.close();
    if (storage.prefixes.length !== 1) {
      await release();
      throw new CorpusManifestError(`job ${source} vends ${storage.prefixes.length} prefixes, and a corpus lives under one`);
    }
    const prefix = storage.prefixes[0]!;
    names = (paths) => storage.files(paths.map((path) => `${prefix}${path}`));
  } else {
    const base = source.endsWith('/') ? source : `${source}/`;
    names = async (paths) => paths.map((path) => `${base}${path}`);
  }

  let counted = false;
  const held = holders.get(engine) ?? new Map<string, number>();
  holders.set(engine, held);
  const catalog = source;
  try {
    const [where] = await names([ENTRY_POINT]);
    let text: string;
    try {
      const answer = await engine.query(`SELECT content FROM read_text(${lit(where!)})`);
      text = String(answer.getChild('content')?.get(0));
    } catch (cause) {
      throw new CorpusManifestError(`${where} did not read: ${(cause as Error).message}`);
    }
    const manifest = parseManifest(text, where!);

    const tables = new Map<string, VertexTable | EdgeTable>();
    for (const table of [...manifest.vertex_tables, ...manifest.edge_tables]) {
      if (tables.has(table.name)) throw new CorpusManifestError(`${where} declares ${table.name} twice`);
      tables.set(table.name, table);
    }
    const files = await names([...tables.values()].map((t) => t.path));

    const relation = (table: string): string => `${ident(catalog)}.${ident(table)}`;
    held.set(catalog, (held.get(catalog) ?? 0) + 1);
    counted = true;
    await engine.query(`ATTACH IF NOT EXISTS ':memory:' AS ${ident(catalog)}`);
    let at = 0;
    for (const name of tables.keys()) {
      await engine.query(`CREATE OR REPLACE VIEW ${relation(name)} AS SELECT * FROM read_parquet(${lit(files[at++]!)})`);
    }

    let closed = false;
    const corpus: Corpus = {
      url: catalog,
      manifest,
      scan: scanOf(engine, relation, tables),
      async close() {
        if (closed) return;
        closed = true;
        const left = (held.get(catalog) ?? 1) - 1;
        if (left > 0) held.set(catalog, left);
        else {
          held.delete(catalog);
          await engine.query(`DETACH DATABASE IF EXISTS ${ident(catalog)}`);
        }
        await release();
      },
    };
    if (options.sql !== 'allowed') return corpus;
    const widened: SqlCorpus = { ...corpus, sql: (statement, o) => sql(engine, statement, o) };
    return widened;
  } catch (cause) {
    if (counted) {
      const left = (held.get(catalog) ?? 1) - 1;
      if (left > 0) held.set(catalog, left);
      else {
        held.delete(catalog);
        await engine.query(`DETACH DATABASE IF EXISTS ${ident(catalog)}`).catch(() => undefined);
      }
    }
    await release();
    throw cause;
  }
}

async function sql(
  engine: Engine,
  statement: string,
  { limit = SQL_LIMIT, signal }: { readonly limit?: number; readonly signal?: AbortSignal } = {},
): Promise<SqlResult> {
  if (!(Number.isSafeInteger(limit) && limit >= 0)) throw new CorpusReadError(`limit ${limit} is not a count of rows`);
  const body = statement.trim().replace(/;+\s*$/, '');
  // The cap is an outer LIMIT whatever the statement says, and one row past it says it bit.
  const wrapped = `SELECT * FROM (${body}) AS _q LIMIT ${limit + 1}`;
  signal?.throwIfAborted();
  const table = await (signal === undefined ? engine.query(wrapped) : engine.query(wrapped, { signal }));
  signal?.throwIfAborted();
  const columns = table.schema.fields.map((f) => f.name);
  const children = columns.map((name) => table.getChild(name));
  const n = Math.min(table.numRows, limit);
  const rows = Array.from({ length: n }, (_, i) => children.map((c) => c?.get(i) ?? null));
  return { columns, rows, truncated: table.numRows > limit };
}
