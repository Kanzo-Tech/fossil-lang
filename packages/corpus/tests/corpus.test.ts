import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { open } from '../src/index.js';
import type { Manifest } from '../src/manifest.js';
import { duckdb } from './engine.js';

/**
 * The door, run against the checked-in conformance corpus with a real DuckDB-WASM.
 *
 * **Every expectation is a second query, not a constant**: what the views answer is held against the
 * manifest and against SQL this file writes over the same files, because a number transcribed from a
 * run of the code under test agrees with it by construction.
 */

const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const MANIFEST = JSON.parse(readFileSync(join(CORPUS, 'fossil.json'), 'utf8')) as Manifest;
const TABLES = [...MANIFEST.vertex_tables, ...MANIFEST.edge_tables];

let engine: Engine;
let query: (sql: string) => Promise<Record<string, unknown>[]>;
const scratch = mkdtempSync(join(tmpdir(), 'fossil-corpus-test-'));

beforeAll(async () => {
  ({ engine, query } = await duckdb(join(scratch, 'spill')));
}, 60_000);
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const catalogs = async (): Promise<string[]> =>
  (await query('SELECT database_name AS d FROM duckdb_databases()')).map((r) => String(r.d));

/** A corpus directory holding `text` as its `fossil.json` and no Parquet at all. */
function manifestOnly(name: string, text: string): string {
  const dir = join(scratch, name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'fossil.json'), text);
  return dir;
}

/** A `FossilError` of `code`, carrying at least `data`. */
const fossil = (code: string, data?: Record<string, unknown>) =>
  expect.objectContaining({ name: 'FossilError', code, ...(data === undefined ? {} : { data: expect.objectContaining(data) }) });

describe('open', () => {
  it('attaches a view per table and the manifest as two relations, under the name it was given', async () => {
    const close = await open('conformance', { engine, url: CORPUS });
    const views = await query(
      `SELECT table_name AS t FROM information_schema.tables WHERE table_catalog = 'conformance' ORDER BY t`,
    );
    expect(views.map((r) => r.t)).toEqual([...TABLES.map((t) => t.name), 'fossil_columns', 'fossil_tables'].sort());

    const tables = await query(`SELECT * FROM conformance.fossil_tables`);
    expect(tables.map((r) => r.table_name)).toEqual(TABLES.map((t) => t.name));
    expect(tables.map((r) => Number(r.rows))).toEqual(TABLES.map((t) => t.record_count));
    for (const t of MANIFEST.edge_tables) {
      expect(tables.find((r) => r.table_name === t.name)).toMatchObject({
        kind: 'edge', source: t.source.references, destination: t.destination.references, first_id: null,
      });
    }

    const columns = await query(`SELECT table_name, column_name, role FROM conformance.fossil_columns ORDER BY table_name, ordinal`);
    for (const t of TABLES) {
      expect(columns.filter((r) => r.table_name === t.name).map((r) => [r.column_name, r.role ?? undefined])).toEqual(
        t.properties.map((p) => [p.name, p.role]),
      );
    }
    await close();
  });

  it.each(MANIFEST.vertex_tables.map((t) => [t.name] as const))(
    "%s holds its record_count, as the range fossil_tables gives it",
    async (name) => {
      const close = await open('ranges', { engine, url: CORPUS });
      const [range] = await query(
        `SELECT t.first_id::BIGINT AS first, t.rows::BIGINT AS rows, min(v.dense_id)::BIGINT AS lo, max(v.dense_id)::BIGINT AS hi, count(*)::BIGINT AS n
           FROM ranges.fossil_tables t, ranges."${name}" v WHERE t.table_name = '${name}' GROUP BY ALL`,
      );
      expect(range!.n).toBe(range!.rows);
      expect([range!.lo, range!.hi]).toEqual([range!.first, range!.first + range!.rows - 1n]);
      await close();
    },
  );

  it('quotes a name SQL would not take bare', async () => {
    const close = await open('a "quoted" name', { engine, url: CORPUS });
    const [count] = await query(`SELECT count(*)::INTEGER AS n FROM "a ""quoted"" name"."Person"`);
    expect(count!.n).toBe(MANIFEST.vertex_tables.find((t) => t.name === 'Person')!.record_count);
    await close();
  });

  it('refuses a format it does not read before reading a byte of Parquet', async () => {
    const dir = manifestOnly('fossil2', JSON.stringify({ ...MANIFEST, format: 'fossil/2' }));
    await expect(open('fossil2', { engine, url: dir })).rejects.toThrow(fossil('corpus/unsupported-format', { format: 'fossil/2' }));
    expect(await catalogs()).not.toContain('fossil2');
  });

  it('refuses a manifest that is not JSON, and one that is not there', async () => {
    await expect(open('garbage', { engine, url: manifestOnly('garbage', 'format: fossil/1') })).rejects.toThrow(fossil('corpus/not-json'));
    await expect(open('nothing', { engine, url: join(scratch, 'nothing-here') })).rejects.toThrow(
      fossil('corpus/unreadable', { path: join(scratch, 'nothing-here', 'fossil.json') }),
    );
  });

  it('ignores a key it does not know, and an empty corpus is two empty relations', async () => {
    const dir = manifestOnly('extra', JSON.stringify({ ...MANIFEST, vertex_tables: [], edge_tables: [], extent: [0, 0, 1, 1] }));
    const close = await open('extra', { engine, url: dir });
    expect(await query('SELECT * FROM extra.fossil_tables')).toEqual([]);
    expect(await query('SELECT * FROM extra.fossil_columns')).toEqual([]);
    await close();
  });

  it('refuses a manifest that names one table twice, or a table named like its own relations', async () => {
    const twice = manifestOnly('twice', JSON.stringify({ ...MANIFEST, edge_tables: [{ ...MANIFEST.edge_tables[0], name: 'Person' }] }));
    await expect(open('twice', { engine, url: twice })).rejects.toThrow(fossil('corpus/duplicate-table', { table: 'Person' }));
    const reserved = manifestOnly('reserved', JSON.stringify({ ...MANIFEST, edge_tables: [{ ...MANIFEST.edge_tables[0], name: 'fossil_tables' }] }));
    await expect(open('reserved', { engine, url: reserved })).rejects.toThrow(fossil('corpus/duplicate-table', { table: 'fossil_tables' }));
    expect(await catalogs()).not.toContain('twice');
  });

  it('needs an engine, and exactly one of a host and a url', async () => {
    await expect(open('x', { url: CORPUS } as never)).rejects.toThrow(fossil('api/invalid-argument', { argument: 'engine' }));
    await expect(open('x', { engine })).rejects.toThrow(fossil('api/invalid-argument', { argument: 'host' }));
  });

  it('refuses a location carrying a query or a fragment, before the engine sees anything', async () => {
    const asked: string[] = [];
    const untouched = { query: async (sql: string) => void asked.push(sql) } as unknown as Engine;
    for (const location of ['https://acct.blob.core.windows.net/c?sv=x&sig=y', `${CORPUS}#frag`]) {
      await expect(open('x', { engine: untouched, url: location })).rejects.toThrow(fossil('corpus/not-a-location', { location }));
    }
    expect(asked).toEqual([]);
  });

  it('shares a catalog between two opens, and the last to close detaches it', async () => {
    const first = await open('shared', { engine, url: CORPUS });
    const again = await open('shared', { engine, url: CORPUS });
    await first();
    await first();
    const [count] = await query(`SELECT count(*)::INTEGER AS n FROM shared."Person"`);
    expect(count!.n).toBe(MANIFEST.vertex_tables[0]!.record_count);
    expect(await catalogs()).toContain('shared');
    await again();
    expect(await catalogs()).not.toContain('shared');
  });
});
