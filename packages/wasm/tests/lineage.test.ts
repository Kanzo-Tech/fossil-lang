/**
 * The questions a host asks with no program open — what a program reads, and which format reads a
 * file.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { Host } from '@fossil-lang/types';
import { formatFor, formats, initFossilWasm, inputs, type Format } from '../src/index.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

const host: Host = { connections: async () => ({ lake: 's3://lake' }), credentials: async () => [] };

const PROGRAM = `type { Person } := io.shex("@lake/person.shex")
users := io.csv("@lake/users.csv", delimiter = "|")
orders := io.parquet("orders.parquet")
`;

describe('inputs', () => {
  it('answers what a program reads through the host’s connections, the option only where written', async () => {
    expect(await inputs(PROGRAM, { host })).toEqual([
      {
        role: 'data',
        binding: 'users',
        key: '@lake/users.csv',
        location: 's3://lake/users.csv',
        connection: 'lake',
        format: 'csv',
        option: '|',
      },
      { role: 'data', binding: 'orders', key: 'orders.parquet', location: 'orders.parquet', format: 'parquet' },
      { role: 'schema', key: '@lake/person.shex', location: 's3://lake/person.shex', connection: 'lake' },
    ]);
  });

  it('answers the keys as written without a host', async () => {
    expect((await inputs(PROGRAM)).map((i) => [i.role, i.key, i.connection])).toEqual([
      ['data', '@lake/users.csv', undefined],
      ['data', 'orders.parquet', undefined],
      ['schema', '@lake/person.shex', undefined],
    ]);
  });

  it('stops waiting at its signal', async () => {
    const stop = new AbortController();
    stop.abort(new Error('stopped'));
    await expect(inputs('', { signal: stop.signal })).rejects.toThrow('stopped');
  });

  it('lists the formats this build reads', async () => {
    const listed = await formats();
    expect(listed.map((f) => f.name)).toContain('csv');
    expect(listed.find((f) => f.name === 'shex')?.kind).toBe('schema');
  });
});

describe('formatFor', () => {
  const listed: Format[] = [
    { name: 'csv', extensions: ['csv'], kind: 'data' },
    { name: 'rdf', extensions: ['ttl', 'nt'], kind: 'data' },
    { name: 'shacl', extensions: ['ttl', 'shacl'], kind: 'schema' },
    { name: 'shex', extensions: ['shex'], kind: 'schema' },
  ];

  it('reads one extension as two formats, by the role it plays', () => {
    expect(formatFor('a/b.ttl', 'data', listed)?.name).toBe('rdf');
    expect(formatFor('a/b.ttl', 'schema', listed)?.name).toBe('shacl');
    expect(formatFor('b.csv', 'schema', listed)).toBeUndefined();
  });

  it('matches case-insensitively, and ignores a URL query and fragment', () => {
    expect(formatFor('@lake/Users.CSV', 'data', listed)?.name).toBe('csv');
    expect(formatFor('https://x.example/a.csv?sig=abc.def#t', 'data', listed)?.name).toBe('csv');
  });

  it('finds nothing for a name with no extension', () => {
    expect(formatFor('csv', 'data', listed)).toBeUndefined();
    expect(formatFor('dir.csv/README', 'data', listed)).toBeUndefined();
    expect(formatFor('.csv', 'data', listed)).toBeUndefined();
    expect(formatFor('a.csv', 'data', [])).toBeUndefined();
  });

  it('lets a format of both kinds answer either role', () => {
    const both: Format[] = [{ name: 'x', extensions: ['x'], kind: 'both' }];
    expect(formatFor('a.x', 'data', both)?.name).toBe('x');
    expect(formatFor('a.x', 'schema', both)?.name).toBe('x');
  });

  it('agrees with the formats this build reads', async () => {
    const installed = await formats();
    expect(formatFor('people.parquet', 'data', installed)?.name).toBe('parquet');
    expect(formatFor('person.shex', 'schema', installed)?.name).toBe('shex');
  });
});
