import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import './boot.js';
import { duckdb } from './engine.js';
import { CorpusReadError, open, type Batch, type Box, type Corpus, type Filter } from '../src/index.js';
import type { QueryFn } from '../src/query.js';

// `tileMatrix`, `scan` and `edges` over the checked-in conformance corpus — 300 `Person`s in five
// file-per-tile tiles of 64, one self-relation stored twice. Every expectation is a second query
// this file writes against the Parquet, never the reader's own arithmetic.

const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const spill = mkdtempSync(join(tmpdir(), 'fossil-scan-spill-'));
let engine: Engine;
let query: QueryFn;
let corpus: Corpus;

beforeAll(async () => {
  ({ engine, query } = await duckdb(spill));
  corpus = await open(CORPUS, { engine });
}, 60_000);

afterAll(() => rmSync(spill, { recursive: true, force: true }));

const lit = (s: string) => `'${s.replace(/'/g, "''")}'`;
const tiles = (dir: string) => `[${[0, 1, 2, 3, 4].map((k) => lit(`${CORPUS}/${dir}/chunk${k}.parquet`)).join(', ')}]`;
const VERTICES = tiles('vertex/Person');
const column = (batch: Batch, name: string): bigint[] =>
  Array.from(batch.getChild(name)!.toArray(), (v) => BigInt(v as number | bigint)).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
const ids = async (sql: string): Promise<bigint[]> =>
  (await query(sql)).map((r) => BigInt(Object.values(r)[0] as number | bigint)).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
const inBox = ({ x, y, w, h }: Box): Filter => ({
  and: [
    { column: 'x', op: '>=', value: x },
    { column: 'x', op: '<', value: x + w },
    { column: 'y', op: '>=', value: y },
    { column: 'y', op: '<', value: y + h },
  ],
});

describe('tileMatrix', () => {
  it('lists every tile of the payload with its rows and its box, as the footers have them', async () => {
    const set = corpus.tileMatrix('Person');
    expect(set.tileMatrices).toHaveLength(1);
    const [payload] = set.tileMatrices;
    expect(payload).toMatchObject({ z: 0, kind: 'rows', count: 300n, shift: 0, tileRows: 64 });
    const truth = await query(
      `SELECT dense_id // 64 AS t, count(*) AS n, min(x) AS x0, max(x) AS x1, min(y) AS y0, max(y) AS y1
         FROM read_parquet(${VERTICES}) GROUP BY 1 ORDER BY 1`,
    );
    expect(payload!.tiles.map((t) => [t.tile, t.rows])).toEqual(truth.map((r) => [Number(r['t']), Number(r['n'])]));
    for (const [k, t] of payload!.tiles.entries()) {
      const r = truth[k]!;
      expect(t.bbox).toEqual({ x: r['x0'], y: r['y0'], w: Number(r['x1']) - Number(r['x0']), h: Number(r['y1']) - Number(r['y0']) });
    }
    expect(set.extent!.x).toBe(Math.min(...truth.map((r) => Number(r['x0']))));
  });

  it('refuses an address outside the matrix, as OGC answers one with a 404', async () => {
    const scan = corpus.scan({ type: 'Person' });
    await expect(scan.read([{ type: 'Person', z: 0, tile: 5 }])).rejects.toThrow(CorpusReadError);
    await expect(scan.read([{ type: 'Person', z: 1, tile: 0 }])).rejects.toThrow(/z = 0 to 0/);
  });
});

describe('scan', () => {
  it('reads every row once over the whole plan, and a tile at most tileRows of them', async () => {
    const scan = corpus.scan({ type: 'Person', select: ['dense_id', 'x'] });
    const plan = scan.plan();
    expect(plan.map((t) => [t.tile, t.residual])).toEqual([0, 1, 2, 3, 4].map((t) => [t, null]));
    const batches = await scan.read(plan);
    expect(batches.map((b) => b.numRows)).toEqual([64, 64, 64, 64, 44]);
    expect(batches[0]!.getChild('subject')).toBeNull();
    expect(batches.flatMap((b) => column(b, 'dense_id'))).toEqual(Array.from({ length: 300 }, (_, i) => BigInt(i)));
  });

  it('at Z, selects what the payload holds inside the box — the conformance obligation', async () => {
    const extent = corpus.tileMatrix('Person').extent!;
    const boxes: Box[] = [
      { x: 0, y: 0, w: 100, h: 100 },
      { x: extent.x, y: extent.y, w: extent.w / 3, h: extent.h / 2 },
      { x: 120, y: 60, w: 30, h: 250 },
      { x: 1e6, y: 1e6, w: 1, h: 1 },
    ];
    for (const box of boxes) {
      const scan = corpus.scan({ type: 'Person', filter: inBox(box), select: ['dense_id'] });
      const read = (await scan.read(scan.plan())).flatMap((b) => column(b, 'dense_id'));
      // The oracle is SQL over every payload file, with no addressing and no statistics: the
      // half-open box `x <= v.x < x + w`, as a full scan reads it.
      const truth = await ids(
        `SELECT dense_id FROM read_parquet(${VERTICES})
          WHERE x >= ${box.x} AND x < ${box.x + box.w} AND y >= ${box.y} AND y < ${box.y + box.h}`,
      );
      expect(read).toEqual(truth);
    }
  });

  it('plans only the tiles whose bounds admit the filter, and prunes the rest without a byte', async () => {
    const counted = async (filter: Filter, where: string) => {
      const planned = corpus.scan({ type: 'Person', filter }).plan().map((t) => t.tile);
      const admitted = (
        await query(
          `SELECT dense_id // 64 AS t FROM read_parquet(${VERTICES}) GROUP BY 1
            HAVING ${where} ORDER BY 1`,
        )
      ).map((r) => Number(r['t']));
      expect(planned).toEqual(admitted);
      return planned.length;
    };
    // `cluster_id` is the column the page names: communities are placed together.
    expect(await counted({ column: 'cluster_id', op: '=', value: 13 }, 'min(cluster_id) <= 13 AND max(cluster_id) >= 13')).toBeLessThan(5);
    expect(await counted({ column: 'dense_id', op: 'in', values: [3, 250] }, 'min(dense_id) <= 250 AND max(dense_id) >= 3 AND (min(dense_id) <= 3 AND max(dense_id) >= 3 OR min(dense_id) <= 250 AND max(dense_id) >= 250)')).toBe(2);
    expect(await counted({ column: 'birth_year', op: 'is null' }, 'count(*) > count(birth_year)')).toBe(0);
  });

  it('carries a residual only where the statistics could not settle the filter', async () => {
    const plan = corpus.scan({ type: 'Person', filter: { and: [{ column: 'dense_id', op: '>=', value: 100 }, { column: 'dense_id', op: '<', value: 200 }] } }).plan();
    expect(plan.map((t) => [t.tile, t.residual])).toEqual([
      [1, { column: 'dense_id', op: '>=', value: 100 }],
      [2, null],
      [3, { column: 'dense_id', op: '<', value: 200 }],
    ]);
  });

  it('binds when it is built: an unknown column fails in scan, not in a read', () => {
    expect(() => corpus.scan({ type: 'Person', filter: { column: 'nope', op: '=', value: 1 } })).toThrow(CorpusReadError);
    expect(() => corpus.scan({ type: 'Person', select: ['nope'] })).toThrow(/select names nope/);
  });

  it('rejects with AbortError when the read is aborted, and hands the signal to the engine', async () => {
    const seen: (AbortSignal | undefined)[] = [];
    const slow: Engine = {
      ...engine,
      async query(sql, options) {
        seen.push(options?.signal);
        await new Promise((r) => setTimeout(r, 50));
        options?.signal?.throwIfAborted();
        return engine.query(sql, options);
      },
    };
    const held = await open(CORPUS, { engine: slow });
    const scan = held.scan({ type: 'Person' });
    const controller = new AbortController();
    const reading = scan.read([{ type: 'Person', z: 0, tile: 1 }], { signal: controller.signal });
    setTimeout(() => controller.abort(), 10);
    await expect(reading).rejects.toMatchObject({ name: 'AbortError' });
    expect(seen.at(-1)).toBe(controller.signal);
    const [again] = await scan.read([{ type: 'Person', z: 0, tile: 1 }]);
    expect(again!.numRows).toBe(64);
    const aborted = AbortSignal.abort();
    await expect(scan.read([{ type: 'Person', z: 0, tile: 1 }], { signal: aborted })).rejects.toMatchObject({ name: 'AbortError' });
  });
});

describe('read coalesces a run of consecutive tiles into one statement', () => {
  const counting = () => {
    const sent: string[] = [];
    const counted: Engine = {
      ...engine,
      query(sql, options) {
        sent.push(sql);
        return engine.query(sql, options);
      },
    };
    return { sent, counted };
  };
  const at = (tile: number) => ({ type: 'Person', z: 0, tile });
  const values = (batch: Batch, name: string) => Array.from(batch.getChild(name)!.toArray(), (v) => String(v));
  /** One conjunctive range on the aligned column and no disjunction — the shape the engine prunes. */
  const oneRange = (sql: string) => {
    expect(sql.match(/"?dense_id"? >= \d+ AND "?dense_id"? < \d+/g)).toHaveLength(1);
    expect(sql).not.toMatch(/\bOR\b/);
  };

  it('reads consecutive tiles in one statement and scattered ones in one each', async () => {
    const { sent, counted } = counting();
    const scan = (await open(CORPUS, { engine: counted })).scan({ type: 'Person', select: ['x'] });
    sent.length = 0;
    await scan.read([at(3), at(1), at(2)]);
    expect(sent).toHaveLength(1);
    sent.forEach(oneRange);
    sent.length = 0;
    await scan.read([at(0), at(2), at(4)]);
    expect(sent).toHaveLength(3);
    sent.forEach(oneRange);
    sent.length = 0;
    await scan.read([at(0), at(1), at(3), at(4), at(1)]);
    expect(sent).toHaveLength(2);
  });

  it('splits a run into the batch each tile answers alone, in the order asked', async () => {
    for (const select of [['dense_id', 'x'], ['x', 'y']]) {
      const scan = corpus.scan({ type: 'Person', select });
      const order = [4, 0, 2, 1, 3, 2].map(at);
      const run = await scan.read(order);
      expect(run).toHaveLength(order.length);
      for (const [k, address] of order.entries()) {
        const [alone] = await scan.read([address]);
        expect(run[k]!.numRows).toBe(alone!.numRows);
        for (const name of select) expect(values(run[k]!, name)).toEqual(values(alone!, name));
        if (!select.includes('dense_id')) expect(run[k]!.getChild('dense_id')).toBeNull();
      }
    }
  });

  it('keeps a run whole when its tiles carry different residuals, and each tile its own rows', async () => {
    const { sent, counted } = counting();
    const filter: Filter = { and: [{ column: 'dense_id', op: '>=', value: 100 }, { column: 'dense_id', op: '<', value: 200 }] };
    const scan = (await open(CORPUS, { engine: counted })).scan({ type: 'Person', filter, select: ['dense_id'] });
    sent.length = 0;
    const batches = await scan.read([at(1), at(2), at(3)]);
    expect(sent).toHaveLength(1);
    expect(batches.map((b) => b.numRows)).toEqual([28, 64, 8]);
    expect(batches.flatMap((b) => column(b, 'dense_id'))).toEqual(Array.from({ length: 100 }, (_, i) => BigInt(100 + i)));
  });

  it('answers a tile its statistics exclude with no request, and that tile breaks the run', async () => {
    const { sent, counted } = counting();
    const scan = (await open(CORPUS, { engine: counted })).scan({
      type: 'Person',
      filter: { or: [{ column: 'dense_id', op: '<', value: 100 }, { column: 'dense_id', op: '>=', value: 256 }] },
      select: ['dense_id'],
    });
    sent.length = 0;
    const batches = await scan.read([at(0), at(1), at(2), at(3), at(4)]);
    expect(batches.map((b) => b.numRows)).toEqual([64, 36, 0, 0, 44]);
    expect(sent).toHaveLength(2);
  });

  it('rejects the whole read with AbortError when aborted mid-run, and the engine answers the next', async () => {
    const slow: Engine = {
      ...engine,
      async query(sql, options) {
        await new Promise((r) => setTimeout(r, 50));
        options?.signal?.throwIfAborted();
        return engine.query(sql, options);
      },
    };
    const scan = (await open(CORPUS, { engine: slow })).scan({ type: 'Person' });
    const controller = new AbortController();
    const reading = scan.read([at(0), at(1), at(2), at(4)], { signal: controller.signal });
    setTimeout(() => controller.abort(), 10);
    await expect(reading).rejects.toMatchObject({ name: 'AbortError' });
    const again = await scan.read([at(0), at(1), at(2), at(4)]);
    expect(again.map((b) => b.numRows)).toEqual([64, 64, 64, 44]);
  });
});

describe('the engine interrupts the statement it is running', () => {
  it('stops a statement mid-flight, rejects with AbortError, and answers the next one', async () => {
    const controller = new AbortController();
    const started = performance.now();
    const running = engine.query('SELECT count(*) AS n FROM range(20000000000) t(i) WHERE i % 7 = 3', {
      signal: controller.signal,
    });
    setTimeout(() => controller.abort(), 50);
    await expect(running).rejects.toMatchObject({ name: 'AbortError' });
    // Twenty billion rows take minutes; an interrupt that only dropped the answer would take them.
    expect(performance.now() - started).toBeLessThan(5_000);
    const next = await engine.query('SELECT 42 AS x');
    expect(next.getChild('x')!.get(0)).toBe(42);
  }, 30_000);
});

describe('edges', () => {
  const adjacency = async (dir: 'by_source' | 'by_target', key: string, tile: number) =>
    (
      await query(
        `SELECT src_dense AS s, dst_dense AS d FROM read_parquet(${tiles(`edge/Person_knows_Person/${dir}`)})
          WHERE ${key} // 64 = ${tile} ORDER BY 1, 2`,
      )
    ).map((r) => `${r['s']}→${r['d']}`);
  const pairs = (src: BigUint64Array, dst: BigUint64Array) =>
    Array.from(src, (s, i) => `${s}→${dst[i]}`).sort((a, b) => {
      const [as, ad] = a.split('→').map(BigInt);
      const [bs, bd] = b.split('→').map(BigInt);
      return as! !== bs! ? (as! < bs! ? -1 : 1) : ad! < bd! ? -1 : ad! > bd! ? 1 : 0;
    });

  it('reads the source-aligned half out of the tile at src, and the target-aligned one at dst', async () => {
    for (const tile of [0, 2, 4]) {
      const out = (await corpus.edges({ from: [{ type: 'Person', z: 0, tile }], direction: 'src' }))[0]!;
      const into = (await corpus.edges({ from: [{ type: 'Person', z: 0, tile }], direction: 'dst' }))[0]!;
      expect(out.declined).toEqual([]);
      expect(out.batches.map((b) => [b.edgeType, b.srcType, b.dstType, b.weight])).toEqual([['knows', 'Person', 'Person', null]]);
      expect(pairs(out.batches[0]!.src, out.batches[0]!.dst)).toEqual(await adjacency('by_source', 'src_dense', tile));
      expect(pairs(into.batches[0]!.src, into.batches[0]!.dst)).toEqual(await adjacency('by_target', 'dst_dense', tile));
    }
  });

  it('reads a run of tiles in one statement per half, and answers each tile as it answers alone', async () => {
    const sent: string[] = [];
    const counted = await open(CORPUS, {
      engine: {
        ...engine,
        query(sql, options) {
          sent.push(sql);
          return engine.query(sql, options);
        },
      },
    });
    const from = [3, 0, 1, 2, 4].map((tile) => ({ type: 'Person', z: 0, tile }));
    for (const direction of ['src', 'dst'] as const) {
      sent.length = 0;
      const answers = await counted.edges({ from, direction });
      expect(sent).toHaveLength(1);
      for (const [k, address] of from.entries()) {
        expect(answers[k]).toEqual((await counted.edges({ from: [address], direction }))[0]);
      }
    }
  });

  it('declines a relation it cannot read, with fossil’s reason', async () => {
    await expect(corpus.edges({ from: [{ type: 'Person', z: 0, tile: 0 }], direction: 'src', relation: 'likes' })).rejects.toThrow(
      /likes is not a relation incident to Person/,
    );
  });
});
