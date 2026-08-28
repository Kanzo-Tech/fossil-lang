import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { CorpusManifestError, CorpusReadError, openCorpus, type Corpus } from '../src/corpus.js';
import { initFossilGraphWasm } from '../src/load.js';
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

  // The verbs run in `fossil-graph-wasm`, and `openCorpus` boots it on the first verb call only
  // when it was told where it is. Here the module is booted directly, which is the other half of
  // the same memoised init and the shape a host that already holds the WASM is in.
  await initFossilGraphWasm({
    wasmUrl: (await readFile(
      fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
    )) as unknown as URL,
  });

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
    // Seven on disk against THREE in `property_groups` — the manifest's promise is not the
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
    const address = corpus.addressing.vertexType();
    expect(address.chunkSize).toBe(Number(CHUNK_SIZE));
    expect(address.tiles).toBe(TILES);
    expect(address.files()).toHaveLength(Number(TILES));
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
    expect(Object.keys(vertex!.fields)).toEqual(['birth_year', 'postcode', 'cluster_id']);
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

/** The subject IRI of one address, read by this file rather than by the code under test. */
const seedOf = async (denseId: number) =>
  String(
    await scalar(`SELECT subject FROM read_parquet(${vertexTiles()}) WHERE dense_id = ${denseId}`),
  );

describe('neighbours — a walk seeded by identity', () => {
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

  it('resolves a batch of seeds in a fixed number of reads, whatever the batch is', async () => {
    const seeds = await Promise.all([1, 2, 3, 4, 5].map(seedOf));
    let queries = 0;
    const counted = await openCorpus(CORPUS, {
      query: async (sql) => {
        queries += 1;
        return query(sql);
      },
    });

    // **The first walk of a corpus pays a sixth read, and every one after it pays five.** That
    // sixth is the index's own footers — one `parquet_metadata` over the index tiles, cached per
    // type exactly as the payload's boxes are — and it is what lets the batched index read name
    // only the tiles its keys can be in. It is a constant per corpus and not a term in the batch,
    // which is the whole reason it is read once and kept rather than folded into the lookup.
    //
    // What the bound is actually for has not moved through either rewrite: **five seeds cost the
    // same reads as one.** The failure it exists to catch is a lookup per seed — the cost this API
    // is most able to multiply, and the shape a per-identity `=` query would have — and no route
    // here has ever paid it. So the assertion below is the one that matters: the warm walk and the
    // cold one differ by the cached footer read, and neither differs by the size of the batch.
    const cold = queries;
    await counted.neighbours(seeds, { depth: 1 });
    expect(queries - cold, 'cold: index footers, index, payload, two orientations, the hop').toBe(6);

    const warm = queries;
    await counted.neighbours(seeds, { depth: 1 });
    const five = queries - warm;
    expect(five, 'warm: the footers are cached').toBeLessThanOrEqual(5);

    // One seed instead of five, on the same warm corpus: the same reads. A batch that cost per
    // identity would fall to a fifth of the five-seed count here, and this is where it would show.
    const one = queries;
    await counted.neighbours(seeds.slice(0, 1), { depth: 1 });
    expect(queries - one, 'one seed costs what five did').toBe(five);
  });

  it('asks only the index tiles whose footers say a key could be in them', async () => {
    // The index is five tiles of 64 sorted by `subject`, with disjoint ranges. Two identities that
    // land in one tile must not read the other four — which is the defect this replaced: DuckDB
    // prunes no disjunction over a VARCHAR column, so `key IN (a, b)` opened all five and only
    // `key = a` opened one.
    const named: string[][] = [];
    const counted = await openCorpus(CORPUS, {
      query: async (sql) => {
        if (sql.includes('/index/')) named.push(sql.match(/index\/tile\d+\.parquet/g) ?? []);
        return query(sql);
      },
    });
    // Sorted lexicographically, `.../person/0` and `.../person/1` are neighbours, so this is the
    // pair most likely to share a tile — and the assertion is a relationship, not a tile number:
    // fewer than every tile, and never fewer than the one holding the answer.
    const pair = [await seedOf(0), await seedOf(1)];
    named.length = 0;
    const found = await Promise.all(pair.map((id) => counted.node(id)));
    expect(found.every((v) => v !== null)).toBe(true);

    // The footer sweep names every index tile once — that is what a sweep is — and the lookups
    // after it name a strict subset. `named[0]` is the sweep; the rest are the reads it decided.
    expect(named[0]).toHaveLength(Number(TILES));
    for (const read of named.slice(1)) {
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
   * rule for choosing. It is the transport now. What it needed and `openCorpus` did not is the
   * bridge asserted below: the verbs' SQL names tables, this corpus is Parquet files, and the door
   * registers the temp views that join the two.
   *
   * **These do not assert what the corpus API asserts elsewhere, on purpose.** A verb reads the
   * MANIFEST's vocabulary and this corpus declares three properties against seven columns on
   * disk, so `read` answers with `subject` alone and `schema` reports no fields at all. That divergence is
   * the reason `openCorpus` describes the bytes instead, and pinning it here is what stops the two
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

  it('reads one vertex the way the verb surface says to — `where: subject = …`', async () => {
    const id = await seedOf(7);
    const { rows } = await corpus.read({ vertex_type: 'Person', where: `subject = ${lit(id)}` });
    expect(rows).toHaveLength(1);
    // The manifest's vocabulary, which is now `subject` and the two quasi-identifiers the
    // declared privacy bound is measured over. `node` on the same identity answers from the
    // BYTES instead, so it has `x`, `y` and `cluster_id` that this does not — and this has a
    // bare `subject` that `node` reports as the identity rather than as a field. The
    // divergence the pair pins is which columns each side can see, not how many: it read one
    // against five before the fixture grew the two, and it is three against seven now.
    expect(rows[0]).toEqual({ subject: id, birth_year: 1955, postcode: 'PC5' });
    const placed = await corpus.node(id);
    expect(placed!.id).toBe(id);
    expect(Object.keys(placed!.fields)).toEqual(['birth_year', 'postcode', 'cluster_id']);
  }, 30_000);

  it('expands over the whole relation, in identities, where neighbours walks tiles', async () => {
    const seed = await seedOf(7);
    const reached = await corpus.expand({ from: [seed], depth: 1 });
    const walked = await corpus.neighbours([seed], { depth: 1 });

    // **They do not answer the same question, and the counts are how you can tell.** `expand`
    // reads the source-ordered relation and walks OUT: seed plus out-neighbours. `neighbours`
    // defaults to both orientations, because an undirected neighbourhood is what a drawing means
    // by one and the corpus stores the adjacency twice so it can have it. Neither is a filter on
    // the other.
    expect(reached.vertices.length).toBeGreaterThan(0);
    expect(reached.vertices.length).toBeLessThan(walked.vertices.length);
    // One answers in IRIs and hops, the other in addresses and positions.
    expect(typeof reached.vertices[0]!.iri).toBe('string');
    expect(typeof walked.vertices[0]!.denseId).toBe('bigint');
    // And only one of them says what it is missing.
    expect(walked.frontier.length).toBeGreaterThan(0);
    expect(walked.complete).toBe(false);
  }, 30_000);

  it('shadows a host table of the same name rather than replacing it', async () => {
    // `Person` is not an unlikely name for a table the host already has. A plain `CREATE OR
    // REPLACE VIEW` would destroy it; a TEMP one is resolved first and dropped with the
    // connection. This is the assertion that keeps the door from writing to a host's catalog.
    // Qualified by CATALOG, not by schema: a temp view lands in `temp.main` and the host's table
    // in `memory.main`, so `main."Person"` names both and the temp one wins. That precedence is
    // the mechanism under test, and it makes the schema qualifier useless for saying which.
    await query(`CREATE OR REPLACE TABLE memory.main."Person" AS SELECT 99 AS host_owned`);
    const own = await openCorpus(CORPUS, { query });
    expect((await own.schema()).vertices[0]!.count).toBe(Number(VERTEX_COUNT));
    // The host's table is untouched, and an unqualified name still reaches the corpus.
    expect(await query(`SELECT host_owned FROM memory.main."Person"`)).toEqual([{ host_owned: 99 }]);
    const [seen] = await query(`SELECT count(*) AS n FROM "Person"`);
    expect(Number(seen!['n'])).toBe(Number(VERTEX_COUNT));
    await query(`DROP TABLE memory.main."Person"`);
  }, 30_000);

  it('boots no WASM for a caller that only draws', async () => {
    // The verbs are the only half that needs the module, so the module is instantiated on the
    // first verb call. A window and an extent must not reach for it — this is asserted as the
    // absence of a `CREATE ... VIEW`, which is the observable half of that boot.
    const statements: string[] = [];
    const drawing = await openCorpus(CORPUS, {
      query: async (sql) => {
        statements.push(sql);
        return query(sql);
      },
    });
    statements.length = 0;
    const box = (await drawing.extent())!;
    await drawing.window({ x: box.minX, y: box.minY, w: 1, h: 1 });
    expect(statements.some((sql) => sql.includes('VIEW'))).toBe(false);
  }, 30_000);
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
      index: { indexed: boolean; same_either_way: string[] };
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

  it('hands back the same vertex with the index and without it', async () => {
    // The one assertion that makes the index an OPTIMISATION rather than a second truth. Nothing
    // else here can see which route ran: both return the same answer by construction, which is
    // exactly the gap `apps/corpus`'s `index-agrees-with-the-payload` names in its own
    // `cannotProve`, closed from the reader's side.
    const dir = mkdtempSync(join(tmpdir(), 'fossil-noindex-'));
    scratch.push(dir);
    cpSync(CORPUS, dir, { recursive: true });
    const yml = join(dir, 'vertex/Person.vertex.yml');
    writeFileSync(yml, readFileSync(yml, 'utf8').replace(/^index:\n(?: {2}.*\n)*/m, ''));

    const scanning = await openCorpus(dir, { query });
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
