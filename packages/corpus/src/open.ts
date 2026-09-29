/**
 * The door — Iceberg's `load_table` for a corpus: the two shapes it is opened in, the manifest read,
 * the credential a job's corpus is lent under, the catalog's holders and `close`.
 */

import { addressManifests, CorpusManifestError, GRAPH_INFO_PATH } from './address.js';
import type { Corpus, CorpusField, CorpusTypes, SqlCorpus } from './corpus.js';
import { edgesOf } from './edges.js';
import { frameOf } from './frame.js';
import { IDENTITY, identityOf } from './identity.js';
import { initFossilGraphWasm, type InitInput } from './load.js';
import { join, paths, scan } from './manifest.js';
import type { QueryFn, QueryRow } from './query.js';
import { scanOf } from './scan.js';
import { CorpusReadError, ident, list, lit, text } from './sql.js';
import { tileManifestOf } from './tile-manifest.js';
import { verbsOf } from './verbs.js';
import { mount } from '@fossil-lang/storage';
import type { Engine, Host } from '@fossil-lang/types';

/**
 * Whether this corpus puts a caller's SQL in front of the engine.
 *
 * The default is `'withheld'`, and the asymmetry is deliberate: every other member costs a
 * function of the answer, and the escape hatch costs a function of whatever was typed. A host that
 * wants the hatch says so. `'withheld'` drops {@link SqlCorpus.executeSql} from the object;
 * `crates/fossil-graph/src/operations/raw_sql.rs` is the Rust half of the same permission.
 *
 * **What it does NOT claim.** It is not a sanitiser and it is not a security boundary: the engine
 * is the host's, the corpus is files the host already holds, and `/docs/design/privacy` is why a
 * read-time gate has no chokepoint to stand on. What this holds is the fact that a host must
 * write the word down.
 */
export type SqlPolicy = 'withheld' | 'allowed';

/**
 * What {@link open} takes.
 *
 * **An engine is required**: {@link engine} with {@link host} for a job's corpus, or {@link query}
 * for one at a URL. Every member of a {@link Corpus} needs bytes, so there is nothing to answer
 * without one. {@link manifestFiles} beside `query` only saves the door its own manifest reads.
 */
export interface OpenOptions {
  /**
   * The page's engine, for a job's corpus: `open(job, { engine, host })`.
   *
   * The first argument then names the job and not a URL. The corpus is the one prefix the host
   * vends `read` on for `{ job }`, and every file of it — manifests included — is read through the
   * engine under that credential, which `@fossil-lang/storage` renews before it expires for as
   * long as the corpus is open.
   */
  engine?: Engine;
  /** What vends the job's credential. Required with {@link engine}. */
  host?: Host;
  /**
   * The host's engine. One method, and see `./query.ts` for why it is the only one.
   */
  query?: QueryFn;
  /**
   * The manifest YAMLs, keyed by dataset-relative path, when the host already holds them — the
   * same shape the verbs take. They are small: one index plus one file per type.
   *
   * Given beside an engine, the door skips its own `1 + N` reads and opens against these.
   */
  manifestFiles?: Record<string, string>;
  /**
   * Whether this corpus admits raw SQL. Defaults to `'withheld'`. See {@link SqlPolicy}.
   *
   * `sql: 'allowed'` widens the return type to {@link SqlCorpus}, so the hatch is reachable
   * exactly where the host opened it and the type system carries the policy rather than a runtime
   * check the caller has to remember.
   */
  sql?: SqlPolicy;
  /**
   * The module's `.wasm`, for a host with no bundler — and only for one.
   *
   * **The boot is not a step a consumer sequences.** `open` awaits it, memoised for the process,
   * and with this omitted the module resolves `new URL('fossil_graph_wasm_bg.wasm',
   * import.meta.url)` — the pattern Vite, webpack 5 and Turbopack emit as an asset, so a bundled
   * host names nothing and copies nothing. Node is the host that needs it: its `fetch` rejects
   * `file://`, so a script or a test hands the bytes (`BufferSource`) or a `Response`.
   */
  wasm?: InitInput;
}

/**
 * Open a corpus from its URL.
 *
 * One argument is the corpus and the other is the engine it is read through:
 *
 * ```ts
 * await open(job, { engine, host })          // a job's corpus, under the credential host vends
 * await open(url, { query })                 // a corpus at a URL, 1 + N round trips
 * await open(url, { query, manifestFiles })  // the same, with the manifests already in hand
 * ```
 *
 * **This absorbed `resolveCorpus`, the last of the entry points over one manifest.**
 * `createGraphClient` went first (it is the transport), the `./address` subpath went second (its
 * one justification was a WASM-free closure, and keeping it meant keeping a second implementation
 * of `fossil_graph::plan`), and the engine-free rungs that replaced `resolveCorpus` went last:
 * their consumers were a viewer this repository no longer has, and the one viewer there is opens
 * with an engine and reads {@link Corpus.addressing}.
 *
 * Everything else — which files exist, how many tiles there are, what a row carries — is read from
 * the artefact:
 *
 * 1. `graph.graph.yml` names the per-type manifests, and they are fetched with `read_text` through
 *    the same `query` the payload goes through. There is no separate `fetch` capability because
 *    there is nothing a separate one could reach that the engine cannot: it has to see the tiles.
 * 2. `vertex_count` and `chunk_size` give the tile set by arithmetic. Before those became required
 *    fields there was no way to know it — HTTP has no directory listing, and the alternative
 *    written down at the time was to probe with `HEAD` until a 404.
 * 3. One `DESCRIBE` per vertex type gives the payload vocabulary.
 *
 * **Why step 3 is not read off the manifest.** The payload projection's `properties` carry names
 * *and* types, so this looked free. It is not: on the conformance corpus the manifest declares
 * **three** properties (`subject`, `birth_year`, `postcode`) against **seven** columns on disk —
 * those three plus `dense_id`, `x`, `y` and `cluster_id`, the four the writer puts there and the
 * vocabulary does not name — and
 * `packages/corpus`'s own test fixture declares three of which one is `dense_id`. The manifest's
 * property list is a promise; the payload is the artefact, and this reads the artefact. The cost is
 * one round trip per vertex type at open.
 *
 * **Which container, and it is read off the manifest.** A tile is a range of rows; whether it is a
 * file (`chunk{k}.parquet`) or a row group inside one file (`tiles.parquet`) is a second question,
 * and `graph.graph.yml`'s `container` is what answers it — a reader over HTTP has no directory to
 * list, so this is not something to work out. Both are read here and neither is globbed: a reader
 * that globbed would pick up a staged single-file copy beside the tiles and count every row twice.
 * The row-group one is the measured winner (5.6 requests per window against 22.3, and 496 kB of
 * footer against 1.15 MB, at five million vertices) and it is the container fossil does not write
 * yet, which is why both are read and not one.
 *
 * **Raw SQL is withheld unless the host asks for it.** `sql: 'allowed'` widens the answer to
 * {@link SqlCorpus} and admits {@link Corpus.read}'s `where` with it; see {@link SqlPolicy} for
 * why one option decides both.
 *
 * @throws {CorpusManifestError} when the manifest cannot address itself, or declares no row count.
 * @throws {TypeError} when neither `engine` nor `query` is given — there is then nothing to read
 *   the corpus with.
 */
export function open(
  job: string,
  options: OpenOptions & { engine: Engine; host: Host; sql: 'allowed' },
): Promise<SqlCorpus>;
export function open(
  job: string,
  options: OpenOptions & { engine: Engine; host: Host },
): Promise<Corpus>;
export function open(
  url: string,
  options: OpenOptions & { query: QueryFn; sql: 'allowed' },
): Promise<SqlCorpus>;
export function open(
  url: string,
  options: OpenOptions & { query: QueryFn },
): Promise<Corpus>;
export async function open(url: string, options: OpenOptions): Promise<Corpus> {
  if (options.engine !== undefined) return vended(url, options, options.engine);
  const { query, manifestFiles: held } = options;
  if (typeof query !== 'function') {
    throw new TypeError(
      'open() needs an engine: engine with host (a job\'s corpus) or query (a corpus at a URL)',
    );
  }
  // Before anything is resolved, because resolving is what needs it: the addressing is
  // `fossil_graph::plan` behind this module, not a second implementation of it on this side. The
  // boot is memoised, so a second corpus in the same process costs the check and nothing else.
  await initFossilGraphWasm(options.wasm);

  // The manifests, in ONE round trip because `read_text` takes a list. Which files is not the
  // caller's to know: that is `GRAPH_INFO_PATH` and the index's own two lists.
  const readManifests = async (relative: readonly string[]): Promise<Record<string, string>> => {
    if (relative.length === 0) return {};
    const urls = relative.map((path) => join(url, path));
    const rows = await query(`SELECT filename, content FROM read_text(${list(urls)})`);
    const byUrl = new Map(rows.map((row) => [text(row, 'filename'), text(row, 'content')]));
    const out: Record<string, string> = {};
    for (const [index, path] of relative.entries()) {
      const content = byUrl.get(urls[index]!);
      if (content === undefined) {
        throw new CorpusManifestError(`${urls[index]!} is named by the manifest and did not read`);
      }
      out[path] = content;
    }
    return out;
  };

  let manifestFiles: Record<string, string>;
  if (held !== undefined) {
    manifestFiles = held;
  } else {
    manifestFiles = await readManifests([GRAPH_INFO_PATH]);
    const index = scan(GRAPH_INFO_PATH, manifestFiles[GRAPH_INFO_PATH]!);
    Object.assign(
      manifestFiles,
      await readManifests([...paths(index, 'vertices'), ...paths(index, 'edges')]),
    );
  }

  return opened(url, options, manifestFiles, query);
}

/**
 * How many open corpora hold each catalog, per engine — the one piece of state shared between
 * corpora, because two opens of one name share its files and views and the first to close must
 * not take them from the second.
 */
const holders = new WeakMap<object, Map<string, number>>();

/** A job's corpus: the one prefix the host vends `read` on, read through the engine. */
async function vended(job: string, options: OpenOptions, engine: Engine): Promise<Corpus> {
  const host = options.host;
  if (host === undefined) {
    throw new TypeError('open(job, { engine }) needs host: a corpus is read with what it vends');
  }
  await initFossilGraphWasm(options.wasm);
  const storage = await mount(engine, host, { job }, 'read');
  try {
    if (storage.prefixes.length !== 1) {
      throw new CorpusManifestError(
        `job ${job} vends ${storage.prefixes.length} prefixes, and a corpus lives under one`,
      );
    }
    const prefix = storage.prefixes[0]!;
    const query: QueryFn = (sql) => engine.query(sql);
    const read = async (relative: readonly string[]): Promise<Record<string, string>> => {
      if (relative.length === 0) return {};
      const names = await storage.files(relative.map((path) => `${prefix}${path}`));
      const rows = await query(`SELECT filename, content FROM read_text(${list(names)})`);
      const byName = new Map(rows.map((row) => [text(row, 'filename'), text(row, 'content')]));
      return Object.fromEntries(
        relative.map((path, i) => {
          const content = byName.get(names[i]!);
          if (content === undefined) {
            throw new CorpusManifestError(`${prefix}${path} is named by the manifest and did not read`);
          }
          return [path, content];
        }),
      );
    };
    const manifestFiles = options.manifestFiles ?? (await read([GRAPH_INFO_PATH]));
    if (options.manifestFiles === undefined) {
      const index = scan(GRAPH_INFO_PATH, manifestFiles[GRAPH_INFO_PATH]!);
      Object.assign(manifestFiles, await read([...paths(index, 'vertices'), ...paths(index, 'edges')]));
    }
    // An Azure file is readable only once lent; an S3 one is named as it is.
    const base = storage.name(prefix);
    const files = addressManifests(manifestFiles, base).files();
    await storage.files(files.map((file) => `${prefix}${file.slice(base.length)}`));
    return await opened(base, options, manifestFiles, query, {
      holder: engine,
      drop: () => storage.close(),
    });
  } catch (cause) {
    await storage.close();
    throw cause;
  }
}

/** What {@link vended} hands {@link opened} so that `close` can give it back. */
interface Holding {
  readonly holder: object;
  readonly drop?: () => Promise<void>;
}

async function opened(
  url: string,
  options: OpenOptions,
  manifestFiles: Record<string, string>,
  query: QueryFn,
  holding?: Holding,
): Promise<Corpus> {
  // The policy, read once. Both consequences come off this one binding — the hatch below and
  // `read`'s predicate — so there is no way to wire half of it. See `SqlPolicy`.
  const rawSql = options.sql === 'allowed';
  const addressing = addressManifests(manifestFiles, url);

  // The catalog the verbs' views live in, named after the corpus: a database of its own, so two
  // corpora with a `Person` each never resolve to each other's, and closing is one `DETACH`.
  const catalog = url;
  const holder: object = holding?.holder ?? query;
  const held = holders.get(holder) ?? new Map<string, number>();
  holders.set(holder, held);
  held.set(catalog, (held.get(catalog) ?? 0) + 1);
  let closed = false;

  // Every vertex type's payload files, derived once. `files()` throws when the manifest declares no
  // count, which is the corpus this API cannot open and the addressing layer still can.
  const payloadFiles = new Map<string, readonly string[]>();
  const columns = new Map<string, readonly CorpusField[]>();
  for (const type of addressing.types) {
    payloadFiles.set(type.type, type.files());
    const first = payloadFiles.get(type.type)![0];
    let described: QueryRow[] = [];
    if (first !== undefined) {
      try {
        described = await query(`DESCRIBE SELECT * FROM read_parquet(${lit(first)})`);
      } catch (cause) {
        // This does not diagnose the failure — a truncated corpus reaches here too — it states what
        // was addressed and why nothing else was tried, because the tempting recovery is a glob and
        // a glob is wrong: it would pick up a staged single-file copy beside the tiles and count
        // every row twice.
        throw new CorpusReadError(
          `${first} is the first payload file of ${type.type} and it did not open. The manifest ` +
            `declares the ${addressing.container} container, so that is what was addressed, and ` +
            `this neither falls back to the other nor globs. (${(cause as Error).message})`,
        );
      }
    }
    columns.set(
      type.type,
      described.map((row) => ({
        name: text(row, 'column_name'),
        type: text(row, 'column_type'),
      })),
    );
  }

  const fieldsOf = (type: string): readonly CorpusField[] => columns.get(type) ?? [];
  const has = (type: string, column: string): boolean =>
    fieldsOf(type).some((f) => f.name === column);

  const types: CorpusTypes = {
    vertices: addressing.types.map((type) => ({
      type: type.type,
      // `files()` above threw if this were absent: a corpus whose extent is not derivable is the
      // one this API refuses to open, and it is also the only thing the manifest's count is for.
      count: type.count!,
      fields: fieldsOf(type.type),
      identity: has(type.type, IDENTITY) ? IDENTITY : null,
      geometry: has(type.type, 'x') && has(type.type, 'y'),
      indexed: type.index !== null,
    })),
    edges: addressing.edges.map((edge) => ({
      edgeType: edge.edgeType,
      srcType: edge.srcType,
      dstType: edge.dstType,
      count: edge.count,
      directions: edge.directions,
    })),
  };

  const manifest = await tileManifestOf({ query, addressing, payloadFiles, has });
  const identity = identityOf({ query, addressing, payloadFiles, has });
  const edges = edgesOf({ query, addressing, identity });
  const scanner = scanOf({ query, addressing, payloadFiles, fieldsOf, has, manifest, edges });
  const verbs = verbsOf({ query, addressing, manifestFiles, catalog, payloadFiles, fieldsOf });

  const corpus: Corpus = {
    url,
    types,
    addressing,

    async close() {
      if (closed) return;
      closed = true;
      const left = (held.get(catalog) ?? 1) - 1;
      if (left > 0) {
        held.set(catalog, left);
      } else {
        held.delete(catalog);
        await query(`DETACH DATABASE IF EXISTS ${ident(catalog)}`);
      }
      await holding?.drop?.();
    },
    schema: verbs.schema,
    relations: verbs.relations,
    extent: manifest.extent,
    rows: scanner.rows,
    frame: frameOf({ query, addressing, fieldsOf, has, manifest, scan: scanner }),
    node: identity.node,
    neighbours: edges.neighbours,
  };

  // The hatch is ADDED rather than gated, which is the whole of `SqlPolicy` in one line: a
  // withheld corpus does not carry a member that refuses — it does not carry the member, because
  // what is not on the surface cannot be reached by a caller that forgot to check.
  if (!rawSql) return corpus;
  const widened: SqlCorpus = {
    ...corpus,
    executeSql: verbs.executeSql,
  };
  return widened;
}
