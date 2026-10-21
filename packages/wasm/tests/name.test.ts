/**
 * `bindingName` across the wasm boundary, held against the lexer it is derived from: whatever a
 * file becomes tokenizes as exactly one `Ident`. The cases themselves are the Rust test's
 * (`fossil_syntax::name`); this suite holds that the export reaches JS and agrees with `tokenize`.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { bindingName, initFossilWasm, open, type FossilProgram } from '../src/index.js';

let tokenize: FossilProgram['tokenize'];

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
  ({ tokenize } = await open('t.fossil', { host: { connections: async () => ({}), credentials: async () => [] } }));
});

describe('@fossil-lang/wasm — bindingName', () => {
  it('is the file\'s own name, without its folder or extension, held to what an identifier holds', async () => {
    expect(await bindingName('people.csv')).toBe('people');
    expect(await bindingName('crm/people.v2.csv')).toBe('people_v2');
    expect(await bindingName('@lake/users.csv')).toBe('users');
    expect(await bindingName('2024-sales.parquet')).toBe('_2024_sales');
    expect(await bindingName('from.csv')).toBe('_from');
  });

  it('always lexes as one IDENT', async () => {
    for (const file of ['people.csv', 'Order Date.csv', '2024-sales.parquet', 'café.json', 'true.csv', '.csv', '', 'a b/ c.d.e']) {
      const name = await bindingName(file);
      const tokens = tokenize(name);
      expect(tokens.map((t) => t.kind), `${file} → ${name}`).toEqual(['Ident']);
    }
  });
});
