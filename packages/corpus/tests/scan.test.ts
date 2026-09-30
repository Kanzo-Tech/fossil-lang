import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { bind, sqlOf } from '../src/filter.js';
import { open, type Corpus, type Filter, type Manifest } from '../src/index.js';
import { duckdb } from './engine.js';

/**
 * `scan` against the conformance corpus: every filter's answer is held against SQL this file writes
 * over the same file, never against a number a run of the code produced.
 */

const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const MANIFEST = JSON.parse(readFileSync(join(CORPUS, 'fossil.json'), 'utf8')) as Manifest;
const PERSON = MANIFEST.vertex_tables.find((t) => t.name === 'Person')!;
const FILE = `read_parquet('${join(CORPUS, PERSON.path)}')`;

let engine: Engine;
let query: (sql: string) => Promise<Record<string, unknown>[]>;
let corpus: Corpus;
const spill = mkdtempSync(join(tmpdir(), 'fossil-scan-spill-'));

beforeAll(async () => {
  ({ engine, query } = await duckdb(spill));
  corpus = await open(CORPUS, { engine });
}, 60_000);
afterAll(async () => {
  await corpus.close();
  rmSync(spill, { recursive: true, force: true });
});

const count = async (where: string): Promise<number> =>
  Number((await query(`SELECT count(*) AS n FROM ${FILE} WHERE ${where}`))[0]!.n);

const rows = async (filter: Filter): Promise<number> => {
  const scan = corpus.scan({ table: 'Person', filter, select: ['dense_id'] });
  return (await scan.read(scan.plan()))[0]!.numRows;
};

describe('a filter selects what SQL over the file selects', () => {
  const extent = async () =>
    (await query(`SELECT min(x) AS x0, max(x) AS x1, min(y) AS y0, max(y) AS y1 FROM ${FILE}`))[0] as Record<
      'x0' | 'x1' | 'y0' | 'y1',
      number
    >;

  it.each<[string, Filter, string]>([
    ['=', { column: 'cluster_id', op: '=', value: 3 }, 'cluster_id = 3'],
    ['<', { column: 'dense_id', op: '<', value: 100n }, 'dense_id < 100'],
    ['in', { column: 'cluster_id', op: 'in', values: [1, 2, 5] }, 'cluster_id IN (1, 2, 5)'],
    ['not in, empty', { column: 'cluster_id', op: 'not in', values: [] }, 'cluster_id IS NOT NULL'],
    ['is null', { column: 'birth_year', op: 'is null' }, 'birth_year IS NULL'],
    ['!= leaves nulls out', { column: 'birth_year', op: '!=', value: 1950 }, 'birth_year <> 1950'],
    ['not over an and', { not: { and: [{ column: 'cluster_id', op: '>=', value: 4 }, { column: 'birth_year', op: '<', value: 1980 }] } }, 'cluster_id < 4 OR birth_year >= 1980'],
    ['a string', { column: 'postcode', op: '=', value: 'PC0007' }, "postcode = 'PC0007'"],
  ])('%s', async (_, filter, where) => {
    expect(await rows(filter)).toBe(await count(where));
  });

  it('bbox is closed on every edge: the whole extent is every drawn vertex', async () => {
    const { x0, x1, y0, y1 } = await extent();
    expect(await rows({ bbox: [x0, y0, x1, y1] })).toBe(PERSON.record_count);
  });

  it('bbox keeps what lies inside, and not bbox the rest', async () => {
    const { x0, x1, y0, y1 } = await extent();
    const box: [number, number, number, number] = [x0, y0, (x0 + x1) / 2, (y0 + y1) / 2];
    const inside = await count(`x >= ${box[0]} AND x <= ${box[2]} AND y >= ${box[1]} AND y <= ${box[3]}`);
    expect(inside).toBeGreaterThan(0);
    expect(inside).toBeLessThan(PERSON.record_count);
    expect(await rows({ bbox: box })).toBe(inside);
    expect(await rows({ not: { bbox: box } })).toBe(PERSON.record_count - inside);
  });

  it('limit caps the rows and select names the columns', async () => {
    const scan = corpus.scan({ table: 'Person', select: ['x', 'dense_id'], limit: 7 });
    const [batch] = await scan.read(scan.plan());
    expect(batch!.numRows).toBe(7);
    expect(batch!.getChild('x')).not.toBeNull();
    expect(batch!.getChild('subject')).toBeNull();
  });

  it('reads an edge table like any other', async () => {
    const knows = MANIFEST.edge_tables.find((t) => t.label === 'knows')!;
    const scan = corpus.scan({ table: knows.name, select: ['src', 'dst'], filter: { column: 'src', op: '<', value: 10 } });
    const [batch] = await scan.read(scan.plan());
    const direct = await query(`SELECT count(*) AS n FROM read_parquet('${join(CORPUS, knows.path)}') WHERE src < 10`);
    expect(batch!.numRows).toBe(Number(direct[0]!.n));
    expect(ArrayBuffer.isView(batch!.getChild('src')!.toArray())).toBe(true);
  });

  it('plans one task, the table itself', () => {
    expect(corpus.scan({ table: 'Person' }).plan()).toEqual([
      { table: 'Person', path: PERSON.path, rows: PERSON.record_count },
    ]);
  });
});

describe('the SQL a filter becomes', () => {
  const columns = { table: 'Person', properties: PERSON.properties, position: PERSON.position };

  it('never casts the column, so DuckDB can prune on it', () => {
    const sql = sqlOf(bind({ bbox: [0, 1, 2, 3] }, columns));
    expect(sql).not.toMatch(/::|CAST/i);
    expect(sql).toContain('"x" >= 0');
    expect(sql).toContain('"y" <= 3');
  });

  it('keeps NaN out of > and >= on a float, as no comparison matches one', () => {
    expect(sqlOf(bind({ column: 'x', op: '>', value: 1 }, columns))).toBe('("x" > 1 AND NOT isnan("x"))');
    expect(sqlOf(bind({ column: 'dense_id', op: '>', value: 1 }, columns))).toBe('"dense_id" > 1');
  });

  it('refuses a NaN, a string against a number and a box turned inside out', () => {
    expect(() => bind({ column: 'x', op: '=', value: Number.NaN }, columns)).toThrow(/NaN/);
    expect(() => bind({ column: 'dense_id', op: '=', value: '3' }, columns)).toThrow(/holds numbers/);
    expect(() => bind({ bbox: [2, 0, 1, 1] }, columns)).toThrow(/x0 ≤ x1/);
  });
});
