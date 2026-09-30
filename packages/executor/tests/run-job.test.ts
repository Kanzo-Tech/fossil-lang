/**
 * Orchestration smoke for {@link runJob} — drives the full browser job flow
 * (documents → sources → run → write → complete) against a host that vends
 * `read` on one connection and `write` on the job, and a stubbed `fetch` that
 * answers as S3 does — HEAD, ranged GET, PUT — and only to signed requests.
 * No network, no server.
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

/** An S3 that serves the fixtures by path to signed HEADs and GETs (ranged too), and records PUTs. */
function stubFetch(serve: Record<string, string>) {
  const gets: string[] = [];
  const puts: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      const signed = request.headers.get('authorization')?.startsWith('AWS4-HMAC-SHA256 ') === true;
      const answer = (response: Response) => Object.defineProperty(response, 'url', { value: request.url });
      if (!signed) return answer(new Response(null, { status: 403 }));
      if (request.method === 'PUT') {
        puts.push(request.url);
        return answer(new Response(null, { status: 200, headers: { etag: '"e"' } }));
      }
      const fixture = serve[new URL(request.url).pathname];
      if (fixture === undefined) return answer(new Response(null, { status: 404 }));
      const whole = new Uint8Array(await readFile(fileURLToPath(new URL(fixture, import.meta.url))));
      const meta = { 'last-modified': 'Tue, 29 Sep 2026 08:00:00 GMT', etag: '"f"' };
      if (request.method === 'HEAD') {
        return answer(new Response(null, { headers: { ...meta, 'content-length': String(whole.length) } }));
      }
      gets.push(request.url);
      const range = /^bytes=(\d+)-(\d*)$/.exec(request.headers.get('range') ?? '');
      if (range === null) {
        return answer(new Response(whole, { headers: { ...meta, 'content-length': String(whole.length) } }));
      }
      const start = Number(range[1]);
      const end = range[2] === '' ? whole.length - 1 : Math.min(Number(range[2]), whole.length - 1);
      const part = whole.subarray(start, end + 1);
      return answer(
        new Response(part, {
          status: 206,
          headers: {
            ...meta,
            'content-length': String(part.length),
            'content-range': `bytes ${start}-${end}/${whole.length}`,
          },
        }),
      );
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
  it('reads through the connection and writes under the job prefix, every request signed', async () => {
    const { gets, puts } = stubFetch(FIXTURES);
    const { host, asks } = recordingHost();
    const job = recording(host);

    const report = await runJob(PROGRAM, job);

    expect(job.completed?.status).toBe('completed');
    expect(job.completed?.manifest).toEqual(report);
    expect(report.dest).toBe(JOB);
    // The relation exists only because the read shape document was the output
    // contract: without it `placedBy` would be a literal property.
    expect(report.dropped).toEqual([{ table: 'Order_placedBy_Person', dropped: 0 }]);

    for (const path of Object.keys(FIXTURES)) {
      expect(gets).toContain(`http://localhost:9000${path}`);
    }
    expect(asks).toContainEqual({ scope: { connection: 'lake' }, access: 'read' });
    expect(asks).toContainEqual({ scope: { job: 'job-1' }, access: 'write' });

    // Every file of the corpus was PUT at `<prefix><path>`, and the manifest
    // last: it is the commit.
    const written = puts.map((u) => `s3:/${new URL(u).pathname}`);
    expect(written.every((w) => w.startsWith(JOB))).toBe(true);
    for (const need of [
      'vertex/Person.parquet',
      'vertex/Order.parquet',
      'edge/Order_placedBy_Person.parquet',
      'fossil.json',
    ]) {
      expect(written).toContain(`${JOB}${need}`);
    }
    expect(written.at(-1)).toBe(`${JOB}fossil.json`);
  });

  it('fails the job, writing nothing, when a document the program names cannot be read', async () => {
    const { puts } = stubFetch({});
    const { host } = recordingHost();
    const job = recording(host);

    await expect(runJob(PROGRAM, job)).rejects.toThrow(
      expect.objectContaining({ name: 'FossilError', code: 'document/unread', data: { documents: [expect.stringContaining('graph.shex')] } }),
    );
    expect(job.completed?.status).toBe('failed');
    expect(job.completed?.problem).toMatchObject({
      code: 'document/unread',
      cause: { code: 'storage/unreachable', data: { locator: expect.stringContaining('graph.shex') } },
    });
    expect(puts).toEqual([]);
  });

  it('fails the job when the host vends no write on it', async () => {
    const { puts } = stubFetch(FIXTURES);
    const { host } = recordingHost();
    const job = recording(host, 'job-2');

    await expect(runJob(PROGRAM, job)).rejects.toThrow(
      expect.objectContaining({ name: 'FossilError', code: 'storage/no-credential', data: { scope: 'job job-2', access: 'write' } }),
    );
    expect(job.completed?.status).toBe('failed');
    expect(puts).toEqual([]);
  });
});
