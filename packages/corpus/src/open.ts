/**
 * The door: `fossil.json` read once, one view per table, and `close`.
 */

import { mount, type Mount } from '@fossil-lang/storage';
import { FossilError, attachCause, type Engine, type Host } from '@fossil-lang/types';

import type { Corpus, SqlCorpus, SqlResult } from './corpus.js';
import { parseManifest, type EdgeTable, type VertexTable } from './manifest.js';
import { notACount, scanOf } from './scan.js';
import { ident, lit, query } from './sql.js';

/** The file every open reads first. */
const ENTRY_POINT = 'fossil.json';

/** The schema an attached database creates its views in, and so where a corpus's views are. */
const SCHEMA = 'main';

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
  /**
   * Stops the open: it rejects with the signal's reason, and what it had made — a mount, a catalog —
   * is given back. It reaches the host's credential request and every statement the open runs; the
   * corpus that comes back does not keep it.
   */
  signal?: AbortSignal;
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
 * @throws {FossilError} before any Parquet is read: `corpus/unreadable` when `fossil.json` does not
 *   read, `corpus/not-json`, `corpus/unsupported-format` for a format other than `fossil/1`,
 *   `corpus/duplicate-table` when it names one table twice; `corpus/not-a-location` for a URL
 *   carrying a query or a fragment; `storage/ambiguous-prefix` for a job
 *   vended more than one prefix; `api/invalid-argument` when no `engine` is given; `engine/failed`
 *   when the engine refuses a view; for a job, what `mount` throws — `storage/host-silent` for a host
 *   that does not answer within 30 s among them.
 */
export function open(source: string, options: OpenOptions & { sql: 'allowed' }): Promise<SqlCorpus>;
export function open(source: string, options: OpenOptions): Promise<Corpus>;
export async function open(source: string, options: OpenOptions): Promise<Corpus> {
  const { engine, host, signal } = options;
  if (typeof engine?.query !== 'function') {
    throw FossilError.of(
      'api/invalid-argument',
      { argument: 'engine', expected: 'an Engine' },
      { help: "open() needs { engine } for a corpus at a URL, and { engine, host } for a job's" },
    );
  }

  // What SQL calls each file of the corpus, and what closing gives back. A job's files are lent
  // under the host's credential — an Azure file one by one — and a URL's are named as they are.
  let names: (paths: readonly string[]) => Promise<string[]>;
  let release = async (): Promise<void> => {};
  let storage: Mount | undefined;
  if (host !== undefined) {
    const mounted = await mount(engine, host, { job: source }, 'read', { signal });
    storage = mounted;
    release = () => mounted.close();
    if (mounted.prefixes.length !== 1) {
      const ambiguous = FossilError.of(
        'storage/ambiguous-prefix',
        { scope: `job ${source}`, count: mounted.prefixes.length },
      );
      try {
        await release();
      } catch (cleanup) {
        throw attachCause(ambiguous, cleanup);
      }
      throw ambiguous;
    }
    const prefix = mounted.prefixes[0]!;
    names = (paths) => mounted.files(paths.map((path) => `${prefix}${path}`));
  } else {
    // A corpus is a prefix and its files are named under it, so a query — a signature among them —
    // would end up in the middle of every path. Signed storage is a job under its host.
    if (/[?#]/.test(source)) {
      throw FossilError.of(
        'corpus/not-a-location',
        { location: source },
      );
    }
    const base = source.endsWith('/') ? source : `${source}/`;
    names = async (paths) => paths.map((path) => `${base}${path}`);
  }

  let counted = false;
  const held = holders.get(engine) ?? new Map<string, number>();
  holders.set(engine, held);
  const catalog = source;
  try {
    const [where] = await names([ENTRY_POINT]);
    const unreadable = (cause?: unknown) =>
      FossilError.of('corpus/unreadable', { path: where! }, cause === undefined ? {} : { cause });
    let answer: Awaited<ReturnType<Engine['query']>>;
    try {
      answer = await engine.query(`SELECT content FROM read_text(${lit(where!)})`, {
        signal: signal ?? new AbortController().signal,
      });
    } catch (cause) {
      signal?.throwIfAborted();
      throw unreadable(cause);
    }
    if (answer.numRows === 0) throw unreadable();
    const text = String(answer.getChild('content')?.get(0));
    const manifest = parseManifest(text, where!);

    const tables = new Map<string, VertexTable | EdgeTable>();
    for (const table of [...manifest.vertex_tables, ...manifest.edge_tables]) {
      if (tables.has(table.name)) {
        throw FossilError.of('corpus/duplicate-table', { table: table.name });
      }
      tables.set(table.name, table);
    }
    const files = await names([...tables.values()].map((t) => t.path));

    const relation = (table: string): string => `${ident(catalog)}.${ident(table)}`;
    held.set(catalog, (held.get(catalog) ?? 0) + 1);
    counted = true;
    await query(engine, `ATTACH IF NOT EXISTS ':memory:' AS ${ident(catalog)}`, signal);
    let at = 0;
    for (const name of tables.keys()) {
      await query(
        engine,
        `CREATE OR REPLACE VIEW ${relation(name)} AS SELECT * FROM read_parquet(${lit(files[at++]!)})`,
        signal,
      );
    }

    // A credential whose renewal failed past expiry makes every read a 403; the read reports the
    // storage failure instead, which names what to fix.
    const live = (): void => {
      if (storage?.failure !== undefined) throw storage.failure;
    };
    let closed = false;
    const corpus: Corpus = {
      url: catalog,
      schema: SCHEMA,
      manifest,
      scan: scanOf(engine, relation, tables, live),
      relation(table) {
        if (!tables.has(table)) {
          throw FossilError.of('corpus/unknown-table', { table, tables: [...tables.keys()] });
        }
        return relation(table);
      },
      async close() {
        if (closed) return;
        closed = true;
        await detached(release);
      },
    };
    if (options.sql !== 'allowed') return corpus;
    const widened: SqlCorpus = {
      ...corpus,
      sql: (statement, o) => {
        live();
        return sql(engine, statement, o);
      },
    };
    return widened;
  } catch (cause) {
    try {
      if (counted) await detached(release);
      else await release();
    } catch (cleanup) {
      throw attachCause(cause, cleanup);
    }
    throw cause;
  }

  /**
   * Give this open's hold on the catalog back — the last holder detaches it — and then `after`, which
   * runs whether or not the detach did. Two failures are one: the detach's, the release's attached.
   */
  async function detached(after: () => Promise<void>): Promise<void> {
    const left = (held.get(catalog) ?? 1) - 1;
    let failure: unknown;
    if (left > 0) held.set(catalog, left);
    else {
      held.delete(catalog);
      try {
        await query(engine, `DETACH DATABASE IF EXISTS ${ident(catalog)}`);
      } catch (cause) {
        failure = cause;
      }
    }
    try {
      await after();
    } catch (cause) {
      if (failure === undefined) throw cause;
      attachCause(failure, cause);
    }
    if (failure !== undefined) throw failure;
  }
}

async function sql(
  engine: Engine,
  statement: string,
  { limit = SQL_LIMIT, signal }: { readonly limit?: number; readonly signal?: AbortSignal } = {},
): Promise<SqlResult> {
  if (!(Number.isSafeInteger(limit) && limit >= 0)) throw notACount(limit);
  const body = statement.trim().replace(/;+\s*$/, '');
  // The cap is an outer LIMIT whatever the statement says, and one row past it says it bit.
  const wrapped = `SELECT * FROM (${body}) AS _q LIMIT ${limit + 1}`;
  signal?.throwIfAborted();
  const table = await query(engine, wrapped, signal);
  signal?.throwIfAborted();
  const columns = table.schema.fields.map((f) => f.name);
  const children = columns.map((name) => table.getChild(name));
  const n = Math.min(table.numRows, limit);
  const rows = Array.from({ length: n }, (_, i) => children.map((c) => c?.get(i) ?? null));
  return { columns, rows, truncated: table.numRows > limit };
}
