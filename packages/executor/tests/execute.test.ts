/**
 * Integration smoke for @fossil-lang/executor — exercises the real wasm-bindgen
 * build (`--target web`) through the package's typed surface. Proves the full
 * vertex+edge path runs in WASM and emits a GraphAr graph with its mapping.
 *
 * The Rust core is covered by `crates/fossil-df/tests/execute_core.rs` and the
 * graph it writes by `crates/fossil-df/tests/write.rs`;
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

/** A `vertex_count` / `edge_count` file: one little-endian 64-bit integer. */
function countOf(files: { path: string; bytes: Uint8Array }[], path: string): bigint {
  const file = files.find((f) => f.path === path);
  if (file === undefined) throw new Error(`no ${path}`);
  return new DataView(file.bytes.buffer, file.bytes.byteOffset, file.bytes.byteLength).getBigInt64(0, true);
}

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

  it('refuses to write a run over files into a job: there is no host to vend the write', async () => {
    // @ts-expect-error — a run over files writes into memory, and the types say so.
    const refused = run(PROGRAM, { files: await files(), path: PATH, into: { job: 'job-1' } });
    await expect(refused).rejects.toMatchObject({
      name: 'FossilError',
      code: 'api/invalid-argument',
      data: { argument: 'into' },
    });
  });

  it('runs the full vertex+edge path in wasm and emits valid Parquet', async () => {
    const result = await run(PROGRAM, { files: await files(), path: PATH });

    // The GraphAr graph and its mapping, and nothing else: an information file
    // per type and per relation, their chunks and counts, `mapping.ttl`, and
    // `fossil.graph.yml`. Nothing on the Rust side sees the wasm build's tree;
    // this file is the gate.
    const paths = result.files.map((f) => f.path).sort();
    expect(paths).toEqual([
      'Order.vertex.yml',
      'Order_placedBy_Person.edge.yml',
      'Person.vertex.yml',
      'edge/Order_placedBy_Person/ordered_by_source/adj_list/part0/chunk0',
      'edge/Order_placedBy_Person/ordered_by_source/edge_count0',
      'edge/Order_placedBy_Person/ordered_by_source/offset/chunk0',
      'edge/Order_placedBy_Person/ordered_by_source/vertex_count',
      'fossil.graph.yml',
      'mapping.ttl',
      'vertex/Order/subject_total/chunk0',
      'vertex/Order/vertex_count',
      'vertex/Person/subject_name/chunk0',
      'vertex/Person/vertex_count',
    ]);

    // Every chunk is a real Parquet file (magic "PAR1" at both ends).
    for (const f of result.files) {
      if (f.path.includes('/chunk')) {
        const b = Buffer.from(f.bytes);
        expect(b.subarray(0, 4).toString('ascii')).toBe('PAR1');
        expect(b.subarray(b.length - 4).toString('ascii')).toBe('PAR1');
      }
    }

    const graph = new TextDecoder().decode(result.files.find((f) => f.path === 'fossil.graph.yml')!.bytes);
    expect(graph).toMatch(/^name: fossil$/m);
    expect(graph).toMatch(/^version: gar\/v1$/m);
    expect(countOf(result.files, 'vertex/Person/vertex_count')).toBe(3n);
    expect(countOf(result.files, 'edge/Order_placedBy_Person/ordered_by_source/edge_count0')).toBe(4n);

    expect(result.report.location).toBe('memory://graph/');
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
    expect(result.files.map((f) => f.path).sort()).toEqual([
      'Person.vertex.yml',
      'fossil.graph.yml',
      'mapping.ttl',
      'vertex/Person/subject_name/chunk0',
      'vertex/Person/vertex_count',
    ]);
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
    expect(countOf(result.files, 'vertex/Person/vertex_count')).toBe(3n);
  });
});
