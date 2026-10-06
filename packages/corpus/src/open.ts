/**
 * The door: a corpus attached to the host's engine under a name, and the function that detaches it.
 */

import { mount } from '@fossil-lang/storage';
import { FossilError, attachCause, type Engine, type Host } from '@fossil-lang/types';

import type { Manifest } from './manifest.gen.js';
import { parseManifest, tablesIn } from './manifest.js';
import { ident, lit, query } from './sql.js';

/** The file every open reads first. */
const ENTRY_POINT = 'fossil.json';

/** The relations the manifest becomes, beside the tables' views, and the SQL of each. */
const CATALOG_VIEWS: Readonly<Record<string, (manifest: Manifest) => string>> = {
  fossil_tables: tablesOf,
  fossil_columns: columnsOf,
};

/** What {@link open} takes: the engine, and where the corpus is — a job under its host, or a URL. */
export interface OpenOptions {
  /**
   * The page's engine — `@fossil-lang/types`' `Engine`, DuckDB-WASM in a browser. `fossil.json` is
   * read through it and the views are created in it; every later read is the host's own SQL.
   */
  engine: Engine;
  /**
   * What vends the job's credential. With it, the name {@link open} is given is the job: the corpus
   * is the one prefix the host vends `read` on for `{ job }`, kept fresh by `@fossil-lang/storage`
   * until the corpus is closed.
   */
  host?: Host;
  /** A corpus at a URL, for a caller with no host — public, or the local filesystem in Node. */
  url?: string;
  /**
   * Stops the open: it rejects with the signal's reason, and what it had made — a mount, a catalog —
   * is given back. It reaches the host's credential request and every statement the open runs.
   */
  signal?: AbortSignal;
}

/** Gives back what {@link open} took: the catalog, once no other open of the name holds it, and the credential. */
export type Close = () => Promise<void>;

/**
 * How many open corpora hold each catalog, per engine: two opens of one name share its views, and
 * the first to close must not take them from the second.
 */
const holders = new WeakMap<object, Map<string, number>>();

/**
 * **Attach a corpus to the engine as the catalog `name`**: a view per table — `"<name>"."Person"` —
 * and the manifest as two relations, `"<name>".fossil_tables` and `"<name>".fossil_columns`. Read it
 * with SQL — Mosaic's, the host's — and call what comes back to detach it.
 *
 * ```ts
 * const close = await open(job, { engine, host });          // a job's corpus, under its credential
 * const close = await open('demo', { engine, url });        // a corpus at a URL
 * ```
 *
 * `fossil_tables(table_name, kind, iri, path, rows, first_id, source, destination)` — one row per
 * table, in manifest order; `kind` is `vertex`, `edge` or `property`; `path` is its file under the
 * corpus root; a vertex table's `dense_id`s are `first_id … first_id + rows − 1`; an edge table's
 * `source`/`destination` name the vertex tables its `src`/`dst` point into, and a property table's
 * `source` the one its `src` does — a row per value of a multi-valued property, whose IRI is its
 * value column's. `fossil_columns(table_name, column_name, ordinal, type, role, iri,
 * nullable)` — one row per column, `role` the writer's (`address`, `identity`, `endpoint`) and null
 * on a program's column.
 *
 * @throws {FossilError} before any Parquet is read: `corpus/unreadable` when `fossil.json` does not
 *   read, `corpus/not-json`, `corpus/unsupported-format` for a format other than `fossil/1`,
 *   `corpus/duplicate-table` when it names one table twice or a table `fossil_tables` /
 *   `fossil_columns`; `corpus/not-a-location` for a URL carrying a query or a fragment;
 *   `storage/ambiguous-prefix` for a job vended more than one prefix; `api/invalid-argument` without
 *   an `engine`, or without exactly one of `host` and `url`; `engine/failed` when the engine refuses
 *   a view; for a job, what `mount` throws — `storage/host-silent` among them.
 */
export async function open(name: string, options: OpenOptions): Promise<Close> {
  const { engine, host, url, signal } = options;
  if (typeof engine?.query !== 'function') {
    throw FossilError.of(
      'api/invalid-argument',
      { argument: 'engine', expected: 'an Engine' },
      { help: 'open() needs { engine } and either { host } for a job or { url } for a corpus at a URL' },
    );
  }
  if ((host === undefined) === (url === undefined)) {
    throw FossilError.of('api/invalid-argument', { argument: 'host', expected: 'exactly one of host and url' });
  }

  // What SQL calls each file of the corpus, and what closing gives back. A job's files are lent
  // under the host's credential — an Azure file one by one — and a URL's are named as they are.
  let names: (paths: readonly string[]) => Promise<string[]>;
  let release = async (): Promise<void> => {};
  if (host !== undefined) {
    const mounted = await mount(engine, host, { job: name }, 'read', { signal });
    release = () => mounted.close();
    if (mounted.prefixes.length !== 1) {
      const ambiguous = FossilError.of('storage/ambiguous-prefix', { scope: `job ${name}`, count: mounted.prefixes.length });
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
    if (/[?#]/.test(url!)) throw FossilError.of('corpus/not-a-location', { location: url! });
    const base = url!.endsWith('/') ? url! : `${url!}/`;
    names = async (paths) => paths.map((path) => `${base}${path}`);
  }

  let counted = false;
  const held = holders.get(engine) ?? new Map<string, number>();
  holders.set(engine, held);
  try {
    const [where] = await names([ENTRY_POINT]);
    const manifest = await read(engine, where!, signal);
    const tables = tablesIn(manifest);
    const seen = new Set(Object.keys(CATALOG_VIEWS));
    for (const table of tables) {
      if (seen.has(table.name)) throw FossilError.of('corpus/duplicate-table', { table: table.name });
      seen.add(table.name);
    }
    const files = await names(tables.map((t) => t.path));

    held.set(name, (held.get(name) ?? 0) + 1);
    counted = true;
    const relation = (table: string): string => `${ident(name)}.${ident(table)}`;
    await query(engine, `ATTACH IF NOT EXISTS ':memory:' AS ${ident(name)}`, signal);
    for (const [at, table] of tables.entries()) {
      await query(
        engine,
        `CREATE OR REPLACE VIEW ${relation(table.name)} AS SELECT * FROM read_parquet(${lit(files[at]!)})`,
        signal,
      );
    }
    for (const [view, sql] of Object.entries(CATALOG_VIEWS)) {
      await query(engine, `CREATE OR REPLACE VIEW ${relation(view)} AS ${sql(manifest)}`, signal);
    }

    let closed = false;
    return async () => {
      if (closed) return;
      closed = true;
      await detached(release);
    };
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
    const left = (held.get(name) ?? 1) - 1;
    let failure: unknown;
    if (left > 0) held.set(name, left);
    else {
      held.delete(name);
      try {
        await query(engine, `DETACH DATABASE IF EXISTS ${ident(name)}`);
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

/** `fossil.json` at `where`, through the engine, parsed. */
async function read(engine: Engine, where: string, signal?: AbortSignal): Promise<Manifest> {
  const unreadable = (cause?: unknown) =>
    FossilError.of('corpus/unreadable', { path: where }, cause === undefined ? {} : { cause });
  let answer: Awaited<ReturnType<Engine['query']>>;
  try {
    answer = await engine.query(`SELECT content FROM read_text(${lit(where)})`, {
      signal: signal ?? new AbortController().signal,
    });
  } catch (cause) {
    signal?.throwIfAborted();
    throw unreadable(cause);
  }
  if (answer.numRows === 0) throw unreadable();
  return parseManifest(String(answer.getChild('content')?.get(0)), where);
}

/** A SQL literal, or `NULL`. */
const opt = (value: string | undefined): string => (value === undefined ? 'NULL' : lit(value));

/**
 * The rows of a relation as a `SELECT`, its columns typed even when there are none — a `VALUES` with
 * no row is not SQL, and a column of nulls only would have no type.
 */
function relationOf(columns: readonly (readonly [string, string])[], rows: readonly (readonly string[])[]): string {
  const typed = columns.map(([column, type]) => `CAST(NULL AS ${type}) AS ${ident(column)}`).join(', ');
  const empty = `SELECT ${typed} WHERE false`;
  if (rows.length === 0) return empty;
  const values = rows.map((row) => `(${row.join(', ')})`).join(', ');
  return `${empty} UNION ALL BY NAME SELECT * FROM (VALUES ${values}) AS t(${columns.map(([c]) => ident(c)).join(', ')})`;
}

/** `fossil_tables`: one row per table, in manifest order. */
function tablesOf(manifest: Manifest): string {
  let first = 0;
  const rows = [
    ...manifest.vertex_tables.map((t) => {
      const row = [lit(t.name), `'vertex'`, opt(t.iri), lit(t.path), String(t.record_count), String(first), 'NULL', 'NULL'];
      first += t.record_count;
      return row;
    }),
    ...manifest.edge_tables.map((t) => [
      lit(t.name), `'edge'`, opt(t.iri), lit(t.path), String(t.record_count), 'NULL', lit(t.source.references), lit(t.destination.references),
    ]),
    ...(manifest.property_tables ?? []).map((t) => [
      lit(t.name), `'property'`, 'NULL', lit(t.path), String(t.record_count), 'NULL', lit(t.source.references), 'NULL',
    ]),
  ];
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['kind', 'VARCHAR'], ['iri', 'VARCHAR'], ['path', 'VARCHAR'], ['rows', 'UBIGINT'],
      ['first_id', 'UBIGINT'], ['source', 'VARCHAR'], ['destination', 'VARCHAR'],
    ],
    rows,
  );
}

/** `fossil_columns`: one row per column of every table, in file order. */
function columnsOf(manifest: Manifest): string {
  const rows = tablesIn(manifest).flatMap((t) =>
    t.properties.map((p, at) => [
      lit(t.name), lit(p.name), String(at + 1), lit(p.type), opt(p.role), opt(p.iri), p.nullable === true ? 'true' : 'false',
    ]),
  );
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['column_name', 'VARCHAR'], ['ordinal', 'INTEGER'], ['type', 'VARCHAR'],
      ['role', 'VARCHAR'], ['iri', 'VARCHAR'], ['nullable', 'BOOLEAN'],
    ],
    rows,
  );
}
