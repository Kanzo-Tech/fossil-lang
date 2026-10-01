/**
 * The failure paths of `open` and `close` that `/docs/design/failure` names, against a scripted engine
 * rather than DuckDB: what is forced here is the engine or the host failing, not a query.
 */
import './boot.js';

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { HOST_MS, isFossilError, type Engine, type Host, type Table } from '@fossil-lang/types';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { open } from '../src/index.js';

const MANIFEST = readFileSync(fileURLToPath(new URL('../conformance/corpus/fossil.json', import.meta.url)), 'utf8');
const AZURE = 'acct.dfs.core.windows.net';
const never = <T>(): Promise<T> => new Promise<T>(() => {});

function answer(rows: readonly unknown[]): Table {
  return {
    numRows: rows.length,
    schema: { fields: [{ name: 'content' }] },
    getChild: () => ({ length: rows.length, get: (i) => rows[i] ?? null, toArray: () => rows }),
  };
}

/** An engine that serves `manifest` as fossil.json and fails the statements `fails` matches. */
function scripted(manifest: string, fails: (sql: string) => boolean = () => false) {
  const sql: string[] = [];
  const dropped: string[][] = [];
  const engine: Engine = {
    async query(text, { signal }) {
      signal.throwIfAborted();
      sql.push(text);
      if (fails(text)) throw new Error(`refused: ${text.split(' ')[0]}`);
      return answer(text.includes('read_text') ? [manifest] : []);
    },
    lend: async () => {},
    async drop(names) {
      dropped.push([...names]);
    },
  };
  return { engine, sql, dropped };
}

/** A host that vends an Azure prefix — lent file by file, so closing it drops what it lent. */
const azure = (drop?: () => never): Host => ({
  connections: async () => ({}),
  credentials: async (scope) => {
    drop?.();
    return [
      {
        prefix: `abfss://lake@${AZURE}/jobs/${'job' in scope ? scope.job : ''}/`,
        config: { [`adls.sas-token.${AZURE}`]: 'sv=2025&sig=x' },
      },
    ];
  },
});

describe('open, when something fails', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('is storage/host-silent after 30 s for a job whose host never answers', async () => {
    const { engine } = scripted(MANIFEST);
    const host: Host = { connections: never, credentials: never };
    const outcome = open('job-1', { engine, host }).catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(HOST_MS);
    expect(await outcome).toMatchObject({ code: 'storage/host-silent', data: { scope: 'job job-1' } });
  });

  it('stops when the caller does, and gives back what it had lent', async () => {
    const stop = new AbortController();
    const { engine, dropped } = scripted(MANIFEST);
    const slow: Engine = {
      ...engine,
      query: (text, options) =>
        text.startsWith('ATTACH') ? (stop.abort(new DOMException('stopped', 'AbortError')), engine.query(text, options)) : engine.query(text, options),
    };
    const e = await open('job-1', { engine: slow, host: azure(), signal: stop.signal }).catch((x: unknown) => x);
    expect(e).toMatchObject({ name: 'AbortError' });
    expect(dropped.flat()).toContain('azure/acct/lake/jobs/job-1/fossil.json');
  });

  it('keeps the reason it failed when giving the credential back fails too', async () => {
    const { engine } = scripted('not json');
    const failing: Engine = {
      ...engine,
      drop: async () => {
        throw new Error('drop failed');
      },
    };
    const e = await open('job-1', { engine: failing, host: azure() }).catch((x: unknown) => x);
    expect(isFossilError(e, 'corpus/not-json')).toBe(true);
    const chain: unknown[] = [];
    for (let at: unknown = e; at instanceof Error; at = at.cause) chain.push(at);
    expect(chain.at(-2)).toMatchObject({ code: 'engine/failed' });
    expect(chain.at(-1)).toEqual(new Error('drop failed'));
  });
});

describe('close, when the detach fails', () => {
  it('still gives the credential back, and says the detach failed', async () => {
    const { engine, dropped } = scripted(MANIFEST, (sql) => sql.startsWith('DETACH'));
    const corpus = await open('job-1', { engine, host: azure() });
    await expect(corpus.close()).rejects.toMatchObject({ code: 'engine/failed' });
    expect(dropped).toHaveLength(1);
  });
});

describe('a read after the credential expired', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('is the storage failure, not the engine’s 403', async () => {
    const hour = 3_600_000;
    let calls = 0;
    const host: Host = {
      connections: async () => ({}),
      credentials: async () => {
        if (++calls > 1) throw new Error('keasy is down');
        return [
          {
            prefix: `abfss://lake@${AZURE}/jobs/job-1/`,
            config: {
              [`adls.sas-token.${AZURE}`]: 'sv=2025&sig=x',
              [`adls.sas-token-expires-at-ms.${AZURE}`]: String(Date.now() + hour),
            },
          },
        ];
      },
    };
    const { engine } = scripted(MANIFEST);
    const corpus = await open('job-1', { engine, host });
    await vi.advanceTimersByTimeAsync(2 * hour);
    const scan = corpus.scan({ table: corpus.manifest.vertex_tables[0]!.name });
    await expect(scan.read(scan.plan())).rejects.toMatchObject({ code: 'storage/host-refused' });
  });
});
