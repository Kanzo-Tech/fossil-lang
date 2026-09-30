import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import './boot.js';
import {
  CorpusManifestError,
  CorpusReadError,
  open,
  type Corpus,
  type Direction,
  type OpenOptions,
  type TileAddress,
} from '../src/index.js';
import type { QueryFn } from '../src/query.js';
import { duckdb } from './engine.js';

/**
 * The reference API, run against the conformance corpus with a real engine.
 *
 * `tests/conformance.test.ts` executes `expected.json`, which is a table of **addresses**: what a
 * reader must compose and what it must refuse to compose. Nothing in it opens a byte. This file is
 * the other half — the corpus is 300 vertices in five tiles of 64 with a tail of 44, and 596 edges
 * stored twice, so every answer below has a size that a wrong shift, a hard-coded 4,096 or a
 * doubled orientation changes.
 *
 * **Every expectation is a second query, not a constant.** The API's answer is compared against SQL
 * this file writes over the same files, because a constant transcribed from a run of the code under
 * test agrees with it by construction. The four numbers that *are* constants — 300, 5, 64, 596 —
 * come from the manifest and from `packages/corpus/guards/vectors.json`, which neither this file nor
 * `open` wrote.
 *
 * **What it cannot prove**, and each of these is why:
 *
 * - **That the identity survives a re-layout**, which is the entire argument for keying on the
 *   subject IRI. Proving it needs two corpora over the same graph with different placements, and
 *   the fixture is one tree. What is proved instead is the half that is observable here: the API
 *   refuses a `dense_id` as an identity, and every id it hands back is a subject.
 * - **That the tile manifest is true.** It is written by the writer and believed by the reader. The
 *   cross-check below re-reads the rows, so it catches a box that disagrees with the pages *of this
 *   corpus*; it cannot catch a writer that lies consistently — `guards/`' `tile-manifest` holds it
 *   against the footers.
 * - **That the other container is read**, because it is not. The refusal is asserted; the row-group
 *   container has no test here because it has no implementation, and no manifest field to select it.
 * - **Anything above 2⁵³.** The `BigInt` boundary is exercised at conformance-corpus scale, where a
 *   `Number` would also work. `tests/address.test.ts` and `vectors.json` hold the borders.
 */

const CONFORMANCE = fileURLToPath(new URL('../conformance/', import.meta.url));
const CORPUS = join(CONFORMANCE, 'corpus');

/** The manifest's own numbers, read here so a hard-coded stride shows up as a failure. */
const VERTEX_COUNT = 300n;
const CHUNK_SIZE = 64n;
const TILES = 5n;
const EDGE_COUNT = 596n;

let query: QueryFn;
let engine: Engine;
let corpus: Corpus;
const scratch: string[] = [];

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
  // DuckDB-WASM under NODE_RUNTIME pre-allocates its spill files the moment a query touches the
  // filesystem, and it puts them in `./.tmp` relative to the process — which is the package
  // directory. Left alone this run writes 29 GB into `packages/corpus/.tmp` and does not remove it.
  const spill = mkdtempSync(join(tmpdir(), 'fossil-corpus-spill-'));
  scratch.push(spill);
  ({ engine, query } = await duckdb(spill));
  corpus = await open(CORPUS, { engine, sql: 'allowed' });
}, 60_000);

/** The engine, with every statement it is asked recorded — how a test counts what a member reads. */
function recording(statements: string[]): Engine {
  return {
    ...engine,
    query(sql, options) {
      statements.push(sql);
      return engine.query(sql, options);
    },
  };
}

/** The payload's zoom — the finest, and the last matrix. */
const payloadZ = (): number => corpus.tileMatrix('Person').tileMatrices.length - 1;

/**
 * **The relations incident to a set of vertices**, composed out of `edges` the way a caller composes
 * it: read each orientation's half from the tiles holding them, keep the rows whose aligned end is
 * one of them, and count each edge once. `edges` answers every edge of a tile; which of them a
 * caller wants is the caller's.
 */
async function incident(
  ids: ReadonlySet<bigint>,
  directions: readonly Direction[],
  emitted: Set<string> = new Set(),
): Promise<Array<{ src: bigint; dst: bigint }>> {
  const z = payloadZ();
  const tileRows = BigInt(corpus.tileMatrix('Person').tileMatrices[z]!.tileRows);
  const from = [...new Set([...ids].map((id) => Number(id / tileRows)))]
    .sort((a, b) => a - b)
    .map((tile): TileAddress => ({ type: 'Person', z, tile }));
  const found: Array<{ src: bigint; dst: bigint }> = [];
  for (const direction of directions) {
    for (const answer of await corpus.edges({ from, direction })) {
      expect(answer.declined).toEqual([]);
      for (const batch of answer.batches) {
        batch.src.forEach((src, i) => {
          const dst = batch.dst[i]!;
          if (!ids.has(direction === 'src' ? src : dst)) return;
          const key = `${src} ${dst}`;
          if (emitted.has(key)) return;
          emitted.add(key);
          found.push({ src, dst });
        });
      }
    }
  }
  return found;
}

/** One scalar out of a query this file wrote, so an expectation is never the code under test. */
async function scalar(sql: string): Promise<unknown> {
  const rows = await query(sql);
  return Object.values(rows[0]!)[0];
}

const lit = (s: string) => `'${s.replace(/'/g, "''")}'`;
const vertexTiles = () =>
  `[${[0, 1, 2, 3, 4].map((k) => lit(`${CORPUS}/vertex/Person/chunk${k}.parquet`)).join(', ')}]`;
const adjTiles = (dir: 'by_source' | 'by_target') =>
  `[${[0, 1, 2, 3, 4]
    .map((k) => lit(`${CORPUS}/edge/Person_knows_Person/${dir}/chunk${k}.parquet`))
    .join(', ')}]`;

describe('open — what is inside', () => {
  it('needs an engine, and names both ways to lend one rather than failing at the first query', async () => {
    await expect(open(CORPUS, {} as OpenOptions & { engine: Engine })).rejects.toThrow(
      /needs an engine: engine with host .* or engine alone/,
    );
  });

  it('reads the vertex type off the manifest and its columns off the bytes', () => {
    expect(corpus.types.vertices).toHaveLength(1);
    const person = corpus.types.vertices[0]!;
    expect(person.type).toBe('Person');
    expect(person.count).toBe(VERTEX_COUNT);
    expect(person.identity).toBe('subject');
    expect(person.geometry).toBe(true);
    // Seven on disk against THREE in the payload projection — the manifest's promise is not the
    // artefact, which is why the vocabulary is a DESCRIBE and not a read of the manifest. The
    // gap is what matters and not its width: it was five against one before the fixture grew
    // the two quasi-identifiers the declared privacy bound is measured over.
    expect(person.fields.map((f) => f.name)).toEqual([
      'dense_id',
      'subject',
      'birth_year',
      'postcode',
      'x',
      'y',
      'cluster_id',
    ]);
    expect(person.fields.map((f) => f.type)).toEqual([
      'UINTEGER',
      'VARCHAR',
      'INTEGER',
      'VARCHAR',
      'FLOAT',
      'FLOAT',
      'UINTEGER',
    ]);
  });

  it('carries three properties that the manifest declares, against seven columns on disk', () => {
    const declared = readFileSync(join(CORPUS, 'vertex/Person.vertex.yml'), 'utf8');
    expect(declared.match(/^ {2}- name: /gm)).toHaveLength(3);
    expect(corpus.types.vertices[0]!.fields.length).toBe(7);
  });

  it('reads the edge type, both orientations, one count for the pair', () => {
    expect(corpus.types.edges).toEqual([
      {
        edgeType: 'knows',
        srcType: 'Person',
        dstType: 'Person',
        count: EDGE_COUNT,
        directions: ['src', 'dst'],
      },
    ]);
  });

  it('takes the stride from the manifest — 64, not the default 4,096', () => {
    const payload = corpus.tileMatrix('Person').tileMatrices.at(-1)!;
    expect(payload.tileRows).toBe(Number(CHUNK_SIZE));
    expect(payload.tiles).toHaveLength(Number(TILES));
    // The tail tile: 300 = 4·64 + 44, the row `vectors.json` publishes for this corpus.
    expect(payload.tiles.at(-1)!.rows).toBe(Number(VERTEX_COUNT % CHUNK_SIZE));
  });
});

describe('the tile matrix — the box a caller has no other way to know', () => {
  it('agrees with the rows it claims to summarise', async () => {
    const box = corpus.tileMatrix('Person').extent;
    const truth = (
      await query(
        `SELECT min(x) AS minX, max(x) AS maxX, min(y) AS minY, max(y) AS maxY
           FROM read_parquet(${vertexTiles()})`,
      )
    )[0]!;
    expect(box).not.toBeNull();
    expect(box!.x).toBeCloseTo(Number(truth['minX']), 4);
    expect(box!.x + box!.w).toBeCloseTo(Number(truth['maxX']), 4);
    expect(box!.y).toBeCloseTo(Number(truth['minY']), 4);
    expect(box!.y + box!.h).toBeCloseTo(Number(truth['maxY']), 4);
  });
});

describe('node — the identity, and what it refuses to be', () => {
  it('finds a vertex by its subject IRI', async () => {
    const subject = String(
      await scalar(`SELECT subject FROM read_parquet(${vertexTiles()}) WHERE dense_id = 137`),
    );
    const vertex = await corpus.node(subject);
    expect(vertex).not.toBeNull();
    expect(vertex!.id).toBe(subject);
    expect(vertex!.denseId).toBe(137n);
    expect(vertex!.type).toBe('Person');
    // The payload minus the four the answer reads by name.
    expect(Object.keys(vertex!.fields)).toEqual(['birth_year', 'postcode', 'cluster_id']);
  });

  it('refuses a dense id, because an address is not a name', async () => {
    await expect(corpus.node(137n as unknown as string)).rejects.toThrow(TypeError);
    await expect(corpus.node(137n as unknown as string)).rejects.toThrow(/re-layout/);
  });

  it('returns null for an IRI the corpus does not carry', async () => {
    expect(await corpus.node('https://example.org/person/nobody')).toBeNull();
  });

  it('round-trips every id a scan hands out', async () => {
    const scan = corpus.scan({ type: 'Person', select: ['dense_id', 'subject'] });
    const [batch] = await scan.read([{ type: 'Person', z: payloadZ(), tile: 0 }]);
    const ids = Array.from(batch!.getChild('dense_id')!.toArray(), (v) => BigInt(v as number));
    const subjects = Array.from(batch!.getChild('subject')!.toArray(), String);
    expect(ids.length).toBeGreaterThan(0);
    for (const [k, subject] of subjects.slice(0, 5).entries()) {
      expect((await corpus.node(subject))!.denseId).toBe(ids[k]);
    }
  });
});

/** The subject IRI of one address, read by this file rather than by the code under test. */
const seedOf = async (denseId: number) =>
  String(
    await scalar(`SELECT subject FROM read_parquet(${vertexTiles()}) WHERE dense_id = ${denseId}`),
  );

describe('node — what a lookup reads', () => {
  it('asks only the index tiles whose footers say a key could be in them', async () => {
    // The index is five tiles of 64 sorted by `subject`, with disjoint ranges. Two identities that
    // land in one tile must not read the other four — which is the defect this replaced: DuckDB
    // prunes no disjunction over a VARCHAR column, so `key IN (a, b)` opened all five and only
    // `key = a` opened one.
    const statements: string[] = [];
    const counted = await open(CORPUS, { engine: recording(statements) });
    const named = (): string[][] =>
      statements
        .filter((sql) => sql.includes('/index/'))
        .map((sql) => sql.match(/index\/tile\d+\.parquet/g) ?? []);
    // Sorted lexicographically, `.../person/0` and `.../person/1` are neighbours, so this is the
    // pair most likely to share a tile — and the assertion is a relationship, not a tile number:
    // fewer than every tile, and never fewer than the one holding the answer.
    const pair = [await seedOf(0), await seedOf(1)];
    statements.length = 0;
    const found = await Promise.all(pair.map((id) => counted.node(id)));
    expect(found.every((v) => v !== null)).toBe(true);

    // The footer sweep names every index tile once — that is what a sweep is — and the lookups
    // after it name a strict subset. The first is the sweep; the rest are the reads it decided.
    const [sweep, ...reads] = named();
    expect(sweep).toHaveLength(Number(TILES));
    for (const read of reads) {
      expect(read.length).toBeGreaterThanOrEqual(1);
      expect(read.length).toBeLessThan(Number(TILES));
    }
  });

  it('the type says whether a lookup on it is a seek or a scan', () => {
    // Both answers are correct and only one is fast, so a consumer that cannot tell them apart
    // finds out by measuring. This corpus has an index; `refuses` below covers one without.
    expect(corpus.types.vertices.every((v) => v.indexed)).toBe(true);
  });
});

describe('the verbs, through the same door', () => {
  /**
   * The six verbs answer over a corpus that was opened by URL — which is the whole of the merge.
   *
   * They were `createGraphClient`, a second entry point taking the same two arguments and with no
   * rule for choosing. It is the transport now. What it needed and `open` did not is the
   * bridge asserted below: the verbs' SQL names tables, this corpus is Parquet files, and the door
   * registers the temp views that join the two.
   *
   * **These do not assert what the corpus API asserts elsewhere, on purpose.** A verb reads the
   * MANIFEST's vocabulary and this corpus declares three properties against seven columns on
   * disk, so `read` answers with `subject` alone and `schema` reports no fields at all. That divergence is
   * the reason `open` describes the bytes instead, and pinning it here is what stops the two
   * halves being confused for one.
   */
  it('answers a verb over a corpus opened by URL', async () => {
    const { vertices, edges, fields } = await corpus.schema();
    expect(vertices).toHaveLength(1);
    expect(vertices[0]!.name).toBe('Person');
    // The count is `count(*)` over the registered view, not the manifest's declaration — so this
    // is the bytes agreeing with `vertex_count`, which is the disagreement the corpus guards
    // exist to catch.
    expect(vertices[0]!.count).toBe(Number(VERTEX_COUNT));
    expect(edges).toHaveLength(1);
    expect(edges[0]!.table_name).toBe('Person_knows_Person');
    expect(edges[0]!.count).toBe(Number(EDGE_COUNT));
    // Seven columns on disk, three declared, and `subject` is a writer column: no field survives.
    expect(fields).toHaveLength(0);
  }, 30_000);

  it('answers every type\'s field stats, with their kind, in one call', async () => {
    const { vertices } = await corpus.schema({ stats: true });
    const person = vertices.find((v) => v.name === 'Person')!;
    expect(person.stats.map((f) => f.name)).toEqual(person.fields);
    expect(person.stats.map((f) => [f.name, f.kind])).toEqual([
      ['birth_year', 'numeric'],
      ['postcode', 'categorical'],
    ]);
    // Without the flag the summaries carry none — the cheap call stays cheap.
    expect((await corpus.schema()).vertices.every((v) => v.stats.length === 0)).toBe(true);
  }, 30_000);

  it('lists the relations a host records, named by the corpus and never composed', async () => {
    const relations = await corpus.relations();
    const { vertices, edges } = await corpus.schema();
    expect(relations.map((r) => r.name)).toEqual([
      ...vertices.map((v) => v.name),
      ...edges.map((e) => e.table_name),
    ]);
    const person = relations.find((r) => r.kind === 'vertex')!;
    expect(person.rows).toBe(Number(VERTEX_COUNT));
    expect(person.files).toEqual(
      [0, 1, 2, 3, 4].map((k) => `${CORPUS}/vertex/Person/chunk${k}.parquet`),
    );
    expect(person.kind === 'vertex' && person.columns).toEqual(corpus.types.vertices[0]!.fields);
    const knows = relations.find((r) => r.kind === 'edge')!;
    expect(knows.rows).toBe(Number(EDGE_COUNT));
    expect(knows.files).toEqual(
      [0, 1, 2, 3, 4].map((k) => `${CORPUS}/edge/Person_knows_Person/by_source/chunk${k}.parquet`),
    );
  }, 30_000);

  it('withholds the hatch by default, and admits it when the host says so', async () => {
    // A withheld corpus does not carry a member that refuses: it does not carry the member.
    const closed = await open(CORPUS, { engine });
    expect('executeSql' in closed).toBe(false);

    // And the corpus this file opened with the permission has it.
    expect('executeSql' in corpus).toBe(true);
    const permitted = corpus as typeof corpus & {
      executeSql(params: { sql: string }): Promise<{ rows: unknown[] }>;
    };
    expect((await permitted.executeSql({ sql: 'SELECT 1 AS n' })).rows).toHaveLength(1);
  }, 30_000);

  it('keeps its relations in a catalog of its own, beside a host table of the same name', async () => {
    // `Person` is not an unlikely name for a table the host already has, nor for a second corpus
    // in the same engine. The verbs' views live in a database named after the corpus, so the
    // host's table is neither replaced nor shadowed, and the relation is reached by the name
    // `relations()` answers with.
    await query(`CREATE OR REPLACE TABLE memory.main."Person" AS SELECT 99 AS host_owned`);
    const own = await open(CORPUS, { engine });
    expect((await own.schema()).vertices[0]!.count).toBe(Number(VERTEX_COUNT));
    expect(await query(`SELECT host_owned FROM "Person"`)).toEqual([{ host_owned: 99 }]);
    const person = (await own.relations()).find((r) => r.name === 'Person')!;
    const [seen] = await query(`SELECT count(*) AS n FROM ${person.sql}`);
    expect(Number(seen!['n'])).toBe(Number(VERTEX_COUNT));
    await own.close();
    await query(`DROP TABLE memory.main."Person"`);
  }, 30_000);

  it('registers no view for a caller that only draws', async () => {
    // The verbs are the only half that needs the views, so they are registered on the first verb
    // call. A tile read and an edge read must not reach for them — asserted as the absence of a
    // `CREATE ... VIEW`.
    const statements: string[] = [];
    const drawing = await open(CORPUS, { engine: recording(statements) });
    statements.length = 0;
    const tile: TileAddress = { type: 'Person', z: payloadZ(), tile: 0 };
    await drawing.scan({ type: 'Person' }).read([tile]);
    await drawing.edges({ from: [tile], direction: 'src' });
    expect(statements.length).toBeGreaterThan(0);
    expect(statements.some((sql) => sql.includes('VIEW'))).toBe(false);
  }, 30_000);
});

describe('what open refuses, and names', () => {
  /** A corpus tree whose manifests can be edited without touching the fixture. */
  function fork(edit: (yaml: string) => string, tiles: boolean): string {
    const dir = mkdtempSync(join(tmpdir(), 'fossil-corpus-'));
    scratch.push(dir);
    cpSync(CORPUS, dir, { recursive: true });
    const vertexYml = join(dir, 'vertex/Person.vertex.yml');
    writeFileSync(vertexYml, edit(readFileSync(vertexYml, 'utf8')));
    if (!tiles) {
      // The other container: one file holding the type, which is what a Parquet writer produces by
      // default and what the measured winner would be. The data is right there and unreadable.
      cpSync(join(dir, 'vertex/Person/chunk0.parquet'), join(dir, 'vertex/Person.parquet'));
      rmSync(join(dir, 'vertex/Person'), { recursive: true, force: true });
      mkdirSync(join(dir, 'vertex/Person'));
    }
    return dir;
  }

  it('refuses a manifest that declares no vertex_count, because tiles are addressed and never listed', async () => {
    const dir = fork((yaml) => yaml.replace(/^vertex_count: .*\n/m, ''), true);
    await expect(open(dir, { engine })).rejects.toThrow(CorpusManifestError);
    await expect(open(dir, { engine })).rejects.toThrow(/no vertex_count/);
  });

  it('refuses the row-group container by naming the tile it did not find, and does not glob', async () => {
    const dir = fork((yaml) => yaml, false);
    await expect(open(dir, { engine })).rejects.toThrow(CorpusReadError);
    await expect(open(dir, { engine })).rejects.toThrow(/chunk0\.parquet/);
    // The point of naming it: a file with the type's rows in it is sitting beside the address, and
    // a reader that globbed would open it, count every row twice against the manifest, and pass.
    await expect(open(dir, { engine })).rejects.toThrow(/container/);
  });
});

/**
 * The same table `packages/corpus/conformance/verify.mjs` executes, executed here.
 *
 * Everything above compares an answer to SQL **this file writes**, which catches the API being
 * wrong about the bytes. This asks the other question — and asks it of `scan`, `edges` and `node`
 * composed the way a view composes them, since the members the table was first written against
 * (`rows`, `neighbours`) are gone and their answers are now the caller's loop over these — and it is the one a self-check cannot: has
 * this reader drifted from the other one? `expected.json`'s `answers` block is a table neither
 * implementation wrote — its numbers come from a full scan of every tile with no addressing at all
 * — and `conformance/answers.mjs` executes it in plain Node over the `duckdb` binary, sharing no
 * line of answer logic with `open`.
 *
 * It is the distinction the addressing half already draws and states in `verify.mjs`'s header: a
 * table catches **drift between two readers**, and pointing a reader at bytes a writer just made
 * catches **two readers agreeing while both disagree with the writer**. Neither replaces the other,
 * and neither replaces the inline cross-checks above.
 *
 * The block has already earned it once, on the other implementation: an adjacency tile filtered by
 * `src_dense OR dst_dense` instead of by the column its orientation is aligned on read 153 edges
 * for a drawing read where the scan says 152.
 */
describe('the conformance table, executed against the published API', () => {
  const table = JSON.parse(readFileSync(join(CONFORMANCE, 'expected.json'), 'utf8')) as {
    answers: {
      vertex_count: number;
      types: { vertices: Array<Record<string, unknown>>; edges: Array<Record<string, unknown>> };
      window: Array<{
        box: { x: number; y: number; w: number; h: number };
        directions: Direction[];
        vertices: number;
        tiles: number[];
        edges: number;
      }>;
      node: Array<{ id: string; found: boolean; dense_id?: number }>;
      index: { indexed: boolean; same_either_way: string[] };
      neighbours: Array<{
        ids: string[];
        depth: number;
        vertices: number;
        edges: number;
        frontier: number;
      }>;
    };
  };

  it('is a table with something in it', () => {
    // A renamed key or a moved file would otherwise make every case below vacuous, which is the
    // classic way a shared contract stops being shared.
    expect(table.answers.window.length).toBeGreaterThan(0);
    expect(table.answers.neighbours.length).toBeGreaterThan(0);
  });

  it('what is inside is what the table says is inside', () => {
    const declared = table.answers.types.vertices[0] as {
      type: string;
      fields: string[];
      identity: string;
      geometry: boolean;
    };
    const got = corpus.types.vertices.find((v) => v.type === declared.type);
    expect(got).toBeDefined();
    expect(got!.count).toBe(BigInt(table.answers.vertex_count));
    expect(got!.fields.map((f) => f.name)).toEqual(declared.fields);
    expect(got!.identity).toBe(declared.identity);
    expect(got!.geometry).toBe(declared.geometry);
  });

  it.each(table.answers.window)(
    'window $box.x,$box.y over $directions, as a scan and its edges',
    async ({ box, directions, vertices, tiles, edges }) => {
      // The box is half-open, `x <= v.x < x + w`, as the table's full scan reads it.
      const scan = corpus.scan({
        type: 'Person',
        filter: {
          and: [
            { column: 'x', op: '>=', value: box.x },
            { column: 'x', op: '<', value: box.x + box.w },
            { column: 'y', op: '>=', value: box.y },
            { column: 'y', op: '<', value: box.y + box.h },
          ],
        },
        select: ['dense_id'],
      });
      const tasks = scan.plan().filter((t) => t.z === payloadZ());
      expect(tasks.map((t) => t.tile)).toEqual(tiles);
      const ids = new Set(
        (await scan.read(tasks)).flatMap((b) =>
          Array.from(b.getChild('dense_id')!.toArray(), (v) => BigInt(v as number)),
        ),
      );
      expect(ids.size).toBe(vertices);
      expect((await incident(ids, directions)).length).toBe(edges);
    },
  );

  it.each(table.answers.node)('node $id resolves to found=$found', async ({ id, found, dense_id }) => {
    const got = await corpus.node(id);
    if (!found) {
      expect(got).toBeNull();
      return;
    }
    expect(got).not.toBeNull();
    expect(got!.id).toBe(id);
    expect(got!.denseId).toBe(BigInt(dense_id!));
  });

  it('hands back the same vertex with the index and without it', async () => {
    // The one assertion that makes the index an OPTIMISATION rather than a second truth. Nothing
    // else here can see which route ran: both return the same answer by construction, which is
    // exactly the gap `packages/corpus/guards`' `index-agrees-with-the-payload` names in its own
    // `cannotProve`, closed from the reader's side.
    const dir = mkdtempSync(join(tmpdir(), 'fossil-noindex-'));
    scratch.push(dir);
    cpSync(CORPUS, dir, { recursive: true });
    const yml = join(dir, 'vertex/Person.vertex.yml');
    writeFileSync(yml, readFileSync(yml, 'utf8').replace(/^index:\n(?: {2}.*\n)*/m, ''));

    const scanning = await open(dir, { engine });
    // The comparison is worthless if the strip did not strip, and worthless the other way if the
    // fixture never had one. Both are asserted.
    expect(corpus.types.vertices[0]!.indexed).toBe(true);
    expect(scanning.types.vertices[0]!.indexed).toBe(false);

    for (const id of (table.answers as { index: { same_either_way: string[] } }).index
      .same_either_way) {
      const seek = await corpus.node(id);
      const scan = await scanning.node(id);
      expect(seek).not.toBeNull();
      expect(scan).toEqual(seek);
    }
  });

  it.each(table.answers.neighbours)(
    'a walk of depth $depth, as node and a loop over edges',
    async ({ ids, depth, vertices, edges, frontier }) => {
      // A walk of more than one hop is the caller's loop: resolve the seeds by identity, then per
      // hop read both halves from the frontier's tiles and keep what is incident to the frontier.
      const seeds = await Promise.all(ids.map((id) => corpus.node(id)));
      const seen = new Set(seeds.map((v) => v!.denseId));
      const emitted = new Set<string>();
      let reached = [...seen];
      for (let hop = 0; hop < depth && reached.length > 0; hop += 1) {
        const found = await incident(new Set(reached), ['src', 'dst'], emitted);
        reached = [...new Set(found.flatMap((e) => [e.src, e.dst]))].filter((id) => !seen.has(id));
        for (const id of reached) seen.add(id);
      }
      expect(seen.size).toBe(vertices);
      expect(emitted.size).toBe(edges);
      expect(reached.length).toBe(frontier);
    },
  );
});
