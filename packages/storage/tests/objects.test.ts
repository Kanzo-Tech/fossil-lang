import './boot.js';

import { afterEach, describe, expect, it, vi } from 'vitest';

import type { DocumentWorkspace, MissingDocument } from '@fossil-lang/types';

import { read, resolveDocuments, write } from '../src/index.js';
import { countingHost, s3 } from './fixtures.js';

const LAKE = 's3://b/lake/';

function fakeFetch(files: Record<string, string>) {
  const seen: { url: string; method: string; headers: Headers }[] = [];
  const impl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = new Request(input, init);
    seen.push({ url: request.url, method: request.method, headers: request.headers });
    const path = new URL(request.url).pathname;
    const body = files[path];
    const response =
      request.method === 'PUT'
        ? new Response(null, { status: 200, headers: { etag: '"e"' } })
        : body === undefined
          ? new Response('no', { status: 404 })
          : new Response(body, {
              headers: {
                'content-length': String(new TextEncoder().encode(body).length),
                'last-modified': 'Tue, 29 Sep 2026 08:00:00 GMT',
                etag: '"e"',
              },
            });
    // A fetched response carries its URL, and a client reads it back.
    return Object.defineProperty(response, 'url', { value: request.url });
  });
  vi.stubGlobal('fetch', impl);
  return seen;
}

afterEach(() => vi.unstubAllGlobals());

describe('read', () => {
  it('reads each file with the credential of its connection, asking once per connection', async () => {
    const seen = fakeFetch({ '/b/lake/a.csv': 'a', '/b/lake/b.csv': 'b' });
    const { host, asks } = countingHost(() => [s3(LAKE, 'K')]);
    const out = await read(host, [
      { locator: `${LAKE}a.csv`, connection: 'lake' },
      { locator: `${LAKE}b.csv`, connection: 'lake' },
    ]);
    expect(out.map((r) => (r.ok ? new TextDecoder().decode(r.bytes) : r.reason))).toEqual(['a', 'b']);
    expect(asks).toEqual([{ scope: { connection: 'lake' }, access: 'read' }]);
    expect(seen[0]!.url).toBe('http://localhost:9000/b/lake/a.csv');
    expect(seen[0]!.headers.get('authorization')).toMatch(/^AWS4-HMAC-SHA256 Credential=K\//);
    expect(seen[0]!.headers.get('x-amz-security-token')).toBe('token-K');
  });

  it('reads a public URL as it is, and refuses a bare path with no connection', async () => {
    fakeFetch({ '/shapes/p.shex': 'shape' });
    const { host, asks } = countingHost(() => []);
    const [pub, bare] = await read(host, [{ locator: 'https://x.test/shapes/p.shex' }, { locator: 'p.shex' }]);
    expect(pub).toEqual({ ok: true, bytes: new TextEncoder().encode('shape') });
    expect(bare).toEqual({ ok: false, reason: 'it names no connection and is not a public URL' });
    expect(asks).toEqual([]);
  });

  it('answers a failure per file, and asks nothing of the store outside what was vended', async () => {
    const seen = fakeFetch({});
    const { host } = countingHost(() => [s3(LAKE, 'K')]);
    const [missing, outside] = await read(host, [
      { locator: `${LAKE}gone.csv`, connection: 'lake' },
      { locator: 's3://b/other/x.csv', connection: 'lake' },
    ]);
    expect(missing).toMatchObject({ ok: false, reason: expect.stringMatching(/not found/i) });
    expect(outside).toMatchObject({ ok: false, reason: expect.stringMatching(/lies outside/) });
    expect(seen.map((r) => r.url)).toEqual(['http://localhost:9000/b/lake/gone.csv']);
  });
});

describe('write', () => {
  it('puts each file under the one prefix the job vends write on', async () => {
    const seen = fakeFetch({});
    const job = 's3://b/output/job-1/';
    const { host, asks } = countingHost(() => [s3(job, 'W')]);
    const prefix = await write(host, { job: 'job-1' }, [
      { path: 'graph.graph.yml', bytes: new Uint8Array([1]) },
    ]);
    expect(prefix).toBe(job);
    expect(asks).toEqual([{ scope: { job: 'job-1' }, access: 'write' }]);
    expect(seen[0]!.method).toBe('PUT');
    expect(seen[0]!.url).toBe('http://localhost:9000/b/output/job-1/graph.graph.yml');
    expect(seen[0]!.headers.get('authorization')).toMatch(/^AWS4-HMAC-SHA256 Credential=W\//);
  });

  it('refuses a scope with no single prefix to write under', async () => {
    const { host } = countingHost(() => [s3('s3://b/a/', 'A'), s3('s3://b/c/', 'C')]);
    await expect(write(host, { job: 'j' }, [])).rejects.toThrow(/exactly one prefix/);
  });
});

class Workspace implements DocumentWorkspace {
  readonly registered = new Map<string, string>();
  constructor(private readonly names: Record<string, MissingDocument[]>) {}
  setConnections(): void {}
  missingDocuments(): MissingDocument[] {
    const named = [...(this.names[''] ?? []), ...[...this.registered.keys()].flatMap((k) => this.names[k] ?? [])];
    return named.filter((d) => !this.registered.has(d.key));
  }
  registerDocument(key: string, text: string): void {
    this.registered.set(key, text);
  }
}

describe('resolveDocuments', () => {
  it('reads until nothing new is missing, and reports what stayed unread', async () => {
    fakeFetch({ '/b/lake/a.shex': 'A', '/b/lake/b.shex': 'B' });
    const { host } = countingHost(() => [s3(LAKE, 'K')]);
    const ws = new Workspace({
      '': [{ key: '@lake/a.shex', locator: `${LAKE}a.shex`, connection: 'lake' }],
      '@lake/a.shex': [
        { key: '@lake/b.shex', locator: `${LAKE}b.shex`, connection: 'lake' },
        { key: '@lake/c.shex', locator: `${LAKE}c.shex`, connection: 'lake' },
      ],
    });
    const out = await resolveDocuments(ws, host);
    expect(out.registered).toBe(2);
    expect(out.unread).toEqual([
      { key: '@lake/c.shex', locator: `${LAKE}c.shex`, connection: 'lake', reason: expect.stringMatching(/not found/) },
    ]);
    expect(ws.registered.get('@lake/b.shex')).toBe('B');
  });
});
