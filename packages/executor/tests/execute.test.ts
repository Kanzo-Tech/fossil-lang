/**
 * Integration smoke for @fossil-lang/executor — exercises the real wasm-bindgen
 * build (`--target web`) through the package's typed surface. Proves the full
 * vertex+edge path runs in WASM and emits valid Parquet + the manifest.
 *
 * The Rust core is covered by `crates/fossil-df/tests/execute_core.rs` and the
 * corpus it writes by `crates/fossil-df/tests/write.rs`;
 * this suite covers the wasm-bindgen + JS-marshalling boundary cargo can't reach.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { initFossilExecutor, run } from '../src/index.js';

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
  await initFossilExecutor(bytes);
});

/** Where `PROGRAM` lives, so `graph.shex` resolves beside it. */
const PATH = 'https://data.example.com/prog.fossil';

/** The program's document and both its sources, by location. */
async function files(): Promise<Record<string, Uint8Array>> {
  return {
    'https://data.example.com/graph.shex': new TextEncoder().encode(SHEX),
    'https://data.example.com/users.csv': await fixture('users.csv'),
    'https://data.example.com/orders.csv': await fixture('orders.csv'),
  };
}

describe('run over files', () => {
  it('refuses a document the files do not hold, by its key', async () => {
    const { 'https://data.example.com/graph.shex': _, ...rest } = await files();
    await expect(run(PROGRAM, { files: rest, path: PATH })).rejects.toThrow(
      expect.objectContaining({ name: 'FossilError', code: 'document/unread', data: { documents: ['https://data.example.com/graph.shex'] } }),
    );
  });

  it('refuses a source the files do not hold', async () => {
    const { 'https://data.example.com/orders.csv': _, ...rest } = await files();
    await expect(run(PROGRAM, { files: rest, path: PATH })).rejects.toThrow(
      expect.objectContaining({
        name: 'FossilError',
        code: 'source/not-found',
        data: { location: 'https://data.example.com/orders.csv' },
      }),
    );
  });

  it('runs the full vertex+edge path in wasm and emits valid Parquet', async () => {
    const result = await run(PROGRAM, { files: await files(), path: PATH });

    // One Parquet per vertex type and per relation, and the manifest — and
    // nothing else. Nothing on the Rust side sees the wasm build's tree; this
    // file is the gate.
    const paths = result.files.map((f) => f.path).sort();
    expect(paths).toEqual([
      'edge/Order_placedBy_Person.parquet',
      'fossil.json',
      'vertex/Order.parquet',
      'vertex/Person.parquet',
    ]);

    // Every .parquet is a real Parquet file (magic "PAR1" at both ends).
    for (const f of result.files) {
      if (f.path.endsWith('.parquet')) {
        const b = Buffer.from(f.bytes);
        expect(b.subarray(0, 4).toString('ascii')).toBe('PAR1');
        expect(b.subarray(b.length - 4).toString('ascii')).toBe('PAR1');
      }
    }

    const manifest = JSON.parse(
      new TextDecoder().decode(result.files.find((f) => f.path === 'fossil.json')!.bytes),
    );
    expect(manifest.format).toBe('fossil/1');
    const person = manifest.vertex_tables.find((v: { name: string }) => v.name === 'Person');
    expect(person.record_count).toBe(3);
    expect(person.path).toBe('vertex/Person.parquet');
    const edge = manifest.edge_tables.find((e: { label: string }) => e.label === 'placedBy');
    expect(edge.source.references).toBe('Order');
    expect(edge.destination.references).toBe('Person');
    expect(edge.record_count).toBe(4);

    // Every table the manifest names is one of the files written.
    for (const t of [...manifest.vertex_tables, ...manifest.edge_tables]) {
      expect(paths).toContain(t.path);
    }

    expect(result.report.location).toBe('memory://corpus/');
    // Every order names a real person, so nothing dangled — and `0` is stated.
    expect(result.report.dropped).toEqual([{ table: 'Order_placedBy_Person', dropped: 0 }]);
  });

  // The getting-started page's script, as a test: a program located at a URL names its data and
  // its shape relative to itself, and the host reads each by the location fossil resolved.
  it('resolves the relative sources of a program located at a URL', async () => {
    const dir = new URL('../../../docs/programs/hello/', import.meta.url);
    const base = 'https://local.test/hello/';
    const read = async (name: string) => new Uint8Array(await readFile(new URL(name, dir)));
    const result = await run(await readFile(new URL('hello.fossil', dir), 'utf8'), {
      files: { [`${base}hello.shex`]: await read('hello.shex'), [`${base}data/people.csv`]: await read('data/people.csv') },
      path: `${base}hello.fossil`,
    });
    expect(result.files.map((f) => f.path).sort()).toEqual(['fossil.json', 'vertex/Person.parquet']);
  });

  // Two mappings of one type are a `UNION ALL`, and a union of two scans has two
  // partitions. DataFusion 54 coalesces them by spawning a Tokio task per
  // partition, and there is no runtime here: the run panicked, trapped inside a
  // microtask and never settled. The Rust mirror is `execute_core.rs`
  // `a_union_of_two_mappings_runs_with_no_tokio_runtime`.
  it('runs a union of two mappings of one type to completion', async () => {
    const result = await run(
      [
        'type { Person, Order } := io.shex("graph.shex")',
        '',
        'users := io.csv("https://data.example.com/users.csv")',
        '',
        'Early : Person from users.where(users.id < 3)',
        '    @subject = "https://example.org/person/{users.id}"',
        '    name = users.name',
        '',
        'Late : Person from users.where(users.id >= 3)',
        '    @subject = "https://example.org/person/{users.id}"',
        '    name = users.name',
        '',
      ].join('\n'),
      {
        files: {
          'https://data.example.com/graph.shex': new TextEncoder().encode(SHEX),
          'https://data.example.com/users.csv': await fixture('users.csv'),
        },
        path: PATH,
      },
    );
    const manifest = JSON.parse(
      new TextDecoder().decode(result.files.find((f) => f.path === 'fossil.json')!.bytes),
    );
    expect(manifest.vertex_tables[0].record_count).toBe(3);
  });
});
