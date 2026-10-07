/**
 * `referenceTo` (`@fossil-lang/types`) against the expansion it inverts: the Rust in
 * `fossil-location`, reached through `inputs`. A reference written for a location
 * must come back out of the compiler as that location.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { referenceTo } from '@fossil-lang/types';
import { initFossilWasm, inputs } from '../src/index.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

const CONNECTIONS = {
  lake: 's3://lake/in/',
  deeper: 's3://lake/in/2024',
  'MinIO dev bucket': 'http://minio:9000/dev//',
  azure: 'abfss://c@acct.dfs.core.windows.net/raw',
};

/** What the compiler reads each reference as, in order. */
async function locations(references: readonly string[]): Promise<string[]> {
  const program = references.map((r, i) => `s${i} := io.csv(${JSON.stringify(r)})`).join('\n');
  const host = { connections: async () => CONNECTIONS, credentials: async () => [] };
  return (await inputs(`${program}\n`, { host })).map((s) => s.location);
}

describe('referenceTo, through the Rust expansion', () => {
  it('writes a reference the compiler expands back into the location', async () => {
    const wanted = [
      's3://lake/in/users.csv',
      's3://lake/in/2024/q1.csv',
      's3://lake/in/in/x.csv',
      'http://minio:9000/dev/people/2024.csv',
      'abfss://c@acct.dfs.core.windows.net/raw/a/b.parquet',
      's3://elsewhere/x.csv',
    ];
    const references = wanted.map((l) => referenceTo(l, CONNECTIONS));
    expect(references.slice(0, 5).every((r) => r.startsWith('@'))).toBe(true);
    expect(await locations(references)).toEqual(wanted);
  });
});
