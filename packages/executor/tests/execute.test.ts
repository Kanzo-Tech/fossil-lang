/**
 * Integration smoke for @fossil-lang/executor — exercises the real wasm-bindgen
 * build (`--target web`) through the package's typed surface. Proves the full
 * vertex+edge path runs in WASM and emits valid Parquet + the manifest.
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

// The header names are BARE and bound positionally by the `type { … }` line
// against `graph.shex`; every property key is the last segment of a predicate
// that document declares. This program used to carry its own CURIEs
// (`Person : ex:Person from users`, `iri = \`${ex:}person/${.id}\``) and passed
// only because `pkg/` is gitignored and the artefact under test was built in
// June — two and a half months of compiler ahead of it. Rebuilding it is what
// said so.
const PROGRAM = [
  'type { Person, Order } := io.shex("graph.shex")',
  '',
  'users := io.csv("https://data.example.com/users.csv")',
  'orders := io.csv("https://data.example.com/orders.csv")',
  '',
  'Person : Person from users',
  '    @subject = "https://example.org/person/{users.id}"',
  '    name = users.name',
  '',
  'Order : Order from orders',
  '    @subject = "https://example.org/order/{orders.order_id}"',
  '    placedBy = "https://example.org/person/{orders.user_id}"',
  '    total = orders.amount',
  '',
].join('\n');

async function fixture(name: string): Promise<Uint8Array> {
  const p = fileURLToPath(
    new URL(`../../../crates/fossil-df/tests/fixtures/${name}`, import.meta.url),
  );
  return new Uint8Array(await readFile(p));
}

/** The output contract the program names — the same file the Rust tests use. */
let SHEX: string;

beforeAll(async () => {
  SHEX = await readFile(
    fileURLToPath(new URL('../../../crates/fossil-df/tests/fixtures/graph.shex', import.meta.url)),
    'utf8',
  );
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
      const srcs = exec.sources(PROGRAM, {}, SHEX);
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
      result = await exec.run(PROGRAM, sources, 's3://jobs/run-1', {}, SHEX);
    } finally {
      exec.free();
    }

    // The names are the TILED ones, and that is the layout pass running rather
    // than a rename: a payload is `<prefix>/tiles.parquet` and the addressing
    // that opens it without a scan is `<prefix>/index/tiles.parquet`. These
    // read `vertex/Person.parquet` until the pass moved into the browser, and
    // nothing on the Rust side goes red when it changes — this file is the gate.
    const paths = result.files.map((f) => f.path).sort();
    for (const need of [
      'graph.graph.yml',
      'vertex/Person/tiles.parquet',
      'vertex/Person/index/tiles.parquet',
      'vertex/Order/tiles.parquet',
      'vertex/Order/index/tiles.parquet',
      'edge/Order_placedBy_Person/by_source/tiles.parquet',
      'edge/Order_placedBy_Person/by_target/tiles.parquet',
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

    expect(result.report.dest).toBe('s3://jobs/run-1');
    const person = result.report.vertices.find((v) => v.type === 'Person');
    expect(person?.vertex_count).toBe(3);
    expect(person?.prefix).toBe('vertex/Person/');
    const edge = result.report.edges.find((e) => e.edge_type === 'placedBy');
    expect(edge?.src_type).toBe('Order');
    expect(edge?.dst_type).toBe('Person');
    expect(edge?.edge_count).toBe(4);

    // The report is the manifest the run just encoded — not a second account of
    // it. Every document the index names is one of the files being uploaded.
    for (const rel of [...result.report.graph.vertices, ...result.report.graph.edges]) {
      expect(paths).toContain(rel);
    }
    // Every order names a real person, so nothing dangled — and `0` is stated.
    expect(result.report.dropped).toEqual([
      { prefix: 'edge/Order_placedBy_Person/', dropped: 0 },
    ]);
  });
});
