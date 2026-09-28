import './boot.js';

import { afterEach, describe, expect, it, vi } from 'vitest';

import type { DocumentWorkspace, MissingDocument } from '@fossil-lang/types';

import { read, resolveDocuments, write } from '../src/index.js';
import { countingHost, s3 } from './fixtures.js';

const LAKE = 's3://b/lake/';

function fakeFetch(files: Record<string, string>) {
  const seen: { url: string; init?: RequestInit }[] = [];
  const impl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    seen.push({ url, init });
    const path = new URL(url).pathname;
    if (init?.method === 'PUT') return new Response(null, { status: 200 });
    const body = files[path];
    return body === undefined ? new Response('no', { status: 404 }) : new Response(body);
  });
  vi.stubGlobal('fetch', impl);
  return seen;
}

afterEach(() => vi.unstubAllGlobals());

describe('read', () => {
  it('signs a GET per file with the credential of its connection, asking once per connection', async () => {
    const seen = fakeFetch({ '/b/lake/a.csv': 'a', '/b/lake/b.csv': 'b' });
    const { host, asks } = countingHost(() => [s3(LAKE, 'K')]);
    const out = await read(host, [
      { locator: `${LAKE}a.csv`, connection: 'lake' },
      { locator: `${LAKE}b.csv`, connection: 'lake' },
    ]);
    expect(out.map((r) => r.ok && new TextDecoder().decode(r.bytes))).toEqual(['a', 'b']);
    expect(asks).toEqual([{ scope: { connection: 'lake' }, access: 'read' }]);
    expect(seen[0]!.url).toMatch(/^http:\/\/localhost:9000\/b\/lake\/a\.csv\?X-Amz-Algorithm=AWS4-HMAC-SHA256&/);
  });

  it('reads a public URL as it is, and refuses a bare path with no connection', async () => {
    fakeFetch({ '/shapes/p.shex': 'shape' });
    const { host, asks } = countingHost(() => []);
    const [pub, bare] = await read(host, [{ locator: 'https://x.test/shapes/p.shex' }, { locator: 'p.shex' }]);
    expect(pub).toEqual({ ok: true, bytes: new TextEncoder().encode('shape') });
    expect(bare).toEqual({ ok: false, reason: 'it names no connection and is not a public URL' });
    expect(asks).toEqual([]);
  });

  it('answers a failure per file, never with the signed URL in it', async () => {
    fakeFetch({});
    const { host } = countingHost(() => [s3(LAKE, 'K')]);
    const [missing, outside] = await read(host, [
      { locator: `${LAKE}gone.csv`, connection: 'lake' },
      { locator: 's3://b/other/x.csv', connection: 'lake' },
    ]);
    expect(missing).toEqual({ ok: false, reason: 'HTTP 404' });
    expect(outside).toEqual({ ok: false, reason: 'the host vends nothing covering it for connection lake' });
  });
});

describe('write', () => {
  it('PUTs each file under the one prefix the job vends write on', async () => {
    const seen = fakeFetch({});
    const job = 's3://b/output/job-1/';
    const { host, asks } = countingHost(() => [s3(job, 'W')]);
    const prefix = await write(host, { job: 'job-1' }, [
      { path: 'graph.graph.yml', bytes: new Uint8Array([1]) },
    ]);
    expect(prefix).toBe(job);
    expect(asks).toEqual([{ scope: { job: 'job-1' }, access: 'write' }]);
    expect(seen[0]!.init?.method).toBe('PUT');
    expect(seen[0]!.url).toMatch(/^http:\/\/localhost:9000\/b\/output\/job-1\/graph\.graph\.yml\?/);
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
      { key: '@lake/c.shex', locator: `${LAKE}c.shex`, connection: 'lake', reason: 'HTTP 404' },
    ]);
    expect(ws.registered.get('@lake/b.shex')).toBe('B');
  });
});
