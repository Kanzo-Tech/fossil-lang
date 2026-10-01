/**
 * Every failure path `/docs/design/failure` names for this package, forced: a host or a store that
 * never answers, a renewal that fails past expiry, a cleanup that fails during a failure, a release
 * that fails halfway. Each asserts the code that comes out, and that it comes out within its deadline.
 */
import './boot.js';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { HOST_MS, isFossilError, type DocumentWorkspace, type Engine, type Host } from '@fossil-lang/types';

import { mount, read, resolveDocuments } from '../src/index.js';
import { RENEW_BEFORE_MS } from '../src/mount.js';
import { countingHost, recordingEngine, s3, table } from './fixtures.js';

const JOB = 's3://keasy-dev/output/job-1/';
const HOUR = 3_600_000;
const never = <T>(): Promise<T> => new Promise<T>(() => {});
const silentHost: Host = { connections: never, credentials: never };

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('a host that never answers', () => {
  it('fails a mount as storage/host-silent after 30 s, and hands the host a signal that aborts then', async () => {
    let handed: AbortSignal | undefined;
    const host: Host = {
      connections: never,
      credentials: (_scope, _access, { signal }) => ((handed = signal), never()),
    };
    const outcome = mount(recordingEngine().engine, host, { job: 'job-1' }, 'read').catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(HOST_MS - 1);
    expect(handed?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const e = await outcome;
    expect(isFossilError(e, 'storage/host-silent')).toBe(true);
    expect(e).toMatchObject({ data: { scope: 'job job-1', after: HOST_MS } });
    expect(handed?.aborted).toBe(true);
  });

  it('fails resolveDocuments as storage/host-silent when its connections never come', async () => {
    const workspace: DocumentWorkspace = {
      setConnections: () => {},
      missingDocuments: () => [],
      registerDocument: () => {},
    };
    const outcome = resolveDocuments(workspace, silentHost).catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(HOST_MS);
    expect(await outcome).toMatchObject({ code: 'storage/host-silent', data: { scope: 'its connections' } });
  });

  it('answers storage/host-silent for each target a read could not get a credential for — the Rust deadline', async () => {
    const outcome = read(silentHost, [{ locator: `${JOB}a.csv`, connection: 'lake' }]);
    await vi.advanceTimersByTimeAsync(HOST_MS);
    const [result] = await outcome;
    expect(result).toMatchObject({ ok: false, problem: { code: 'storage/host-silent', data: { after: HOST_MS } } });
  });

  it('stops a mount at once when the caller does, with the caller’s reason', async () => {
    const stop = new AbortController();
    const outcome = mount(recordingEngine().engine, silentHost, { job: 'job-1' }, 'read', {
      signal: stop.signal,
    }).catch((e: unknown) => e);
    stop.abort(new DOMException('stopped', 'AbortError'));
    expect(await outcome).toMatchObject({ name: 'AbortError' });
  });
});

describe('a store that never answers', () => {
  it('is storage/unreachable after 30 s on wasm32, where object_store applies no timeout', async () => {
    vi.stubGlobal('fetch', vi.fn(never));
    const { host } = countingHost(() => [s3(JOB, 'K')]);
    const outcome = read(host, [{ locator: `${JOB}a.csv`, connection: 'lake' }]);
    await vi.advanceTimersByTimeAsync(30_000);
    const [result] = await outcome;
    expect(result).toMatchObject({ ok: false, problem: { code: 'storage/unreachable' } });
  });

  it('is storage/unreachable at once for a 503 — wasm32 does not retry, because the backoff would panic', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) =>
      Object.defineProperty(new Response('busy', { status: 503 }), 'url', { value: new Request(input, init).url }),
    );
    vi.stubGlobal('fetch', fetch);
    const { host } = countingHost(() => [s3(JOB, 'K')]);
    const [result] = await read(host, [{ locator: `${JOB}a.csv`, connection: 'lake' }]);
    expect(result).toMatchObject({ ok: false, problem: { code: 'storage/unreachable' } });
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});

describe('a renewal that fails until the credential expires', () => {
  it('keeps the failure where the next read finds it, instead of the console', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { engine } = recordingEngine();
    let calls = 0;
    const { host } = countingHost(() => {
      if (++calls > 1) throw new Error('keasy is down');
      return [s3(JOB, 'K', Date.now() + HOUR)];
    });
    const m = await mount(engine, host, { job: 'job-1' }, 'read');
    expect(m.failure).toBeUndefined();
    await vi.advanceTimersByTimeAsync(HOUR + RENEW_BEFORE_MS);
    expect(m.failure).toMatchObject({ code: 'storage/host-refused', data: { scope: 'job job-1' } });
    expect((m.failure as Error).cause).toEqual(new Error('keasy is down'));
    expect(() => m.name(`${JOB}a.parquet`)).toThrow(m.failure!);
    expect(error).not.toHaveBeenCalled();
  });
});

describe('a cleanup that fails', () => {
  it('during a failed mount is attached to the failure, never in its place', async () => {
    const refusal = new Error('secret refused');
    const dropFailed = new Error('DROP SECRET failed');
    const engine: Engine = {
      async query(sql) {
        if (sql.includes('duckdb_extensions()')) return table('loaded', [true]);
        if (sql.startsWith('CREATE')) throw refusal;
        throw dropFailed;
      },
      lend: async () => {},
      drop: async () => {},
    };
    const { host } = countingHost(() => [s3(JOB, 'A')]);
    const e = await mount(engine, host, { job: 'job-1' }, 'read').catch((x: unknown) => x);
    expect(isFossilError(e, 'engine/failed')).toBe(true);
    expect((e as Error).cause).toBe(refusal);
    expect(refusal.cause).toMatchObject({ code: 'engine/failed', cause: dropFailed });
  });

  it('on close goes on past a prefix that would not release, and says which failed', async () => {
    const dropped: string[] = [];
    const engine: Engine = {
      async query(sql) {
        if (sql.includes('duckdb_extensions()')) return table('loaded', [true]);
        if (sql.startsWith('DROP')) {
          dropped.push(sql);
          if (dropped.length === 1) throw new Error('first DROP failed');
        }
        return table('Success', []);
      },
      lend: async () => {},
      drop: async () => {},
    };
    const { host } = countingHost(() => [s3('s3://b/one/', 'A'), s3('s3://b/two/', 'B')]);
    const m = await mount(engine, host, { connection: 'lake' }, 'read');
    await expect(m.close()).rejects.toMatchObject({ code: 'engine/failed', cause: new Error('first DROP failed') });
    expect(dropped).toHaveLength(2);
  });
});
