import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { CorpusManifestError, CorpusReadError, openCorpus, type Corpus } from '../src/corpus.js';
import type { QueryFn, QueryRow } from '../src/query.js';

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
 * come from the manifest and from `apps/corpus/guards/vectors.json`, which neither this file nor
 * `openCorpus` wrote.
 *
 * **What it cannot prove**, and each of these is why:
 *
 * - **That the identity survives a re-layout**, which is the entire argument for keying on the
 *   subject IRI. Proving it needs two corpora over the same graph with different placements, and
 *   the fixture is one tree. What is proved instead is the half that is observable here: the API
 *   refuses a `dense_id` as an identity, and every id it hands back is a subject.
 * - **That `extent` is true.** It is read off the Parquet footers, and a footer is written by the
 *   writer and believed by the reader. The cross-check below re-reads the rows, so it catches a box
 *   that disagrees with the pages *of this corpus*; it cannot catch a writer that lies consistently.
 * - **That the other container is read**, because it is not. The refusal is asserted; the row-group
 *   container has no test here because it has no implementation, and no manifest field to select it.
 * - **Anything above 2⁵³.** The `BigInt` boundary is exercised at conformance-corpus scale, where a
 *   `Number` would also work. `tests/address.test.ts` and `vectors.json` hold the borders.
 */

const require = createRequire(import.meta.url);
const CONFORMANCE = fileURLToPath(new URL('../../../apps/corpus/conformance/', import.meta.url));
const CORPUS = join(CONFORMANCE, 'corpus');

/** The manifest's own numbers, read here so a hard-coded stride shows up as a failure. */
const VERTEX_COUNT = 300n;
const CHUNK_SIZE = 64n;
const TILES = 5n;
const EDGE_COUNT = 596n;

let query: QueryFn;
let corpus: Corpus;
const scratch: string[] = [];

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
  const dist = dirname(require.resolve('@duckdb/duckdb-wasm'));
  const db = await createDuckDB(
    {
      mvp: {
        mainModule: resolve(dist, './duckdb-mvp.wasm'),
        mainWorker: resolve(dist, './duckdb-node-mvp.worker.cjs'),
      },
      eh: {
        mainModule: resolve(dist, './duckdb-eh.wasm'),
        mainWorker: resolve(dist, './duckdb-node-eh.worker.cjs'),
      },
    },
    new ConsoleLogger(),
    NODE_RUNTIME,
  );
  await db.instantiate();
  const conn = db.connect();
  // DuckDB-WASM under NODE_RUNTIME pre-allocates its spill files the moment a query touches the
  // filesystem, and it puts them in `./.tmp` relative to the process — which is the package
  // directory. Left alone this run writes 29 GB into `packages/graph/.tmp` and does not remove it.
  const spill = mkdtempSync(join(tmpdir(), 'fossil-corpus-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);

  // The host capability, and this is the whole of it: run SQL, hand back rows. Deliberately NOT
  // normalising widths — DuckDB-WASM returns a UINTEGER as a Number and a UBIGINT as a BigInt, and
  // the point of `idOf` is that the API survives a host that does either.
  query = async (sql: string): Promise<QueryRow[]> =>
    conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());

  corpus = await openCorpus(CORPUS, { query });
}, 60_000);

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
    .map((k) => lit(`${CORPUS}/edge/Person_knows_Person/${dir}/tile${k}.parquet`))
    .join(', ')}]`;

describe('openCorpus — what is inside', () => {
  it('needs an engine, and says so rather than failing at the first query', async () => {
    await expect(openCorpus(CORPUS, {} as { query: QueryFn })).rejects.toThrow(
      /needs a query capability/,
    );
  });

  it('reads the vertex type off the manifest and its columns off the bytes', () => {
    expect(corpus.types.vertices).toHaveLength(1);
    const person = corpus.types.vertices[0]!;
    expect(person.type).toBe('Person');
    expect(person.count).toBe(VERTEX_COUNT);
    expect(person.identity).toBe('subject');
    expect(person.geometry).toBe(true);
    // Five on disk against ONE in `property_groups` — the manifest's promise is not the artefact,
    // which is why the vocabulary is a DESCRIBE and not a read of the manifest.
    expect(person.fields.map((f) => f.name)).toEqual([
      'dense_id',
      'subject',
      'x',
      'y',
      'cluster_id',
    ]);
    expect(person.fields.map((f) => f.type)).toEqual([
      'UINTEGER',
      'VARCHAR',
      'FLOAT',
      'FLOAT',
      'UINTEGER',
    ]);
  });

  it('carries one property that the manifest declares, against five columns on disk', () => {
    const declared = readFileSync(join(CORPUS, 'vertex/Person.vertex.yml'), 'utf8');
    expect(declared.match(/^ {2}- name: /gm)).toHaveLength(1);
    expect(corpus.types.vertices[0]!.fields.length).toBe(5);
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
    const address = corpus.addressing.vertexType();
    expect(address.chunkSize).toBe(Number(CHUNK_SIZE));
    expect(address.tiles).toBe(TILES);
    expect(address.tileUrls()).toHaveLength(Number(TILES));
    // The tail tile: 300 = 4·64 + 44, the row `vectors.json` publishes for this corpus.
    expect(address.tileOf(VERTEX_COUNT - 1n)).toBe(TILES - 1n);
  });
});

describe('extent — the box a caller has no other way to know', () => {
  it('agrees with the rows it claims to summarise', async () => {
    const box = await corpus.extent();
    const truth = (
      await query(
        `SELECT min(x) AS minX, max(x) AS maxX, min(y) AS minY, max(y) AS maxY
           FROM read_parquet(${vertexTiles()})`,
      )
    )[0]!;
    expect(box).not.toBeNull();
    expect(box!.minX).toBeCloseTo(Number(truth['minX']), 4);
    expect(box!.maxX).toBeCloseTo(Number(truth['maxX']), 4);
    expect(box!.minY).toBeCloseTo(Number(truth['minY']), 4);
    expect(box!.maxY).toBeCloseTo(Number(truth['maxY']), 4);
  });
});

describe('window — the vertices in a rectangle and the edges among them', () => {
  /** A rectangle that covers everything, so the answer's size is the corpus's own numbers. */
  const everything = async () => {
    const box = (await corpus.extent())!;
    return {
      x: box.minX - 1,
      y: box.minY - 1,
      w: box.maxX - box.minX + 2,
      h: box.maxY - box.minY + 2,
    };
  };

  it('returns every vertex when the box is the extent, and reports every tile', async () => {
    const answer = await corpus.window(await everything());
    expect(answer.vertices).toHaveLength(Number(VERTEX_COUNT));
    expect(answer.tiles).toEqual([0n, 1n, 2n, 3n, 4n]);
    expect(answer.vertices.every((v) => typeof v.id === 'string')).toBe(true);
  });

  it('counts each edge once when both orientations are read', async () => {
    const answer = await corpus.window(await everything());
    // 596 is the manifest's `edge_count`, and it covers the pair: the same relation is in
    // `by_source` and in `by_target`. Reading both and concatenating returns each edge twice.
    expect(answer.edges).toHaveLength(Number(EDGE_COUNT));
    expect(new Set(answer.edges.map((e) => `${e.src}>${e.dst}`)).size).toBe(Number(EDGE_COUNT));
    expect(await scalar(`SELECT count(*) FROM read_parquet(${adjTiles('by_source')})`)).toBe(
      EDGE_COUNT,
    );
  });

  it('is complete for incidence with both orientations, and says which is missing with one', async () => {
    const box = await everything();
    expect(await corpus.window(box)).toMatchObject({ complete: true, gaps: [] });
    const drawing = await corpus.window({ ...box, directions: ['src'] });
    expect(drawing.complete).toBe(false);
    expect(drawing.gaps).toEqual([{ edgeType: 'knows', direction: 'dst', reason: 'not-requested' }]);
  });

  it('answers a sub-box with exactly the rows the box names, and nothing from a tile it touched', async () => {
    const full = (await corpus.extent())!;
    const box = {
      x: full.minX,
      y: full.minY,
      w: (full.maxX - full.minX) / 3,
      h: (full.maxY - full.minY) / 3,
    };
    const answer = await corpus.window(box);
    const where = `x >= ${box.x} AND x < ${box.x + box.w} AND y >= ${box.y} AND y < ${box.y + box.h}`;
    expect(answer.vertices.length).toBe(
      Number(await scalar(`SELECT count(*) FROM read_parquet(${vertexTiles()}) WHERE ${where}`)),
    );
    expect(answer.vertices.length).toBeGreaterThan(0);
    expect(answer.vertices.length).toBeLessThan(Number(VERTEX_COUNT));
    // The tiles are reported, never asked for: they are exactly the tiles the answer's own ids fall
    // in, which is the only sense in which a tile appears in this API at all.
    expect(answer.tiles).toEqual([
      ...new Set(answer.vertices.map((v) => v.denseId / CHUNK_SIZE)),
    ].sort((a, b) => Number(a - b)));
  });

  it('returns every edge incident to the box and no edge incident to neither end', async () => {
    const full = (await corpus.extent())!;
    const box = {
      x: full.minX,
      y: full.minY,
      w: (full.maxX - full.minX) / 3,
      h: (full.maxY - full.minY) / 3,
    };
    const answer = await corpus.window(box);
    const inside = new Set(answer.vertices.map((v) => v.denseId));
    expect(answer.edges.every((e) => inside.has(e.src) || inside.has(e.dst))).toBe(true);

    const where = `x >= ${box.x} AND x < ${box.x + box.w} AND y >= ${box.y} AND y < ${box.y + box.h}`;
    const truth = await scalar(
      `SELECT count(*) FROM (
         SELECT src_dense, dst_dense FROM read_parquet(${adjTiles('by_source')})
          WHERE src_dense IN (SELECT dense_id FROM read_parquet(${vertexTiles()}) WHERE ${where})
          UNION
         SELECT src_dense, dst_dense FROM read_parquet(${adjTiles('by_target')})
          WHERE dst_dense IN (SELECT dense_id FROM read_parquet(${vertexTiles()}) WHERE ${where}))`,
    );
    expect(answer.edges).toHaveLength(Number(truth));
    expect(Number(truth)).toBeGreaterThan(0);
  });

  it('answers an empty box as complete rather than as a failure', async () => {
    const full = (await corpus.extent())!;
    const answer = await corpus.window({ x: full.maxX + 10, y: full.maxY + 10, w: 1, h: 1 });
    expect(answer.vertices).toEqual([]);
    expect(answer.edges).toEqual([]);
    expect(answer.tiles).toEqual([]);
    expect(answer.complete).toBe(true);
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
    expect(Object.keys(vertex!.fields)).toEqual(['cluster_id']);
  });

  it('refuses a dense id, because an address is not a name', async () => {
    await expect(corpus.node(137n as unknown as string)).rejects.toThrow(TypeError);
    await expect(corpus.node(137n as unknown as string)).rejects.toThrow(/re-layout/);
  });

  it('returns null for an IRI the corpus does not carry', async () => {
    expect(await corpus.node('https://example.org/person/nobody')).toBeNull();
  });

  it('round-trips every id it hands out', async () => {
    const full = (await corpus.extent())!;
    const answer = await corpus.window({ x: full.minX, y: full.minY, w: 1e-3, h: 1e30 });
    expect(answer.vertices.length).toBeGreaterThan(0);
    for (const vertex of answer.vertices.slice(0, 5)) {
      expect((await corpus.node(vertex.id!))!.denseId).toBe(vertex.denseId);
    }
  });
});

describe('neighbours — a walk seeded by identity', () => {
  const seedOf = async (denseId: number) =>
    String(
      await scalar(
        `SELECT subject FROM read_parquet(${vertexTiles()}) WHERE dense_id = ${denseId}`,
      ),
    );

  it('reaches exactly the one-hop neighbourhood in both orientations', async () => {
    const seed = await seedOf(42);
    const hood = await corpus.neighbours([seed]);
    const truth = (
      await query(
        `SELECT dst_dense AS n FROM read_parquet(${adjTiles('by_source')}) WHERE src_dense = 42
           UNION
         SELECT src_dense AS n FROM read_parquet(${adjTiles('by_target')}) WHERE dst_dense = 42`,
      )
    ).map((r) => BigInt(r['n'] as number));
    expect(truth.length).toBeGreaterThan(0);
    const reached = new Set(hood.vertices.map((v) => v.denseId));
    expect(reached).toEqual(new Set([42n, ...truth]));
    expect(hood.seeds).toEqual([seed]);
    expect(hood.missing).toEqual([]);
    // Every vertex reached is named, so a caller can key on what it got back.
    expect(hood.vertices.every((v) => typeof v.id === 'string')).toBe(true);
  });

  it('emits each edge once, however many orientations found it', async () => {
    const hood = await corpus.neighbours([await seedOf(42)]);
    expect(new Set(hood.edges.map((e) => `${e.src}>${e.dst}`)).size).toBe(hood.edges.length);
    expect(hood.edges.every((e) => e.edgeType === 'knows')).toBe(true);
  });

  /**
   * A breadth-first walk over the same 596 edges, in this file, with no addressing in it.
   *
   * The API walks by *tile*: it turns a frontier into tile numbers, opens those adjacency files and
   * keeps the rows whose addressed endpoint is in the frontier. This walks an adjacency map. They
   * are the same answer only if the shift, the orientation pairing and the frontier bookkeeping are
   * all right, and this corpus is 34 hops across, so a walk that quietly stalled or re-visited would
   * diverge long before it exhausted the component.
   */
  async function bfs(from: bigint, depth: number): Promise<{ seen: Set<bigint>; ring: Set<bigint> }> {
    const rows = await query(`SELECT src_dense, dst_dense FROM read_parquet(${adjTiles('by_source')})`);
    const adjacent = new Map<bigint, Set<bigint>>();
    const link = (a: bigint, b: bigint) => {
      if (!adjacent.has(a)) adjacent.set(a, new Set());
      adjacent.get(a)!.add(b);
    };
    for (const row of rows) {
      const a = BigInt(row['src_dense'] as number);
      const b = BigInt(row['dst_dense'] as number);
      link(a, b);
      link(b, a);
    }
    const seen = new Set([from]);
    let ring = new Set([from]);
    for (let hop = 0; hop < depth && ring.size > 0; hop += 1) {
      const next = new Set<bigint>();
      for (const n of ring) {
        for (const m of adjacent.get(n) ?? []) {
          if (!seen.has(m)) {
            seen.add(m);
            next.add(m);
          }
        }
      }
      ring = next;
    }
    return { seen, ring };
  }

  it('reaches hop for hop what a breadth-first walk over the same edges reaches', async () => {
    const seed = await seedOf(42);
    for (const depth of [1, 2, 3, 7]) {
      const hood = await corpus.neighbours([seed], { depth });
      const truth = await bfs(42n, depth);
      expect(new Set(hood.vertices.map((v) => v.denseId))).toEqual(truth.seen);
      expect(new Set(hood.frontier)).toEqual(truth.ring);
    }
  }, 30_000);

  it('is incomplete while the frontier is populated, and complete once the walk exhausts it', async () => {
    const seed = await seedOf(42);
    const one = await corpus.neighbours([seed], { depth: 1 });
    expect(one.frontier.length).toBeGreaterThan(0);
    expect(one.complete).toBe(false);
    expect(one.gaps).toEqual([]); // both orientations read — the gap is the depth, not a direction

    // This component is 34 hops across from here, so a depth that merely looks generous does not
    // exhaust it. Walked past that, the frontier empties, every vertex is reached and nothing was
    // cut off — which is the only configuration in which `complete` is true.
    const all = await corpus.neighbours([seed], { depth: 40 });
    expect(all.frontier).toEqual([]);
    expect(all.complete).toBe(true);
    expect(all.vertices).toHaveLength(Number(VERTEX_COUNT));
    expect(all.edges).toHaveLength(Number(EDGE_COUNT));
  }, 30_000);

  it('reports a seed it could not resolve rather than silently walking without it', async () => {
    const hood = await corpus.neighbours([await seedOf(42), 'https://example.org/person/nobody']);
    expect(hood.missing).toEqual(['https://example.org/person/nobody']);
    expect(hood.seeds).toHaveLength(1);
  });

  it('refuses a depth that is not a whole number of hops', async () => {
    await expect(corpus.neighbours([], { depth: 0 })).rejects.toThrow(RangeError);
  });

  it('resolves a batch of seeds without one scan per seed', async () => {
    const seeds = await Promise.all([1, 2, 3, 4, 5].map(seedOf));
    let queries = 0;
    const counted = await openCorpus(CORPUS, {
      query: async (sql) => {
        queries += 1;
        return query(sql);
      },
    });
    const before = queries;
    await counted.neighbours(seeds, { depth: 1 });
    // One scan for the whole batch, one adjacency read per orientation, one read of the reached
    // tiles. A per-seed scan of the `subject` column is the cost this API is most able to multiply.
    expect(queries - before).toBeLessThanOrEqual(4);
  });
});

describe('what openCorpus refuses, and names', () => {
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
    await expect(openCorpus(dir, { query })).rejects.toThrow(CorpusManifestError);
    await expect(openCorpus(dir, { query })).rejects.toThrow(/no vertex_count/);
  });

  it('refuses the row-group container by naming the tile it did not find, and does not glob', async () => {
    const dir = fork((yaml) => yaml, false);
    await expect(openCorpus(dir, { query })).rejects.toThrow(CorpusReadError);
    await expect(openCorpus(dir, { query })).rejects.toThrow(/chunk0\.parquet/);
    // The point of naming it: a file with the type's rows in it is sitting beside the address, and
    // a reader that globbed would open it, count every row twice against the manifest, and pass.
    await expect(openCorpus(dir, { query })).rejects.toThrow(/container/);
  });
});

/**
 * The same table `apps/corpus/conformance/verify.mjs` executes, executed here.
 *
 * Everything above compares an answer to SQL **this file writes**, which catches the API being
 * wrong about the bytes. This asks the other question, and it is the one a self-check cannot: has
 * this reader drifted from the other one? `expected.json`'s `answers` block is a table neither
 * implementation wrote — its numbers come from a full scan of every tile with no addressing at all
 * — and `conformance/answers.mjs` executes it in plain Node over the `duckdb` binary, sharing no
 * line of answer logic with `openCorpus`.
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
        directions: Array<'src' | 'dst'>;
        vertices: number;
        tiles: number[];
        edges: number;
        complete: boolean;
        gaps: string[];
      }>;
      node: Array<{ id: string; found: boolean; dense_id?: number }>;
      neighbours: Array<{
        ids: string[];
        depth: number;
        seeds: number;
        missing: number;
        vertices: number;
        edges: number;
        frontier: number;
        complete: boolean;
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
    'window $box.x,$box.y over $directions',
    async ({ box, directions, vertices, tiles, edges, complete, gaps }) => {
      const got = await corpus.window({ ...box, directions });
      expect(got.vertices.length).toBe(vertices);
      expect(got.tiles.map(Number)).toEqual(tiles);
      expect(got.edges.length).toBe(edges);
      expect(got.complete).toBe(complete);
      expect(got.gaps.map((g) => g.reason).sort()).toEqual(gaps);
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

  it.each(table.answers.neighbours)(
    'neighbours at depth $depth',
    async ({ ids, depth, seeds, missing, vertices, edges, frontier, complete }) => {
      const got = await corpus.neighbours(ids, { depth });
      expect(got.seeds.length).toBe(seeds);
      expect(got.missing.length).toBe(missing);
      expect(got.vertices.length).toBe(vertices);
      expect(got.edges.length).toBe(edges);
      expect(got.frontier.length).toBe(frontier);
      expect(got.complete).toBe(complete);
    },
  );
});
