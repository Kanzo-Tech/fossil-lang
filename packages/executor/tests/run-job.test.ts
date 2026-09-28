/**
 * Orchestration smoke for {@link runJob} — drives the full browser job flow
 * (documents → sources → run → upload → complete) against a host that vends
 * `read` on one connection and `write` on the job, and a stubbed `fetch` that
 * serves the fixtures to signed GETs and records the signed PUTs. No network,
 * no server.
 */
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { initStorage } from '@fossil-lang/storage';
import type { Access, Host, Scope, StorageCredential } from '@fossil-lang/types';

import { initFossilExecutor, runJob, type CompletePayload, type Job } from '../src/index.js';

// See `execute.test.ts` for why this program changed shape: bare header names
// bound positionally against `graph.shex`, which is the language since ruling 3
// of 2026-08-11.
const PROGRAM = [
  'type { Person, Order } := io.shex("@lake/graph.shex")',
  '',
  'users := io.csv("@lake/users.csv")',
  'orders := io.csv("@lake/orders.csv")',
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

const LAKE = 's3://lake/in/';
const JOB = 's3://lake/output/job-1/';

const s3 = (prefix: string): StorageCredential => ({
  prefix,
  config: {
    's3.access-key-id': 'K',
    's3.secret-access-key': 'secret',
    's3.endpoint': 'http://localhost:9000',
    's3.path-style-access': 'true',
    'client.region': 'us-east-1',
  },
});

/** Vends `read` on `@lake` and `write` on job-1, recording each ask. */
function recordingHost() {
  const asks: { scope: Scope; access: Access }[] = [];
  const host: Host = {
    connections: async () => ({ lake: LAKE.slice(0, -1) }),
    credentials: async (scope, access) => {
      asks.push({ scope, access });
      if ('connection' in scope && scope.connection === 'lake' && access === 'read') return [s3(LAKE)];
      if ('job' in scope && scope.job === 'job-1' && access === 'write') return [s3(JOB)];
      return [];
    },
  };
  return { host, asks };
}

function recording(host: Host, id = 'job-1'): Job & { completed?: CompletePayload } {
  const job: Job & { completed?: CompletePayload } = {
    id,
    host,
    complete: async (req) => {
      job.completed = req;
    },
  };
  return job;
}

const FIXTURES: Record<string, string> = {
  '/lake/in/graph.shex': '../../../crates/fossil-df/tests/fixtures/graph.shex',
  '/lake/in/users.csv': '../../../crates/fossil-df/tests/fixtures/users.csv',
  '/lake/in/orders.csv': '../../../crates/fossil-df/tests/fixtures/orders.csv',
};

/** GET serves a fixture by path, and only when it is signed; PUT records the URL. */
function stubFetch(serve: Record<string, string>) {
  const gets: string[] = [];
  const puts: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (init?.method === 'PUT') {
        puts.push(url);
        return new Response(null, { status: 200 });
      }
      gets.push(url);
      const fixture = serve[new URL(url).pathname];
      if (fixture === undefined || !url.includes('X-Amz-Signature=')) return new Response(null, { status: 404 });
      return new Response(await readFile(fileURLToPath(new URL(fixture, import.meta.url))));
    }),
  );
  return { gets, puts };
}

beforeAll(async () => {
  await initStorage(
    await readFile(createRequire(import.meta.url).resolve('@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm')),
  );
  const wasmPath = fileURLToPath(new URL('../pkg/fossil_df_wasm_bg.wasm', import.meta.url));
  await initFossilExecutor(await readFile(wasmPath));
});

afterEach(() => vi.unstubAllGlobals());

describe('runJob', () => {
  it('reads through the connection with signed GETs and writes under the job prefix with signed PUTs', async () => {
    const { gets, puts } = stubFetch(FIXTURES);
    const { host, asks } = recordingHost();
    const job = recording(host);

    const report = await runJob(PROGRAM, job);

    expect(job.completed?.status).toBe('completed');
    expect(job.completed?.manifest).toEqual(report);
    const person = report.vertices.find((v) => v.type === 'Person');
    expect(person?.vertex_count).toBe(3);
    // The edge exists only because the read shape document was the output
    // contract: without it `placedBy` would be a literal property.
    const edge = report.edges.find((e) => e.edge_type === 'placedBy');
    expect(edge?.edge_count).toBe(4);

    for (const path of Object.keys(FIXTURES)) {
      expect(gets.some((u) => u.startsWith(`http://localhost:9000${path}?X-Amz-`))).toBe(true);
    }
    expect(asks).toContainEqual({ scope: { connection: 'lake' }, access: 'read' });
    expect(asks).toContainEqual({ scope: { job: 'job-1' }, access: 'write' });

    // Every GraphAr file was PUT, signed, at `<prefix><path>` — payload and index alike.
    const written = puts.map((u) => {
      const url = new URL(u);
      expect(url.searchParams.has('X-Amz-Signature')).toBe(true);
      return `s3:/${url.pathname}`;
    });
    expect(written.every((w) => w.startsWith(JOB))).toBe(true);
    for (const need of [
      'vertex/Person/tiles.parquet',
      'vertex/Person/index/tiles.parquet',
      'vertex/Order/tiles.parquet',
      'edge/Order_placedBy_Person/by_source/tiles.parquet',
      'graph.graph.yml',
    ]) {
      expect(written).toContain(`${JOB}${need}`);
    }
  });

  it('fails the job, writing nothing, when a document the program names cannot be read', async () => {
    const { puts } = stubFetch({});
    const { host } = recordingHost();
    const job = recording(host);

    await expect(runJob(PROGRAM, job)).rejects.toThrow(/graph\.shex \(HTTP 404\)/);
    expect(job.completed?.status).toBe('failed');
    expect(job.completed?.error).toMatch(/could not be read/);
    expect(puts).toEqual([]);
  });

  it('fails the job when the host vends no write on it', async () => {
    const { puts } = stubFetch(FIXTURES);
    const { host } = recordingHost();
    const job = recording(host, 'job-2');

    await expect(runJob(PROGRAM, job)).rejects.toThrow(/0 write credentials for job job-2/);
    expect(job.completed?.status).toBe('failed');
    expect(puts).toEqual([]);
  });
});
