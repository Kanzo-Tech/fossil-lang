import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { FOSSIL_FORMAT, open, type Manifest } from '../src/index.js';
import { duckdb } from './engine.js';

/**
 * The door, run against the checked-in conformance corpus with a real DuckDB-WASM.
 *
 * **Every expectation is a second query, not a constant**: a count is held against the manifest and
 * against SQL this file writes over the same files, because a number transcribed from a run of the
 * code under test agrees with it by construction.
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
  it('reads fossil.json once and registers a view per table under the corpus', async () => {
    const corpus = await open(CORPUS, { engine });
    expect(corpus.url).toBe(CORPUS);
    expect(corpus.manifest).toEqual(MANIFEST);
    expect(corpus.manifest.format).toBe(FOSSIL_FORMAT);
    const views = await query(
      `SELECT table_name AS t FROM information_schema.tables WHERE table_catalog = '${CORPUS}' ORDER BY t`,
    );
    expect(views.map((r) => r.t)).toEqual(TABLES.map((t) => t.name).sort());
    await corpus.close();
  });

  it.each(TABLES.map((t) => [t.name, t] as const))('%s holds its record_count', async (_, table) => {
    const corpus = await open(CORPUS, { engine });
    const scan = corpus.scan({ table: table.name });
    const [batch] = await scan.read(scan.plan());
    expect(batch!.numRows).toBe(table.record_count);
    const [direct] = await query(`SELECT count(*)::INTEGER AS n FROM read_parquet('${join(CORPUS, table.path)}')`);
    expect(batch!.numRows).toBe(direct!.n);
    await corpus.close();
  });

  it('refuses a format it does not read before reading a byte of Parquet', async () => {
    const dir = manifestOnly('fossil2', JSON.stringify({ ...MANIFEST, format: 'fossil/2' }));
    await expect(open(dir, { engine })).rejects.toThrow(fossil('corpus/unsupported-format', { format: 'fossil/2' }));
    expect(await catalogs()).not.toContain(dir);
  });

  it('refuses a manifest that is not JSON, and one that is not there', async () => {
    await expect(open(manifestOnly('garbage', 'format: fossil/1'), { engine })).rejects.toThrow(fossil('corpus/not-json'));
    await expect(open(join(scratch, 'nothing-here'), { engine })).rejects.toThrow(
      fossil('corpus/unreadable', { path: join(scratch, 'nothing-here', 'fossil.json') }),
    );
  });

  it('ignores a key it does not know', async () => {
    const dir = manifestOnly('extra', JSON.stringify({ ...MANIFEST, vertex_tables: [], edge_tables: [], extent: [0, 0, 1, 1] }));
    const corpus = await open(dir, { engine });
    expect(corpus.manifest.vertex_tables).toEqual([]);
    await corpus.close();
  });

  it('refuses a manifest that names one table twice', async () => {
    const dir = manifestOnly('twice', JSON.stringify({ ...MANIFEST, edge_tables: [{ ...MANIFEST.edge_tables[0], name: 'Person' }] }));
    await expect(open(dir, { engine })).rejects.toThrow(fossil('corpus/duplicate-table', { table: 'Person' }));
    expect(await catalogs()).not.toContain(dir);
  });

  it('needs an engine', async () => {
    await expect(open(CORPUS, {} as never)).rejects.toThrow(fossil('api/invalid-argument', { argument: 'engine' }));
  });

  it('shares a catalog between two opens, and the last to close detaches it', async () => {
    const first = await open(CORPUS, { engine });
    const again = await open(CORPUS, { engine });
    await first.close();
    await first.close();
    const scan = again.scan({ table: 'Person', select: ['dense_id'] });
    expect((await scan.read(scan.plan()))[0]!.numRows).toBe(MANIFEST.vertex_tables[0]!.record_count);
    expect(await catalogs()).toContain(CORPUS);
    await again.close();
    expect(await catalogs()).not.toContain(CORPUS);
  });
});

describe('sql', () => {
  it('is not on a corpus the host did not open it on', async () => {
    const corpus = await open(CORPUS, { engine });
    expect('sql' in corpus).toBe(false);
    await corpus.close();
  });

  it('answers columns and rows over the views, capped at limit', async () => {
    const corpus = await open(CORPUS, { engine, sql: 'allowed' });
    const people = MANIFEST.vertex_tables[0]!;
    const summary = await corpus.sql(`SUMMARIZE "${CORPUS}"."${people.name}";`);
    expect(summary.columns).toContain('column_name');
    expect(summary.rows.map((r) => r[summary.columns.indexOf('column_name')])).toEqual(
      people.properties.map((p) => p.name),
    );
    expect(summary.truncated).toBe(false);

    const capped = await corpus.sql(`SELECT dense_id FROM "${CORPUS}"."${people.name}" ORDER BY dense_id LIMIT 50`, {
      limit: 10,
    });
    expect(capped.rows).toHaveLength(10);
    expect(capped.truncated).toBe(true);
    const first = await query(`SELECT dense_id FROM read_parquet('${join(CORPUS, people.path)}') ORDER BY dense_id LIMIT 10`);
    expect(capped.rows.map((r) => Number(r[0]))).toEqual(first.map((r) => Number(r.dense_id)));

    const exact = await corpus.sql('SELECT 1 AS one', { limit: 1 });
    expect(exact).toEqual({ columns: ['one'], rows: [[1]], truncated: false });
    await corpus.close();
  });

  it("rejects an aborted statement with the signal's reason", async () => {
    const corpus = await open(CORPUS, { engine, sql: 'allowed' });
    const controller = new AbortController();
    controller.abort(new DOMException('stale', 'AbortError'));
    await expect(corpus.sql('SELECT 1', { signal: controller.signal })).rejects.toThrow(/stale/);
    await corpus.close();
  });
});

describe('scan, refused before any statement', () => {
  it('names the tables when the table is not one', async () => {
    const corpus = await open(CORPUS, { engine });
    expect(() => corpus.scan({ table: 'Nobody' })).toThrow(
      fossil('corpus/unknown-table', { table: 'Nobody', tables: expect.arrayContaining(['Person']) }),
    );
    await corpus.close();
  });

  it('refuses a column the table does not declare, an empty projection and a negative limit', async () => {
    const corpus = await open(CORPUS, { engine });
    const unknown = fossil('corpus/unknown-column', { table: 'Person', column: 'nope' });
    expect(() => corpus.scan({ table: 'Person', select: ['nope'] })).toThrow(unknown);
    expect(() => corpus.scan({ table: 'Person', select: [] })).toThrow(fossil('corpus/empty-projection', { table: 'Person' }));
    expect(() => corpus.scan({ table: 'Person', limit: -1 })).toThrow(fossil('api/invalid-argument', { argument: 'limit' }));
    expect(() => corpus.scan({ table: 'Person', filter: { column: 'nope', op: '=', value: 1 } })).toThrow(unknown);
    await corpus.close();
  });

  it('refuses a box over a table with no position', async () => {
    const corpus = await open(CORPUS, { engine });
    const undrawn = MANIFEST.vertex_tables.find((t) => t.position === undefined)!;
    expect(() => corpus.scan({ table: undrawn.name, filter: { bbox: [0, 0, 1, 1] } })).toThrow(
      fossil('corpus/no-position', { table: undrawn.name }),
    );
    await corpus.close();
  });

  it('refuses a task of another table', async () => {
    const corpus = await open(CORPUS, { engine });
    const other = corpus.scan({ table: 'Tag' }).plan();
    await expect(corpus.scan({ table: 'Person' }).read(other)).rejects.toThrow(fossil('api/invalid-argument', { argument: 'tasks' }));
    await corpus.close();
  });
});
