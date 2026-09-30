import './boot.js';

import { afterEach, describe, expect, it, vi } from 'vitest';

import type { DocumentWorkspace, MissingDocument } from '@fossil-lang/types';

import { read, resolveDocuments } from '../src/index.js';
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
      body === undefined
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
    expect(out.map((r) => (r.ok ? new TextDecoder().decode(r.bytes) : r.problem.code))).toEqual(['a', 'b']);
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
    expect(bare).toMatchObject({ ok: false, problem: { code: 'storage/no-route', data: { locator: 'p.shex' } } });
    expect(asks).toEqual([]);
  });

  it('answers a failure per file, and asks nothing of the store outside what was vended', async () => {
    const seen = fakeFetch({});
    const { host } = countingHost(() => [s3(LAKE, 'K')]);
    const [missing, outside] = await read(host, [
      { locator: `${LAKE}gone.csv`, connection: 'lake' },
      { locator: 's3://b/other/x.csv', connection: 'lake' },
    ]);
    expect(missing).toMatchObject({
      ok: false,
      problem: { code: 'storage/unreachable', data: { locator: `${LAKE}gone.csv` }, cause: { name: expect.any(String) } },
    });
    expect(outside).toMatchObject({
      ok: false,
      problem: { code: 'storage/outside-prefix', data: { locator: 's3://b/other/x.csv' } },
    });
    expect(seen.map((r) => r.url)).toEqual(['http://localhost:9000/b/lake/gone.csv']);
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
      {
        key: '@lake/c.shex',
        locator: `${LAKE}c.shex`,
        connection: 'lake',
        problem: expect.objectContaining({ code: 'storage/unreachable', data: { locator: `${LAKE}c.shex` } }),
      },
    ]);
    expect(ws.registered.get('@lake/b.shex')).toBe('B');
  });
});
