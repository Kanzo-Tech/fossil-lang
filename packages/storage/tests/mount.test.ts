import './boot.js';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { mount } from '../src/index.js';
import { RENEW_BEFORE_MS } from '@fossil-lang/types/internal';
import { RETRY_MS } from '../src/mount.js';
import { azure, countingHost, recordingEngine, s3 } from './fixtures.js';

const JOB = 's3://keasy-dev/output/job-1/';
const T0 = Date.UTC(2026, 8, 28, 12);
const HOUR = 3_600_000;

describe('mount', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
  });
  afterEach(() => vi.useRealTimers());

  it('installs one scoped secret per prefix and names s3:// as it is', async () => {
    const { engine, sql } = recordingEngine();
    const { host } = countingHost(() => [s3(JOB, 'A')]);
    const m = await mount(engine, host, { job: 'job-1' }, 'read');
    expect(sql).toHaveLength(1);
    expect(sql[0]).toMatch(/^CREATE OR REPLACE SECRET fossil_read_[0-9a-f]{16} \(TYPE s3, /);
    expect(sql[0]).toContain(`SCOPE '${JOB}'`);
    expect(m.prefixes).toEqual([JOB]);
    expect(m.name(`${JOB}graph.graph.yml`)).toBe(`${JOB}graph.graph.yml`);
    expect(await m.files([`${JOB}a.parquet`])).toEqual([`${JOB}a.parquet`]);
  });

  it('refuses a location no credential covers, before any request leaves', async () => {
    const { engine } = recordingEngine();
    const { host } = countingHost(() => [s3(JOB, 'A')]);
    const m = await mount(engine, host, { job: 'job-1' }, 'read');
    expect(() => m.name('s3://keasy-dev/output/job-1-evil/x')).toThrow(
      expect.objectContaining({
        name: 'FossilError',
        code: 'storage/outside-prefix',
        data: expect.objectContaining({ location: 's3://keasy-dev/output/job-1-evil/x' }),
      }),
    );
  });

  it('picks the longest prefix that covers a location', async () => {
    const { engine } = recordingEngine();
    const wide = 's3://b/data/';
    const narrow = 's3://b/data/deep/';
    const { host } = countingHost(() => [s3(wide, 'W'), s3(narrow, 'N')]);
    const m = await mount(engine, host, { connection: 'lake' }, 'read');
    expect(m.name(`${narrow}x.csv`)).toBe(`${narrow}x.csv`);
    expect(m.name(`${wide}x.csv`)).toBe(`${wide}x.csv`);
  });

  it('renews five minutes before expiry under the same name, and again after', async () => {
    const { engine, sql } = recordingEngine();
    let issued = 0;
    const { host, asks } = countingHost(() => [s3(JOB, `K${++issued}`, Date.now() + HOUR)]);
    await mount(engine, host, { job: 'job-1' }, 'read');
    const name = /SECRET (\S+) /.exec(sql[0]!)![1];

    await vi.advanceTimersByTimeAsync(HOUR - RENEW_BEFORE_MS - 1);
    expect(asks).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(asks).toHaveLength(2);
    expect(sql).toHaveLength(2);
    expect(sql[1]).toContain(`SECRET ${name} (`);
    expect(sql[1]).toContain("KEY_ID 'K2'");

    await vi.advanceTimersByTimeAsync(HOUR - RENEW_BEFORE_MS);
    expect(asks).toHaveLength(3);
    expect(sql[2]).toContain("KEY_ID 'K3'");
  });

  it('retries a failed renewal while the credential still works', async () => {
    const { engine, sql } = recordingEngine();
    let calls = 0;
    const { host } = countingHost(() => {
      calls++;
      if (calls === 2) throw new Error('keasy is down');
      return [s3(JOB, `K${calls}`, Date.now() + HOUR)];
    });
    await mount(engine, host, { job: 'job-1' }, 'read');
    await vi.advanceTimersByTimeAsync(HOUR - RENEW_BEFORE_MS);
    expect(calls).toBe(2);
    expect(sql).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    expect(calls).toBe(3);
    expect(sql[1]).toContain("KEY_ID 'K3'");
  });

  it('shares a prefix between two mounts and drops it with the last one', async () => {
    const { engine, sql } = recordingEngine();
    const { host, asks } = countingHost(() => [s3(JOB, 'A', Date.now() + HOUR)]);
    const first = await mount(engine, host, { job: 'job-1' }, 'read');
    const second = await mount(engine, host, { job: 'job-1' }, 'read');
    expect(sql.filter((s) => s.startsWith('CREATE'))).toHaveLength(1);

    await first.close();
    expect(sql.some((s) => s.startsWith('DROP'))).toBe(false);
    await first.close();
    expect(sql.some((s) => s.startsWith('DROP'))).toBe(false);
    await second.close();
    expect(sql.at(-1)).toMatch(/^DROP SECRET IF EXISTS fossil_read_[0-9a-f]{16}$/);

    await vi.advanceTimersByTimeAsync(2 * HOUR);
    expect(asks).toHaveLength(2);
  });

  it('keeps read and write on one prefix apart', async () => {
    const { engine, sql } = recordingEngine();
    const { host } = countingHost(() => [s3(JOB, 'A')]);
    await mount(engine, host, { job: 'job-1' }, 'read');
    await mount(engine, host, { job: 'job-1' }, 'write');
    expect(sql[0]).toContain('fossil_read_');
    expect(sql[1]).toContain('fossil_write_');
  });

  it('lends Azure files by name, re-lends them on renewal and drops them at close', async () => {
    const { engine, sql, leases, dropped } = recordingEngine();
    const prefix = 'abfss://lake@acct.dfs.core.windows.net/raw/';
    let issued = 0;
    const { host } = countingHost(() => [
      {
        ...azure(prefix, `sig=${++issued}`),
        config: {
          ...azure(prefix, `sig=${issued}`).config,
          'adls.sas-token-expires-at-ms.acct.dfs.core.windows.net': String(Date.now() + HOUR),
        },
      },
    ]);
    const m = await mount(engine, host, { connection: 'lake' }, 'read');
    expect(sql).toEqual([]);
    expect(await m.files([`${prefix}users.csv`])).toEqual(['azure/acct/lake/raw/users.csv']);
    expect(leases).toEqual([
      { 'azure/acct/lake/raw/users.csv': 'https://acct.blob.core.windows.net/lake/raw/users.csv?sig=1' },
    ]);
    await vi.advanceTimersByTimeAsync(HOUR - RENEW_BEFORE_MS);
    expect(leases.at(-1)).toEqual({
      'azure/acct/lake/raw/users.csv': 'https://acct.blob.core.windows.net/lake/raw/users.csv?sig=2',
    });
    await m.close();
    expect(dropped).toEqual([['azure/acct/lake/raw/users.csv']]);
  });

  it('says so when the engine has no httpfs', async () => {
    const { engine } = recordingEngine(false);
    const { host } = countingHost(() => [s3(JOB, 'A')]);
    await expect(mount(engine, host, { job: 'job-1' }, 'read')).rejects.toMatchObject({
      name: 'FossilError',
      code: 'storage/no-httpfs',
    });
  });

  it('says so when the host vends nothing', async () => {
    const { engine } = recordingEngine();
    const { host } = countingHost(() => []);
    await expect(mount(engine, host, { connection: 'lake' }, 'read')).rejects.toMatchObject({
      name: 'FossilError',
      code: 'storage/no-credential',
      data: { scope: 'connection lake', access: 'read' },
    });
  });
});
