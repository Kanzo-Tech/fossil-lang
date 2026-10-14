import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { attach } from '../src/index.js';
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
const TABLES = [...MANIFEST.vertex_tables, ...MANIFEST.edge_tables, ...(MANIFEST.property_tables ?? [])];

/** The relations every attach makes beside the tables' views. */
const RELATIONS = ['fossil_annotations', 'fossil_columns', 'fossil_stats', 'fossil_tables', 'triples'];

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

describe('attach', () => {
  it('attaches a view per table, the manifest as two relations and the corpus as triples, under the name it was given', async () => {
    const corpus = await attach('conformance', { engine, url: CORPUS });
    const views = await query(
      `SELECT table_name AS t FROM information_schema.tables WHERE table_catalog = 'conformance' ORDER BY t`,
    );
    expect(views.map((r) => r.t)).toEqual(
      [...TABLES.map((t) => t.name), ...RELATIONS].sort(),
    );

    const tables = await query(`SELECT * FROM conformance.fossil_tables`);
    expect(tables.map((r) => r.table_name)).toEqual(TABLES.map((t) => t.name));
    expect(tables.map((r) => Number(r.record_count))).toEqual(TABLES.map((t) => t.record_count));
    for (const t of MANIFEST.edge_tables) {
      expect(tables.find((r) => r.table_name === t.name)).toMatchObject({
        kind: 'edge', source: t.source.references, destination: t.destination.references, first_id: null,
      });
    }
    for (const t of MANIFEST.property_tables ?? []) {
      expect(tables.find((r) => r.table_name === t.name)).toMatchObject({
        kind: 'property', source: t.source.references, destination: null, first_id: null,
      });
    }

    const columns = await query(`SELECT table_name, column_name, role FROM conformance.fossil_columns ORDER BY table_name, ordinal_position`);
    for (const t of TABLES) {
      expect(columns.filter((r) => r.table_name === t.name).map((r) => [r.column_name, r.role ?? undefined])).toEqual(
        t.properties.map((p) => [p.name, p.role]),
      );
    }
    await corpus.detach();
  });

  it.each(MANIFEST.vertex_tables.map((t) => [t.name] as const))(
    "%s holds its record_count, as the range fossil_tables gives it",
    async (name) => {
      const corpus = await attach('ranges', { engine, url: CORPUS });
      const [range] = await query(
        `SELECT t.first_id::BIGINT AS first, t.record_count::BIGINT AS record_count, min(v.dense_id)::BIGINT AS lo, max(v.dense_id)::BIGINT AS hi, count(*)::BIGINT AS n
           FROM ranges.fossil_tables t, ranges."${name}" v WHERE t.table_name = '${name}' GROUP BY ALL`,
      );
      expect(range!.n).toBe(range!.record_count);
      expect([range!.lo, range!.hi]).toEqual([range!.first, range!.first + range!.record_count - 1n]);
      await corpus.detach();
    },
  );

  it('lists the sources each table was derived from, as the manifest names them', async () => {
    const dir = join(scratch, 'lineage');
    cpSync(CORPUS, dir, { recursive: true });
    const [person, ...others] = MANIFEST.vertex_tables;
    const sources = ['@lake/people.csv', 'tags.csv'];
    writeFileSync(join(dir, 'fossil.json'), JSON.stringify({ ...MANIFEST, vertex_tables: [{ ...person!, derived_from: sources }, ...others] }));
    const corpus = await attach('lineage', { engine, url: dir });
    const tables = await query(`SELECT table_name, derived_from FROM lineage.fossil_tables`);
    expect(tables.map((r) => [r.table_name, Array.from(r.derived_from as Iterable<unknown>, String)])).toEqual(
      TABLES.map((t) => [t.name, t.name === person!.name ? sources : []]),
    );
    await corpus.detach();
  });

  it('detaches through Symbol.asyncDispose, the hook `await using` calls', async () => {
    const corpus = await attach('disposed', { engine, url: CORPUS });
    expect(corpus.name).toBe('disposed');
    await corpus[Symbol.asyncDispose]();
    expect(await catalogs()).not.toContain('disposed');
  });

  it('quotes a name SQL would not take bare', async () => {
    const corpus = await attach('a "quoted" name', { engine, url: CORPUS });
    const [count] = await query(`SELECT count(*)::INTEGER AS n FROM "a ""quoted"" name"."Person"`);
    expect(count!.n).toBe(MANIFEST.vertex_tables.find((t) => t.name === 'Person')!.record_count);
    await corpus.detach();
  });

  it('refuses a format it does not read before reading a byte of Parquet', async () => {
    const dir = manifestOnly('fossil2', JSON.stringify({ ...MANIFEST, format: 'fossil/2' }));
    await expect(attach('fossil2', { engine, url: dir })).rejects.toThrow(fossil('corpus/unsupported-format', { format: 'fossil/2' }));
    expect(await catalogs()).not.toContain('fossil2');
  });

  it('refuses a manifest that is not JSON, and one that is not there', async () => {
    await expect(attach('garbage', { engine, url: manifestOnly('garbage', 'format: fossil/1') })).rejects.toThrow(fossil('corpus/not-json'));
    await expect(attach('nothing', { engine, url: join(scratch, 'nothing-here') })).rejects.toThrow(
      fossil('corpus/unreadable', { path: join(scratch, 'nothing-here', 'fossil.json') }),
    );
  });

  it('ignores a key it does not know, and an empty corpus is empty relations', async () => {
    const dir = manifestOnly('extra', JSON.stringify({ ...MANIFEST, vertex_tables: [], edge_tables: [], property_tables: [], extent: [0, 0, 1, 1] }));
    const corpus = await attach('extra', { engine, url: dir });
    expect(await query('SELECT * FROM extra.fossil_tables')).toEqual([]);
    expect(await query('SELECT * FROM extra.fossil_columns')).toEqual([]);
    expect(await query('SELECT * FROM extra.triples')).toEqual([]);
    for (const relation of ['fossil_annotations', 'fossil_stats']) {
      expect(await query(`SELECT * FROM extra.${relation}`)).toEqual([]);
    }
    await corpus.detach();
  });

  it("carries every field of the manifest: an edge's label and keys, a column's term type, the annotations, the counts", async () => {
    const dir = join(scratch, 'facts');
    cpSync(CORPUS, dir, { recursive: true });
    const [person, ...others] = MANIFEST.vertex_tables;
    const label = 'http://www.w3.org/2000/01/rdf-schema#label';
    const named = {
      ...person!,
      annotations: [{ predicate: label, object: { value: 'persona', language: 'es' } }],
      candidate_keys: ['dense_id', 'subject'],
      properties: person!.properties.map((p, at) =>
        at === 0
          ? { ...p, stats: { null_count: 0, distinct_count: 3, min_value: '0', max_value: '2', most_common_vals: ['1'], most_common_freqs: [0.5] } }
          : at === 1
            ? { ...p, annotations: [{ predicate: label, object: 'https://example.org/Identity' }] }
            : p,
      ),
    };
    writeFileSync(join(dir, 'fossil.json'), JSON.stringify({ ...MANIFEST, vertex_tables: [named, ...others] }));
    const corpus = await attach('facts', { engine, url: dir });

    const tables = await query(`SELECT table_name, label, source_key, destination_key, candidate_keys FROM facts.fossil_tables`);
    for (const e of MANIFEST.edge_tables) {
      expect(tables.find((r) => r.table_name === e.name)).toMatchObject({
        label: e.label, source_key: e.source.key, destination_key: e.destination.key,
      });
    }
    expect(Array.from(tables[0]!.candidate_keys as Iterable<unknown>, String)).toEqual(['dense_id', 'subject']);
    const values = MANIFEST.property_tables?.[0];
    if (values !== undefined) {
      expect(tables.find((r) => r.table_name === values.name)).toMatchObject({
        label: values.properties.find((p) => p.role === undefined)!.name, source_key: values.source.key,
      });
    }

    const columns = await query(`SELECT table_name, column_name, term_type FROM facts.fossil_columns`);
    for (const t of TABLES) {
      for (const p of t.properties) {
        expect(columns.find((r) => r.table_name === t.name && r.column_name === p.name)!.term_type ?? undefined).toBe(p.term_type);
      }
    }

    expect(await query(`SELECT * FROM facts.fossil_annotations ORDER BY column_name NULLS FIRST`)).toEqual([
      { table_name: named.name, column_name: null, predicate: label, o_type: 'L', o_value: 'persona', o_datatype: 'http://www.w3.org/1999/02/22-rdf-syntax-ns#langString', o_lang: 'es' },
      { table_name: named.name, column_name: named.properties[1]!.name, predicate: label, o_type: 'I', o_value: 'https://example.org/Identity', o_datatype: '', o_lang: '' },
    ]);
    const [stats] = await query(
      `SELECT null_count::INTEGER AS n, distinct_count::INTEGER AS d, min_value, max_value, most_common_vals AS vals, most_common_freqs AS freqs FROM facts.fossil_stats WHERE table_name = '${named.name}' AND column_name = '${named.properties[0]!.name}'`,
    );
    expect({ ...stats, vals: Array.from(stats!.vals as Iterable<unknown>, String), freqs: Array.from(stats!.freqs as Iterable<unknown>, Number) }).toEqual({
      n: 0, d: 3, min_value: '0', max_value: '2', vals: ['1'], freqs: [0.5],
    });
    const [uncounted] = await query(`SELECT null_count FROM facts.fossil_stats WHERE table_name = '${named.name}' AND column_name = '${named.properties[1]!.name}'`);
    expect(uncounted).toEqual({ null_count: null });
    await corpus.detach();
  });

  it('attaches the files a run answered in memory, as it attaches a corpus at a URL, and gives them back', async () => {
    const files = TABLES.map((t) => t.path).concat('fossil.json').map((path) => ({
      path,
      bytes: new Uint8Array(readFileSync(join(CORPUS, path))),
    }));
    const first = await attach('preview', { engine, files });
    const second = await attach('preview-again', { engine, files });
    for (const t of MANIFEST.vertex_tables) {
      const [count] = await query(`SELECT count(*)::INTEGER AS n FROM preview."${t.name}"`);
      expect(count!.n).toBe(t.record_count);
    }
    expect(files.every((f) => f.bytes.byteLength > 0), 'the caller keeps its bytes').toBe(true);
    await first.detach();
    const [still] = await query(`SELECT count(*)::INTEGER AS n FROM "preview-again"."Person"`);
    expect(still!.n).toBe(MANIFEST.vertex_tables.find((t) => t.name === 'Person')!.record_count);
    await second.detach();
    expect(await catalogs()).not.toContain('preview');
  });

  it('refuses files that do not hold what the manifest names', async () => {
    const files = [{ path: 'fossil.json', bytes: new Uint8Array(readFileSync(join(CORPUS, 'fossil.json'))) }];
    await expect(attach('partial', { engine, files })).rejects.toThrow(fossil('corpus/unreadable'));
    expect(await catalogs()).not.toContain('partial');
  });

  it('refuses a manifest that names one table twice, or a table named like its own relations', async () => {
    const twice = manifestOnly('twice', JSON.stringify({ ...MANIFEST, edge_tables: [{ ...MANIFEST.edge_tables[0], name: 'Person' }] }));
    await expect(attach('twice', { engine, url: twice })).rejects.toThrow(fossil('corpus/duplicate-table', { table: 'Person' }));
    const reserved = manifestOnly('reserved', JSON.stringify({ ...MANIFEST, edge_tables: [{ ...MANIFEST.edge_tables[0], name: 'fossil_tables' }] }));
    await expect(attach('reserved', { engine, url: reserved })).rejects.toThrow(fossil('corpus/duplicate-table', { table: 'fossil_tables' }));
    for (const relation of ['triples', 'fossil_annotations', 'fossil_stats']) {
      const dir = manifestOnly(relation, JSON.stringify({ ...MANIFEST, edge_tables: [{ ...MANIFEST.edge_tables[0], name: relation }] }));
      await expect(attach(relation, { engine, url: dir })).rejects.toThrow(fossil('corpus/duplicate-table', { table: relation }));
    }
    expect(await catalogs()).not.toContain('twice');
  });

  it('needs an engine, and exactly one of a host, a url and files', async () => {
    await expect(attach('x', { url: CORPUS } as never)).rejects.toThrow(fossil('api/invalid-argument', { argument: 'engine' }));
    await expect(attach('x', { engine })).rejects.toThrow(fossil('api/invalid-argument', { argument: 'host' }));
    await expect(attach('x', { engine, url: CORPUS, files: [] })).rejects.toThrow(fossil('api/invalid-argument', { argument: 'host' }));
  });

  it('refuses a location carrying a query or a fragment, before the engine sees anything', async () => {
    const asked: string[] = [];
    const untouched = { query: async (sql: string) => void asked.push(sql) } as unknown as Engine;
    for (const location of ['https://acct.blob.core.windows.net/c?sv=x&sig=y', `${CORPUS}#frag`]) {
      await expect(attach('x', { engine: untouched, url: location })).rejects.toThrow(fossil('corpus/not-a-location', { location }));
    }
    expect(asked).toEqual([]);
  });

  it('shares a catalog between two attachments, and the last to detach takes it', async () => {
    const first = await attach('shared', { engine, url: CORPUS });
    const again = await attach('shared', { engine, url: CORPUS });
    await first.detach();
    await first.detach();
    const [count] = await query(`SELECT count(*)::INTEGER AS n FROM shared."Person"`);
    expect(count!.n).toBe(MANIFEST.vertex_tables[0]!.record_count);
    expect(await catalogs()).toContain('shared');
    await again.detach();
    expect(await catalogs()).not.toContain('shared');
  });
});
