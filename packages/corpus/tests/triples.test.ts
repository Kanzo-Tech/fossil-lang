import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { open } from '../src/index.js';
import type { Manifest, Property } from '../src/manifest.gen.js';
import { ident } from '../src/sql.js';
import { triplesOf } from '../src/triples.js';
import { duckdb } from './engine.js';
import { held, nt, read } from './rdf.js';

/**
 * `"<name>".triples` — the corpus as RDF. Over the conformance corpus it is held to {@link held}, SQL
 * this suite writes over the same views; the natural lexical forms are held to the spellings XSD
 * gives them, over tables of every type word the writer emits.
 */

const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const MANIFEST = JSON.parse(readFileSync(join(CORPUS, 'fossil.json'), 'utf8')) as Manifest;
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const EX = 'https://example.org/';

let engine: Engine;
let query: (sql: string) => Promise<Record<string, unknown>[]>;
const scratch = mkdtempSync(join(tmpdir(), 'fossil-triples-test-'));

beforeAll(async () => {
  ({ engine, query } = await duckdb(join(scratch, 'spill')));
}, 60_000);
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

/** A vertex table over an in-memory relation of the same name: `subject`, `dense_id`, then `columns`. */
const vertex = (name: string, iri: string | undefined, columns: Property[]): Manifest['vertex_tables'][number] => ({
  name, iri, path: '', record_count: 0, identity: 'subject', key: 'dense_id',
  properties: [{ name: 'dense_id', type: 'uint32', role: 'address' }, { name: 'subject', type: 'string', role: 'identity' }, ...columns],
});

/** `triplesOf` over in-memory tables of the schema `tables`, as N-Triples. */
async function triples(manifest: Manifest): Promise<Set<string>> {
  await query(`CREATE OR REPLACE VIEW t AS ${triplesOf(manifest, (table) => `odd.${ident(table)}`)}`);
  return read('t', query);
}

describe('triples', () => {
  it('holds what the corpus holds: the classes, the literals as declared, the relations and the multi-valued properties', async () => {
    const close = await open('conformance', { engine, url: CORPUS });
    const want = await held(MANIFEST, 'conformance', query, { string: `${XSD}string` });
    const got = await read('conformance.triples', query);
    expect(want.size).toBeGreaterThan(0);
    expect([...got].filter((t) => !want.has(t))).toEqual([]);
    expect([...want].filter((t) => !got.has(t))).toEqual([]);
    await close();
  });

  it('spells each type word as its natural RDF literal, and a declared term as declared', async () => {
    await query(`CREATE SCHEMA IF NOT EXISTS odd`);
    await query(`CREATE OR REPLACE TABLE odd."a ""quoted"" table" AS SELECT * FROM (VALUES
      (0::UINTEGER, '${EX}v/0', 42::BIGINT, 1.5::DOUBLE, 'infinity'::DOUBLE, true, DATE '2026-10-06',
        TIMESTAMP '2026-10-06 12:30:00', TIME '12:30:00', 1.5::DECIMAL(4,1), '\\x01\\xAB'::BLOB, 'x', '${EX}o', NULL::VARCHAR),
      (1::UINTEGER, '${EX}v/1', -1::BIGINT, '-infinity'::DOUBLE, 'nan'::DOUBLE, false, DATE '1999-01-31',
        TIMESTAMP '1999-01-31 00:00:00', TIME '00:00:00', -2.0::DECIMAL(4,1), ''::BLOB, 'y', '${EX}p', 'z')
    ) AS t(dense_id, subject, i, d, f, b, day, ts, tm, dec, bin, s, ref, unmapped)`);
    const columns: Property[] = [
      ['i', 'int64'], ['d', 'double'], ['f', 'float'], ['b', 'bool'], ['day', 'date'], ['ts', 'timestamp'], ['tm', 'time'],
      ['dec', 'decimal'], ['bin', 'binary'], ['s', 'string'],
    ].map(([name, type]) => ({ name: name!, type: type!, iri: `${EX}${name}` }));
    columns.push({ name: 'ref', type: 'string', iri: `${EX}ref`, term_type: 'http://www.w3.org/ns/r2rml#IRI' });
    columns.push({ name: 'unmapped', type: 'string' });
    const got = await triples({ format: 'fossil/1', vertex_tables: [vertex('a "quoted" table', `${EX}V`, columns)], edge_tables: [] });

    const of = (s: number, p: string, o: string) => `${nt.iri(`${EX}v/${s}`)} ${nt.iri(`${EX}${p}`)} ${o}`;
    const lit = (value: string, type: string) => nt.literal(value, `${XSD}${type}`);
    expect([...got].sort()).toEqual(
      [
        `${nt.iri(`${EX}v/0`)} ${nt.iri(RDF_TYPE)} ${nt.iri(`${EX}V`)}`,
        `${nt.iri(`${EX}v/1`)} ${nt.iri(RDF_TYPE)} ${nt.iri(`${EX}V`)}`,
        of(0, 'i', lit('42', 'integer')), of(1, 'i', lit('-1', 'integer')),
        of(0, 'd', lit('1.5', 'double')), of(1, 'd', lit('-INF', 'double')),
        of(0, 'f', lit('INF', 'double')), of(1, 'f', lit('NaN', 'double')),
        of(0, 'b', lit('true', 'boolean')), of(1, 'b', lit('false', 'boolean')),
        of(0, 'day', lit('2026-10-06', 'date')), of(1, 'day', lit('1999-01-31', 'date')),
        of(0, 'ts', lit('2026-10-06T12:30:00', 'dateTime')), of(1, 'ts', lit('1999-01-31T00:00:00', 'dateTime')),
        of(0, 'tm', lit('12:30:00', 'time')), of(1, 'tm', lit('00:00:00', 'time')),
        of(0, 'dec', lit('1.5', 'decimal')), of(1, 'dec', lit('-2.0', 'decimal')),
        of(0, 'bin', lit('01AB', 'hexBinary')), of(1, 'bin', lit('', 'hexBinary')),
        of(0, 's', lit('x', 'string')), of(1, 's', lit('y', 'string')),
        of(0, 'ref', nt.iri(`${EX}o`)), of(1, 'ref', nt.iri(`${EX}p`)),
      ].sort(),
    );
  });

  it('maps nothing that has no IRI, and nothing at an end with no table', async () => {
    await query(`CREATE SCHEMA IF NOT EXISTS odd`);
    await query(`CREATE OR REPLACE TABLE odd.A AS SELECT 0::UINTEGER AS dense_id, '${EX}a' AS subject, 'v' AS label`);
    await query(`CREATE OR REPLACE TABLE odd.A_to_B AS SELECT 0::UINTEGER AS src, 0::UINTEGER AS dst`);
    const endpoints = { source: { key: 'src', references: 'A' }, destination: { key: 'dst', references: 'B' } };
    const edge = { name: 'A_to_B', label: 'to', iri: `${EX}to`, path: '', record_count: 1, properties: [], ...endpoints };
    const got = await triples({
      format: 'fossil/1',
      vertex_tables: [vertex('A', undefined, [{ name: 'label', type: 'string' }])],
      edge_tables: [edge],
    });
    expect([...got]).toEqual([]);
  });
});
