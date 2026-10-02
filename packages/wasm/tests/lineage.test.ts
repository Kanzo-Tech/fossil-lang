/**
 * The two questions a host asks with no program open — what a program references, and which
 * provider reads a file — answered with no `initFossilWasm()` first. This file never calls it: the
 * module is per test file, so the first `refs` here IS the boot.
 */
import { describe, it, expect } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { providerFor, providers, refs, type ProviderInfo } from '../src/index.js';

const wasm = () => readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url)));

describe('refs and providers boot the module themselves', () => {
  it('answers refs on the first call, with no initFossilWasm before it', async () => {
    const answer = await refs('users := io.csv("@lake/users.csv")\n', { wasm: await wasm() });
    expect(answer).toEqual([{ connection: 'lake', path: 'users.csv', role: 'data' }]);
  });

  it('keeps the boot, so a later call needs no module', async () => {
    const listed = await providers();
    expect(listed.map((p) => p.name)).toContain('csv');
    expect(listed.find((p) => p.name === 'shex')?.kind).toBe('schema');
  });

  it('stops waiting at its signal', async () => {
    const stop = new AbortController();
    stop.abort(new Error('stopped'));
    await expect(refs('', { signal: stop.signal })).rejects.toThrow('stopped');
  });
});

describe('providerFor', () => {
  const listed: ProviderInfo[] = [
    { name: 'csv', extensions: ['csv'], kind: 'data' },
    { name: 'rdf', extensions: ['ttl', 'nt'], kind: 'data' },
    { name: 'shacl', extensions: ['ttl', 'shacl'], kind: 'schema' },
    { name: 'shex', extensions: ['shex'], kind: 'schema' },
  ];

  it('reads one extension as two providers, by the role it plays', () => {
    expect(providerFor('a/b.ttl', 'data', listed)?.name).toBe('rdf');
    expect(providerFor('a/b.ttl', 'schema', listed)?.name).toBe('shacl');
    expect(providerFor('b.csv', 'schema', listed)).toBeUndefined();
  });

  it('matches case-insensitively, and ignores a URL query and fragment', () => {
    expect(providerFor('@lake/Users.CSV', 'data', listed)?.name).toBe('csv');
    expect(providerFor('https://x.example/a.csv?sig=abc.def#t', 'data', listed)?.name).toBe('csv');
  });

  it('finds nothing for a name with no extension', () => {
    expect(providerFor('csv', 'data', listed)).toBeUndefined();
    expect(providerFor('dir.csv/README', 'data', listed)).toBeUndefined();
    expect(providerFor('.csv', 'data', listed)).toBeUndefined();
    expect(providerFor('a.csv', 'data', [])).toBeUndefined();
  });

  it('lets a provider of both kinds answer either role', () => {
    const both: ProviderInfo[] = [{ name: 'x', extensions: ['x'], kind: 'both' }];
    expect(providerFor('a.x', 'data', both)?.name).toBe('x');
    expect(providerFor('a.x', 'schema', both)?.name).toBe('x');
  });

  it('agrees with the providers this build installs', async () => {
    const installed = await providers();
    expect(providerFor('people.parquet', 'data', installed)?.name).toBe('parquet');
    expect(providerFor('person.shex', 'schema', installed)?.name).toBe('shex');
  });
});
