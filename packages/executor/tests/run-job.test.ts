/**
 * Orchestration smoke for {@link runJob} — drives the full browser job flow
 * (sources → fetch → run → upload → complete) against a mock transport + a fake
 * `fetch` that serves the CSV fixtures and records the signed PUTs. Proves the
 * end-to-end wiring keasy will use, with no network/server.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import {
  initFossilExecutor,
  FossilExecutor,
  runJob,
  type JobTransport,
  type CompletePayload,
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
  '',
].join('\n');

beforeAll(async () => {
  const wasmPath = fileURLToPath(new URL('../pkg/fossil_df_wasm_bg.wasm', import.meta.url));
  await initFossilExecutor({ wasmUrl: (await readFile(wasmPath)) as unknown as URL });
});

describe('runJob', () => {
  it('drives sources → fetch → run → upload → complete', async () => {
    const fixtures: Record<string, string> = {
      'users.csv': '../../../crates/fossil-df/tests/fixtures/users.csv',
      'orders.csv': '../../../crates/fossil-df/tests/fixtures/orders.csv',
    };

    const uploaded: string[] = [];
    let completed: CompletePayload | undefined;

    // Identity source signing; PUT urls keyed by path; complete records.
    const transport: JobTransport = {
      sourceRefs: async () => ({}),
      signSourceUrls: async (uris) => Object.fromEntries(uris.map((u) => [u, u])),
      signOutputUrls: async (paths) => Object.fromEntries(paths.map((p) => [p, `put:${p}`])),
      complete: async (req) => {
        completed = req;
      },
    };

    // Fake fetch: GET serves the fixture by basename; PUT records the key.
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (init?.method === 'PUT') {
        uploaded.push(url.replace(/^put:/, ''));
        return new Response(null, { status: 200 });
      }
      const name = Object.keys(fixtures).find((n) => url.includes(n));
      if (!name) return new Response(null, { status: 404 });
      const bytes = await readFile(fileURLToPath(new URL(fixtures[name], import.meta.url)));
      return new Response(bytes);
    }) as unknown as typeof fetch;

    const exec = new FossilExecutor();
    let runStatus;
    try {
      runStatus = await runJob(exec, PROGRAM, transport, { dest: 'job-1', fetchImpl });
    } finally {
      exec.free();
    }

    // Completed with the executor's RunStatus.
    expect(completed?.status).toBe('completed');
    expect(completed?.manifest).toBeDefined();
    const person = runStatus.vertices.find((v) => v.type === 'Person');
    expect(person?.count).toBe(3);
    const edge = runStatus.edges.find((e) => e.edge_type === 'placedBy');
    expect(edge?.count).toBe(4);

    // Every GraphAr file was signed + uploaded.
    for (const need of [
      'vertex/Person.parquet',
      'vertex/Order.parquet',
      'edge/Order_placedBy_Person/by_source.parquet',
      'graph.graph.yml',
    ]) {
      expect(uploaded).toContain(need);
    }
  });

  it('reports a failed completion when the executor throws', async () => {
    let completed: CompletePayload | undefined;
    const transport: JobTransport = {
      sourceRefs: async () => ({}),
      signSourceUrls: async (uris) => Object.fromEntries(uris.map((u) => [u, u])),
      signOutputUrls: async (paths) => Object.fromEntries(paths.map((p) => [p, `put:${p}`])),
      complete: async (req) => {
        completed = req;
      },
    };
    // A source fetch that always 404s → run fails before upload.
    const fetchImpl = (async () => new Response(null, { status: 404 })) as unknown as typeof fetch;

    const exec = new FossilExecutor();
    await expect(
      runJob(exec, PROGRAM, transport, { fetchImpl }).finally(() => exec.free()),
    ).rejects.toThrow();
    expect(completed?.status).toBe('failed');
    expect(completed?.error).toBeTruthy();
  });
});
