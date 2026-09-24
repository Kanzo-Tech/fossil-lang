/**
 * Orchestration smoke for {@link runJob} — drives the full browser job flow
 * (documents → sources → run → upload → complete) against an identity-signing
 * `SourceHost` + a fake `fetch` that serves the fixtures and records the signed
 * PUTs. Proves the end-to-end wiring keasy will use, with no network/server.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { SourceHost } from '@fossil-lang/types';

import { initFossilExecutor, runJob, type CompletePayload, type JobOutput } from '../src/index.js';

// See `execute.test.ts` for why this program changed shape: bare header names
// bound positionally against `graph.shex`, which is the language since ruling 3
// of 2026-08-11.
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

/** Signs every locator as itself; the fake `fetch` below serves them. */
const host: SourceHost = {
  connections: async () => ({}),
  sign: async (locators) => Object.fromEntries(locators.map((l) => [l, l])),
};

/** Records the outcome; PUT URLs are keyed by path. */
function recording(): JobOutput & { completed?: CompletePayload } {
  const output: JobOutput & { completed?: CompletePayload } = {
    signOutputUrls: async (paths) => Object.fromEntries(paths.map((p) => [p, `put:${p}`])),
    complete: async (req) => {
      output.completed = req;
    },
  };
  return output;
}

beforeAll(async () => {
  const wasmPath = fileURLToPath(new URL('../pkg/fossil_df_wasm_bg.wasm', import.meta.url));
  await initFossilExecutor({ wasmUrl: (await readFile(wasmPath)) as unknown as URL });
});

describe('runJob', () => {
  it('drives documents → sources → run → upload → complete', async () => {
    // The shape document arrives the way the sources do: named by the
    // program, signed by the host, fetched.
    const fixtures: Record<string, string> = {
      'graph.shex': '../../../crates/fossil-df/tests/fixtures/graph.shex',
      'users.csv': '../../../crates/fossil-df/tests/fixtures/users.csv',
      'orders.csv': '../../../crates/fossil-df/tests/fixtures/orders.csv',
    };

    const uploaded: string[] = [];
    const output = recording();

    // Fake fetch: GET serves the fixture by basename; PUT records the key.
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (init?.method === 'PUT') {
        uploaded.push(url.replace(/^put:/, ''));
        return new Response(null, { status: 200 });
      }
      const name = Object.keys(fixtures).find((n) => url.endsWith(n));
      if (!name) return new Response(null, { status: 404 });
      const bytes = await readFile(fileURLToPath(new URL(fixtures[name], import.meta.url)));
      return new Response(bytes);
    }) as unknown as typeof fetch;

    const report = await runJob(PROGRAM, { host, output }, { dest: 'job-1', fetchImpl });

    // Completed with the manifest the run wrote.
    expect(output.completed?.status).toBe('completed');
    expect(output.completed?.manifest).toEqual(report);
    const person = report.vertices.find((v) => v.type === 'Person');
    expect(person?.vertex_count).toBe(3);
    // The edge exists only because the fetched shape document was the output
    // contract: without it `placedBy` would be a literal property.
    const edge = report.edges.find((e) => e.edge_type === 'placedBy');
    expect(edge?.edge_count).toBe(4);

    // Every GraphAr file was signed + uploaded — under the tiled names the
    // layout pass writes, payload and index alike.
    for (const need of [
      'vertex/Person/tiles.parquet',
      'vertex/Person/index/tiles.parquet',
      'vertex/Order/tiles.parquet',
      'edge/Order_placedBy_Person/by_source/tiles.parquet',
      'graph.graph.yml',
    ]) {
      expect(uploaded).toContain(need);
    }
  });

  it('fails the job when a document the program names cannot be read', async () => {
    const output = recording();
    const fetchImpl = (async () => new Response(null, { status: 404 })) as unknown as typeof fetch;

    await expect(runJob(PROGRAM, { host, output }, { fetchImpl })).rejects.toThrow(
      /graph\.shex \(HTTP 404\)/,
    );
    expect(output.completed?.status).toBe('failed');
    expect(output.completed?.error).toMatch(/could not be read/);
  });
});
