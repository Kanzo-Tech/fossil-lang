/**
 * The door: a corpus attached to the host's engine under a name — DuckDB's `ATTACH … AS name` — and
 * the handle that detaches it.
 */

import { mount } from '@fossil-lang/storage';
import { FossilError, type CorpusFile, type Engine, type Host } from '@fossil-lang/types';
import { attachCause } from '@fossil-lang/types/internal';

import type { Annotation, Manifest, TopValue } from './manifest.gen.js';
import { parseManifest, tablesIn } from './manifest.js';
import { ident, lit, query } from './sql.js';
import { triplesOf } from './triples.js';

/** The file every attach reads first. */
const ENTRY_POINT = 'fossil.json';

/**
 * The relations the manifest becomes, beside the tables' views, and the SQL of each — given the
 * manifest, and the name of a table's view.
 */
const CATALOG_VIEWS: Readonly<Record<string, (manifest: Manifest, relation: (table: string) => string) => string>> = {
  fossil_tables: tablesOf,
  fossil_columns: columnsOf,
  fossil_annotations: annotationsOf,
  fossil_stats: statsOf,
  fossil_inclusions: inclusionsOf,
  triples: triplesOf,
};

/** How many corpora from memory have been attached in this realm: each takes a prefix of its own. */
let fromMemory = 0;

/**
 * What {@link attach} takes: the engine, and where the corpus is — a job under its host, a URL, or
 * the files a run answered in memory. Exactly one of the three.
 */
export interface AttachOptions {
  /**
   * The page's engine — `@fossil-lang/types`' `Engine`, DuckDB-WASM in a browser. `fossil.json` is
   * read through it and the views are created in it; every later read is the host's own SQL.
   */
  engine: Engine;
  /**
   * What vends the job's credential. With it, the name {@link attach} is given is the job: the corpus
   * is the one prefix the host vends `read` on for `{ job }`, kept fresh by `@fossil-lang/storage`
   * until the corpus is detached.
   */
  host?: Host;
  /** A corpus at a URL, for a caller with no host — public, or the local filesystem in Node. */
  url?: string;
  /**
   * A corpus held in memory — what `run(…, { into: 'memory' })` answers. Each file is registered with
   * the engine under a prefix of this attachment's own, so a second attach under the same name does
   * not take the files of the one it replaces; detaching gives them back. The engine copies the
   * bytes, so the same `files` can be attached twice.
   */
  files?: readonly CorpusFile[];
  /**
   * Stops the attach: it rejects with the signal's reason, and what it had made — a mount, a catalog —
   * is given back. It reaches the host's credential request and every statement the attach runs.
   */
  signal?: AbortSignal;
}

/**
 * A corpus attached by {@link attach}: the catalog it is attached as, and how to give it back —
 * `detach()`, or `await using`, which calls it.
 */
export interface Attachment {
  /** The catalog: `"<name>".fossil_tables`. */
  readonly name: string;
  /**
   * Gives back what {@link attach} took: the catalog, once no other attachment of the name holds it,
   * and the credential. A second call does nothing.
   */
  detach(): Promise<void>;
  /** {@link Attachment.detach}, for `await using`. */
  [Symbol.asyncDispose](): Promise<void>;
}

/**
 * How many attachments hold each catalog, per engine: two attaches of one name share its views, and
 * the first to detach must not take them from the second.
 */
const holders = new WeakMap<object, Map<string, number>>();

/**
 * **Attach a corpus to the engine as the catalog `name`**: a view per table — `"<name>"."Person"` —
 * every fact of the manifest as a relation, and the corpus as RDF, `"<name>".triples`. Read it with
 * SQL — Mosaic's, the host's — and detach what comes back.
 *
 * ```ts
 * await using corpus = await attach(job, { engine, host });   // a job's corpus, under its credential
 * const corpus = await attach('demo', { engine, url });       // a corpus at a URL
 * const preview = await attach('preview', { engine, files }); // what a run answered in memory
 * await corpus.detach();
 * ```
 *
 * `fossil_tables(table_name, kind, label, iri, path, record_count, first_id, source, source_key,
 * destination, destination_key, candidate_keys, derived_from)` — one row per table, in manifest
 * order; `kind` is `vertex`, `edge` or `property`; `label` is its SQL/PGQ label — a vertex table's
 * name, an edge table's label, a property table's property; a vertex table's `dense_id`s are
 * `first_id … first_id + record_count − 1`; `source`/`destination` name the vertex tables an edge
 * table's ends point into, and `source_key`/`destination_key` the columns holding them — a property
 * table has a source only; `candidate_keys` are the columns never null and never repeated, as the
 * writer counted them; `derived_from` is the sources the table was derived from, each as the program
 * wrote it. `fossil_columns(table_name, column_name, ordinal_position, data_type, role, iri,
 * term_type, datatype, is_nullable)` — one row per column, `role` the writer's (`address`,
 * `identity`, `endpoint`, `language`) and null on a program's column, `term_type` R2RML's `rr:IRI` or
 * `rr:Literal` and `datatype` the literal datatype IRI its shape declared (`xsd:gYear` over an
 * `int32`), each null where it declared none. `fossil_annotations(table_name, column_name, predicate,
 * o_type, o_value, o_datatype, o_lang)` — a row per annotation, `column_name` null for a table's own.
 * `fossil_stats(table_name, column_name, null_count, distinct_count, min_value, max_value,
 * top_values)` — the writer's counts per column. `fossil_inclusions(table_name, column_name,
 * referenced_table_name, referenced_column_name)` — a column whose every value is a value of another
 * table's key. `triples(s_type, s_value, p, o_type, o_value, o_datatype, o_lang)` — one row per RDF
 * triple, each term as its type (`I` or `L`), its IRI or lexical form, its datatype and language
 * (`/docs/format/reading/rdf`): what a SHACL engine validates. `/docs/design/facts` says why each is
 * there.
 *
 * @throws {FossilError} before any Parquet is read: `corpus/unreadable` when `fossil.json` does not
 *   read, or a file `files` does not hold; `corpus/not-json`, `corpus/unsupported-format` for a
 *   format other than `fossil/1`, `corpus/duplicate-table` when it names one table twice or a table
 *   like one of the relations above; `corpus/not-a-location` for a URL carrying a query or a
 *   fragment; `storage/ambiguous-prefix` for a job vended more than one prefix;
 *   `api/invalid-argument` without an `engine`, or without exactly one of `host`, `url` and `files`;
 *   `engine/failed` when the engine refuses a view; for a job, what `mount` throws —
 *   `storage/host-silent` among them.
 */
export async function attach(name: string, options: AttachOptions): Promise<Attachment> {
  const { engine, host, url, signal } = options;
  if (typeof engine?.query !== 'function') {
    throw FossilError.of(
      'api/invalid-argument',
      { argument: 'engine', expected: 'an Engine' },
      { help: 'attach() needs { engine } and one of { host } for a job, { url } for a corpus at a URL or { files } for one in memory' },
    );
  }
  const { files } = options;
  if ([host, url, files].filter((where) => where !== undefined).length !== 1) {
    throw FossilError.of('api/invalid-argument', { argument: 'host', expected: 'exactly one of host, url and files' });
  }

  // What SQL calls each file of the corpus, and what detaching gives back. A job's files are lent
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
  } else if (files !== undefined) {
    // Named under a prefix of this attachment's own, registered as they are named; a failed attach
    // drops what it registered, and the attach's failure is the one reported.
    const base = `fossil-memory/${++fromMemory}/`;
    const registered: string[] = [];
    release = () => engine.dropFiles(registered.splice(0));
    const held = new Map(files.map((f) => [f.path, f.bytes]));
    names = async (paths) => {
      const out: string[] = [];
      for (const path of paths) {
        const bytes = held.get(path);
        if (bytes === undefined) throw FossilError.of('corpus/unreadable', { path });
        const name = `${base}${path}`;
        if (!registered.includes(name)) {
          await engine.registerFileBuffer(name, bytes);
          registered.push(name);
        }
        out.push(name);
      }
      return out;
    };
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
      await query(engine, `CREATE OR REPLACE VIEW ${relation(view)} AS ${sql(manifest, relation)}`, signal);
    }

    let detachedOnce = false;
    const detach = async (): Promise<void> => {
      if (detachedOnce) return;
      detachedOnce = true;
      await detached(release);
    };
    return { name, detach, [Symbol.asyncDispose]: detach };
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
   * Give this attachment's hold on the catalog back — the last holder detaches it — and then `after`, which
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

/** A `VARCHAR[]` literal of the values, `[]` for none. */
const list = (values: readonly string[] | undefined): string => `[${(values ?? []).map(lit).join(', ')}]::VARCHAR[]`;

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
      const row = [
        lit(t.name), `'vertex'`, lit(t.name), opt(t.iri), lit(t.path), String(t.record_count), String(first),
        'NULL', 'NULL', 'NULL', 'NULL', list(t.candidate_keys), list(t.derived_from),
      ];
      first += t.record_count;
      return row;
    }),
    ...manifest.edge_tables.map((t) => [
      lit(t.name), `'edge'`, lit(t.label), opt(t.iri), lit(t.path), String(t.record_count), 'NULL',
      lit(t.source.references), lit(t.source.key), lit(t.destination.references), lit(t.destination.key),
      list(t.candidate_keys), list(t.derived_from),
    ]),
    ...(manifest.property_tables ?? []).map((t) => [
      // A property table's label is its property: the name of its value column, the one column with
      // no role.
      lit(t.name), `'property'`, opt(t.properties.find((p) => p.role === undefined)?.name), 'NULL', lit(t.path),
      String(t.record_count), 'NULL', lit(t.source.references), lit(t.source.key), 'NULL', 'NULL',
      list(t.candidate_keys), list(t.derived_from),
    ]),
  ];
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['kind', 'VARCHAR'], ['label', 'VARCHAR'], ['iri', 'VARCHAR'], ['path', 'VARCHAR'],
      ['record_count', 'UBIGINT'], ['first_id', 'UBIGINT'], ['source', 'VARCHAR'], ['source_key', 'VARCHAR'],
      ['destination', 'VARCHAR'], ['destination_key', 'VARCHAR'], ['candidate_keys', 'VARCHAR[]'],
      ['derived_from', 'VARCHAR[]'],
    ],
    rows,
  );
}

/** `fossil_columns`: one row per column of every table, in file order. */
function columnsOf(manifest: Manifest): string {
  const rows = tablesIn(manifest).flatMap((t) =>
    t.properties.map((p, at) => [
      lit(t.name), lit(p.name), String(at + 1), lit(p.type), opt(p.role), opt(p.iri), opt(p.term_type), opt(p.datatype),
      p.nullable === true ? 'true' : 'false',
    ]),
  );
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['column_name', 'VARCHAR'], ['ordinal_position', 'INTEGER'], ['data_type', 'VARCHAR'],
      ['role', 'VARCHAR'], ['iri', 'VARCHAR'], ['term_type', 'VARCHAR'], ['datatype', 'VARCHAR'], ['is_nullable', 'BOOLEAN'],
    ],
    rows,
  );
}

const XSD_STRING = 'http://www.w3.org/2001/XMLSchema#string';
const RDF_LANG_STRING = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#langString';

/** An annotation's object as the four columns `triples` spells a term in. */
function term(object: Annotation['object']): string[] {
  if (typeof object === 'string') return [`'I'`, lit(object), `''`, `''`];
  const datatype = object.language !== undefined ? RDF_LANG_STRING : (object.datatype ?? XSD_STRING);
  return [`'L'`, lit(object.value), lit(datatype), lit(object.language ?? '')];
}

/**
 * `fossil_annotations`: one row per annotation of a table (`column_name` null) or of a column — ShEx's
 * `Annotation`, the object as `triples` spells a term. They are about the schema and make no triple.
 */
function annotationsOf(manifest: Manifest): string {
  const rows = [
    ...[...manifest.vertex_tables, ...manifest.edge_tables].flatMap((t) =>
      (t.annotations ?? []).map((a) => [lit(t.name), 'NULL', lit(a.predicate), ...term(a.object)]),
    ),
    ...tablesIn(manifest).flatMap((t) =>
      t.properties.flatMap((p) => (p.annotations ?? []).map((a) => [lit(t.name), lit(p.name), lit(a.predicate), ...term(a.object)])),
    ),
  ];
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['column_name', 'VARCHAR'], ['predicate', 'VARCHAR'], ['o_type', 'VARCHAR'],
      ['o_value', 'VARCHAR'], ['o_datatype', 'VARCHAR'], ['o_lang', 'VARCHAR'],
    ],
    rows,
  );
}

/** A `top_values` list as a SQL list of structs. */
const tops = (values: readonly TopValue[] | undefined): string =>
  `[${(values ?? []).map((v) => `{'value': ${lit(v.value)}, 'count': ${v.count}::UBIGINT}`).join(', ')}]::STRUCT(value VARCHAR, count UBIGINT)[]`;

/**
 * `fossil_stats`: one row per column of every table, the writer's counts over every row it wrote. A
 * corpus written before the writer counted has a row of nulls per column.
 */
function statsOf(manifest: Manifest): string {
  const rows = tablesIn(manifest).flatMap((t) =>
    t.properties.map((p) => {
      const s = p.stats;
      return [
        lit(t.name), lit(p.name),
        s === undefined ? 'NULL' : String(s.null_count),
        s === undefined ? 'NULL' : String(s.distinct_count),
        opt(s?.min_value), opt(s?.max_value),
        s === undefined ? 'NULL' : tops(s.top_values),
      ];
    }),
  );
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['column_name', 'VARCHAR'], ['null_count', 'UBIGINT'], ['distinct_count', 'UBIGINT'],
      ['min_value', 'VARCHAR'], ['max_value', 'VARCHAR'], ['top_values', 'STRUCT(value VARCHAR, count UBIGINT)[]'],
    ],
    rows,
  );
}

/** `fossil_inclusions`: every program column whose values are all values of another table's key. */
function inclusionsOf(manifest: Manifest): string {
  const rows = (manifest.inclusions ?? []).map((i) => [lit(i.from.table), lit(i.from.column), lit(i.to.table), lit(i.to.column)]);
  return relationOf(
    [
      ['table_name', 'VARCHAR'], ['column_name', 'VARCHAR'], ['referenced_table_name', 'VARCHAR'],
      ['referenced_column_name', 'VARCHAR'],
    ],
    rows,
  );
}
