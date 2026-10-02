/**
 * `referenceTo` (`@fossil-lang/types`) against the expansion it inverts: the Rust in
 * `fossil-locator`, reached through `FossilWorkspace.sources`. A reference written for a locator
 * must come back out of the compiler as that locator.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { referenceTo } from '@fossil-lang/types';
import { initFossilWasm, FossilWorkspace } from '../src/index.js';

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
function locators(references: readonly string[]): string[] {
  const ws = new FossilWorkspace();
  try {
    ws.setConnections(CONNECTIONS);
    const program = references.map((r, i) => `s${i} := io.csv(${JSON.stringify(r)})`).join('\n');
    const handle = ws.openFile('prog.fossil', `${program}\n`);
    return ws.sources(handle).map((s) => s.locator);
  } finally {
    ws.free();
  }
}

describe('referenceTo, through the Rust expansion', () => {
  it('writes a reference the compiler expands back into the locator', () => {
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
    expect(locators(references)).toEqual(wanted);
  });
});
