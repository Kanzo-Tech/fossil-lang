/**
 * Node-side smoke test for @fossil-lang/wasm.
 *
 * Boots the wasm-bindgen --target web module in a Node Vitest run, then
 * exercises `program.tokenize`. The cargo-test suite already covers the
 * Rust side (tokenize_native); this suite covers the JsValue → TS-shape
 * serialization boundary the Rust tests can't reach.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { initFossilWasm, openProgram, type FossilProgram } from '../src/index.js';
import type { Token } from '@fossil-lang/types';

let tokenize: FossilProgram['tokenize'];

beforeAll(async () => {
  const wasmPath = fileURLToPath(
    new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url),
  );
  const bytes = await readFile(wasmPath);
  await initFossilWasm(bytes);
  ({ tokenize } = await openProgram('t.fossil', { host: { connections: async () => ({}), credentials: async () => [] } }));
});

describe('@fossil-lang/wasm — tokenize', () => {
  it('names each token and counts in UTF-16 code units', () => {
    const text = '// ñ😀\nusers';
    const rows: Token[] = tokenize(text);
    const last = rows.at(-1)!;
    expect(last.kind).toBe('Ident');
    expect(text.slice(last.start, last.end)).toBe('users');
  });

  it('returns empty array for empty source', () => {
    expect(tokenize('')).toEqual([]);
  });

  it('token ranges are monotonic non-overlapping (start ≤ end; next.start ≥ prev.end)', () => {
    const rows = tokenize(
      'type { Person } := io.shex("p.shex")\nuser := io.csv("u.csv")',
    );
    for (const r of rows) {
      expect(r.start).toBeLessThanOrEqual(r.end);
    }
    for (let i = 1; i < rows.length; i++) {
      expect(rows[i - 1]!.end).toBeLessThanOrEqual(rows[i]!.start);
    }
  });
});
