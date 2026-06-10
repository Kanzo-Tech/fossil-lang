/**
 * Integration smoke for @fossil-lang/executor — exercises the real wasm-bindgen
 * build (`--target web`) through the package's typed surface. Proves the full
 * vertex+edge path runs in WASM and emits valid Parquet + a RunStatus.
 *
 * The Rust core is covered by `crates/fossil-df-wasm/tests/execute_core.rs`;
 * this suite covers the wasm-bindgen + JS-marshalling boundary cargo can't reach.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import {
  initFossilExecutor,
  FossilExecutor,
  type SourceInput,
} from '../src/index.js';

const PROGRAM = [
  'prefix ex: <https://example.org/>',
  '',
  'users := io.csv("https://data.example.com/users.csv")',
  'orders := io.csv("https://data.example.com/orders.csv")',
  '',
  'Person : ex:Person from users',
  '    iri = `${ex:}person/${.id}`',
  '    ex:name = .name',
  '',
  'Order : ex:Order from orders',
  '    iri = `${ex:}order/${.order_id}`',
  '    ex:placedBy = `${ex:}person/${.user_id}`',
  '    ex:total = .amount',
  '',
].join('\n');

async function fixture(name: string): Promise<Uint8Array> {
  const p = fileURLToPath(
    new URL(`../../../crates/fossil-df/tests/fixtures/${name}`, import.meta.url),
  );
  return new Uint8Array(await readFile(p));
}

beforeAll(async () => {
  const wasmPath = fileURLToPath(
    new URL('../pkg/fossil_df_wasm_bg.wasm', import.meta.url),
  );
  const bytes = await readFile(wasmPath);
  await initFossilExecutor({ wasmUrl: bytes as unknown as URL });
});

describe('FossilExecutor', () => {
  it('enumerates the program sources', () => {
    const exec = new FossilExecutor();
    try {
      const srcs = exec.sources(PROGRAM);
      expect(srcs.map((s) => s.uri).sort()).toEqual([
        'https://data.example.com/orders.csv',
        'https://data.example.com/users.csv',
      ]);
      expect(srcs.every((s) => s.format === 'csv')).toBe(true);
    } finally {
      exec.free();
    }
  });

  it('runs the full vertex+edge path in wasm and emits valid Parquet', async () => {
    const sources: SourceInput[] = [
      { uri: 'https://data.example.com/users.csv', format: 'csv', bytes: await fixture('users.csv') },
      { uri: 'https://data.example.com/orders.csv', format: 'csv', bytes: await fixture('orders.csv') },
    ];

    const exec = new FossilExecutor();
    let result;
    try {
      result = await exec.run(PROGRAM, sources, 's3://jobs/run-1');
    } finally {
      exec.free();
    }

    const paths = result.files.map((f) => f.path).sort();
    for (const need of [
      'graph.graph.yml',
      'vertex/Person.parquet',
      'vertex/Order.parquet',
      'edge/Order_placedBy_Person/by_source.parquet',
      'edge/Order_placedBy_Person/by_target.parquet',
    ]) {
      expect(paths).toContain(need);
    }

    // Every .parquet is a real Parquet file (magic "PAR1" at both ends).
    for (const f of result.files) {
      if (f.path.endsWith('.parquet')) {
        const b = Buffer.from(f.bytes);
        expect(b.subarray(0, 4).toString('ascii')).toBe('PAR1');
        expect(b.subarray(b.length - 4).toString('ascii')).toBe('PAR1');
      }
    }

    expect(result.runStatus.dest).toBe('s3://jobs/run-1');
    const person = result.runStatus.vertices.find((v) => v.type === 'Person');
    expect(person?.count).toBe(3);
    const edge = result.runStatus.edges.find((e) => e.edge_type === 'placedBy');
    expect(edge?.src_type).toBe('Order');
    expect(edge?.dst_type).toBe('Person');
    expect(edge?.count).toBe(4);
  });
});
