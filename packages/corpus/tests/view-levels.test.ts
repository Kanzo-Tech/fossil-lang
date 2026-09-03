import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { openCorpus, type Corpus } from '../src/corpus.js';
import { initFossilGraphWasm } from '../src/load.js';
import type { QueryFn, QueryRow } from '../src/query.js';

/**
 * **The read side of the pyramid**, against a corpus that has one.
 *
 * `view.test.ts` cannot ask any of this: the conformance corpus is 300 vertices in 5 tiles and the
 * floor is 64 tiles, so it is three orders under the size at which a writer spends bytes on a level
 * — deliberately, because putting it over the floor would mean growing the artefact every
 * implementation of this format is checked against. So the corpus here is BUILT, small, and over
 * the floor by the term that costs nothing: 600 rows at 8 to a tile is 75 tiles, and
 * `VertexLevels::planned` writes 5, 6 and 7 for it.
 *
 * **What is asserted is the one property the pyramid is not allowed to break.** A level file is a
 * cache of `dense_id % 2^k == 0` over the payload and nothing else, so for every written level the
 * rows a level read answers with are exactly the rows the predicate selects out of `window` — same
 * ids, same positions, same order. If those two can disagree there are two contracts, and a reader
 * would have to know which artefact answered in order to know what it was looking at.
 *
 * The cost is asserted too, in the only direction that is a fact rather than a compressor's
 * opinion: a level read opens strictly fewer bytes than the strided read of the same rectangle.
 */

const require = createRequire(import.meta.url);

/** The corpus this file builds. 600 at 8 to a tile is 75 tiles, over the 64-tile floor. */
const ROWS = 600;
const CHUNK = 8;
/** What `VertexLevels::planned(600, 8)` plans, and this fixture therefore writes. */
const LEVELS = [3, 4, 5, 6, 7];

let query: QueryFn;
let corpus: Corpus;
let root: string;
const scratch: string[] = [];

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

/** One tile's worth of rows, as the layout pass would have written them. */
const rowsSql = (where: string, limit: number, offset: number): string =>
  `SELECT id::UINTEGER AS dense_id, 'urn:n' || id AS subject, ` +
  `(id % 32)::FLOAT AS x, (id // 32)::FLOAT AS y, (id % 7)::UINTEGER AS cluster_id ` +
  `FROM range(0, ${ROWS}) t(id) WHERE ${where} ORDER BY id LIMIT ${limit} OFFSET ${offset}`;

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
  const spill = mkdtempSync(join(tmpdir(), 'fossil-levels-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);
  query = async (sql: string): Promise<QueryRow[]> =>
    conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());

  root = mkdtempSync(join(tmpdir(), 'fossil-levels-corpus-'));
  scratch.push(root);
  const type = join(root, 'vertex', 'Node');
  mkdirSync(type, { recursive: true });

  const copy = (sql: string, path: string): void => {
    conn.query(`COPY (${sql}) TO '${path}' (FORMAT PARQUET)`);
  };
  // The payload: one file per tile, `files` container, gapless `dense_id` in Morton order — which
  // for this fixture is `id`, because the ids ARE the order.
  for (let tile = 0; tile * CHUNK < ROWS; tile += 1) {
    copy(rowsSql('TRUE', CHUNK, tile * CHUNK), join(type, `chunk${tile}.parquet`));
  }
  // The pyramid, written exactly as `write_levels` writes it: the rows the predicate selects, in
  // order, cut into tiles of the plan's own `chunk_size`.
  for (const level of LEVELS) {
    const step = 2 ** level;
    const held = Math.ceil(ROWS / step);
    const dir = join(type, `l${level}`);
    mkdirSync(dir, { recursive: true });
    for (let tile = 0; tile * CHUNK < held; tile += 1) {
      copy(rowsSql(`id % ${step} = 0`, CHUNK, tile * CHUNK), join(dir, `chunk${tile}.parquet`));
    }
  }
  writeFileSync(
    join(root, 'graph.graph.yml'),
    `name: levels\nprefix: ''\ncontainer: files\nvertices:\n- vertex/Node.vertex.yml\nedges: []\nversion: gar/v1\n`,
  );
  writeFileSync(
    join(root, 'vertex', 'Node.vertex.yml'),
    `type: Node\nvertex_count: ${ROWS}\nchunk_size: ${CHUNK}\nprefix: vertex/Node/\n` +
      `property_groups:\n- file_type: parquet\n  properties:\n  - name: subject\n    data_type: string\n    is_primary: true\n` +
      `levels:\n  prefix: l\n${LEVELS.map((l) => `  - ${l}\n`).join('').replace(/^/, '  levels:\n')}` +
      `  chunk_size: ${CHUNK}\nversion: gar/v1\n`,
  );

  await initFossilGraphWasm({
    wasmUrl: (await readFile(
      fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
    )) as unknown as URL,
  });
  corpus = await openCorpus(root, { query });
}, 120_000);

/** The whole corpus as a rectangle, from the footers rather than from a constant. */
async function everything() {
  const extent = (await corpus.extent())!;
  const pad = 1e-3;
  return {
    x: extent.minX - pad,
    y: extent.minY - pad,
    w: extent.maxX - extent.minX + 2 * pad,
    h: extent.maxY - extent.minY + 2 * pad,
  };
}

describe('a corpus that declares a pyramid', () => {
  it('reports exactly the written levels, and every other level as answerable anyway', () => {
    const written = corpus.levels('Node').filter((l) => l.written);
    expect(written.map((l) => l.level)).toEqual(LEVELS);
    // Every level is in the list, written or not: a level is a predicate, and `written` is a cost.
    const all = corpus.levels('Node');
    expect(all[0]!.level).toBe(0);
    expect(all[0]!.written).toBe(false);
    expect(all[5]!.count).toBe(Math.ceil(ROWS / 32));
  });
});

describe('a level read answers with what the predicate selects', () => {
  it('is the same rows as striding the payload, at every written level', async () => {
    const box = await everything();
    const rows = await corpus.window({ ...box });
    for (const level of LEVELS) {
      const step = 2 ** level;
      const expected = rows.vertices
        .map((v) => v.denseId)
        .filter((id) => id % BigInt(step) === 0n)
        .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      const view = await corpus.view({ ...box, level, links: false });
      expect(view.cost.read).toBe('level');
      expect([...view.denseIds.slice(0, view.marks)]).toEqual(expected);
      // The positions come out of a different FILE and have to be the same numbers: a level that
      // carried the right ids at the wrong coordinates draws a picture that is wrong about where
      // everything is, and an id-only assertion is green for it.
      for (let i = 0; i < view.marks; i += 1) {
        const id = view.denseIds[i]!;
        const row = rows.vertices.find((v) => v.denseId === id)!;
        expect(view.positions[i * 2]).toBeCloseTo(row.x, 5);
        expect(view.positions[i * 2 + 1]).toBeCloseTo(row.y, 5);
      }
    }
  });

  it('opens strictly fewer bytes than striding the same rectangle for the same rows', async () => {
    const box = await everything();
    const level = LEVELS[0]!;
    const cheap = await corpus.view({ ...box, level, links: false });
    // The same rows, from the payload: asking for links is what puts this read on the payload, so
    // it is also the comparison that isolates the bytes from the answer.
    const dear = await corpus.view({ ...box, level, links: true });
    expect(cheap.cost.read).toBe('level');
    expect(dear.cost.read).toBe('strided');
    expect([...cheap.denseIds.slice(0, cheap.marks)]).toEqual([
      ...dear.denseIds.slice(0, dear.marks),
    ]);
    expect(cheap.cost.bytes).toBeLessThan(dear.cost.bytes);
    expect(cheap.cost.tiles).toBeLessThan(dear.cost.tiles);
  });

  it('counts `matched` at the level it read, and says which level that is', async () => {
    const box = await everything();
    const level = LEVELS[1]!;
    const cheap = await corpus.view({ ...box, level, links: false });
    const strided = await corpus.view({ ...box, level, links: true });
    expect(cheap.matchedAt).toBe(level);
    expect(cheap.matched).toBe(Math.ceil(ROWS / 2 ** level));
    // The payload read still counts level 0, because it read level 0's bytes.
    expect(strided.matchedAt).toBe(0);
    expect(strided.matched).toBe(ROWS);
  });

  it('strides the payload for a level nobody wrote, and says so', async () => {
    const box = await everything();
    const view = await corpus.view({ ...box, level: 2, links: false });
    expect(view.cost.read).toBe('strided');
    expect(view.matchedAt).toBe(0);
    expect(view.marks).toBe(Math.ceil(ROWS / 4));
  });

  it('brings a pin back off the payload, because no level carries an odd id', async () => {
    const box = await everything();
    const level = LEVELS[0]!;
    const bare = await corpus.view({ ...box, level, links: false });
    const pinned = await corpus.view({ ...box, level, links: false, pinned: [7] });
    expect(pinned.cost.read).toBe('level');
    expect([...pinned.denseIds.slice(0, pinned.marks)]).toContain(7n);
    expect(pinned.marks).toBe(bare.marks + 1);
    // The pin's PAYLOAD tile was opened for it, and a fetch that happened is a fetch on the ledger.
    expect(pinned.cost.bytes).toBeGreaterThan(bare.cost.bytes);
  });
});
