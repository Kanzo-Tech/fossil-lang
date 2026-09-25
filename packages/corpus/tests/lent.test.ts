import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  ConsoleLogger,
  DuckDBDataProtocol,
  NODE_RUNTIME,
  createDuckDB,
  type DuckDBBindings,
} from '@duckdb/duckdb-wasm/blocking';
import type { Engine, Signer } from '@fossil-lang/types';
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

import './boot.js';
import { open } from '../src/corpus.js';

// The lent rung against a real DuckDB-WASM. The engine below is the contract as
// `@kanzo-tech/mosaic` implements it, over the node runtime's file protocol: a signed URL is
// `signed://<n>/<path>`, so every signing is a different URL for the same bytes — which is
// exactly what DuckDB-WASM's registry refuses unless the lease is dropped first.

const require = createRequire(import.meta.url);
const CORPUS = fileURLToPath(new URL('../../../apps/corpus/conformance/corpus', import.meta.url));
const VERTEX_COUNT = 300;

let db: DuckDBBindings;
let engine: Engine & { readonly lent: Map<string, string>; swaps: number };
const spill = mkdtempSync(join(tmpdir(), 'fossil-lent-spill-'));

const pathOf = (url: string): string => url.replace(/^signed:\/\/\d+\//, '/');

beforeAll(async () => {
  const dist = dirname(require.resolve('@duckdb/duckdb-wasm'));
  db = await createDuckDB(
    {
      mvp: { mainModule: resolve(dist, './duckdb-mvp.wasm'), mainWorker: resolve(dist, './duckdb-node-mvp.worker.cjs') },
      eh: { mainModule: resolve(dist, './duckdb-eh.wasm'), mainWorker: resolve(dist, './duckdb-node-eh.worker.cjs') },
    },
    new ConsoleLogger(),
    NODE_RUNTIME,
  );
  await db.instantiate();
  const conn = db.connect();
  conn.query(`SET temp_directory = '${spill}'`);
  const lent = new Map<string, string>();
  engine = {
    lent,
    swaps: 0,
    async query(sql) {
      return conn.query(sql).toArray().map((row: { toJSON(): Record<string, unknown> }) => row.toJSON());
    },
    async lend(files) {
      for (const [name, url] of Object.entries(files)) {
        if (lent.get(name) === url) continue;
        if (lent.has(name)) {
          db.dropFile(name);
          engine.swaps++;
        }
        db.registerFileURL(name, pathOf(url), DuckDBDataProtocol.NODE_FS, true);
        lent.set(name, url);
      }
    },
    async drop(names) {
      for (const name of names) {
        if (!lent.delete(name)) continue;
        db.dropFile(name);
      }
    },
  };
  vi.stubGlobal('fetch', async (url: string) => new Response(readFileSync(pathOf(url), 'utf8')));
}, 60_000);

afterAll(() => {
  vi.unstubAllGlobals();
  rmSync(spill, { recursive: true, force: true });
});

afterEach(() => vi.restoreAllMocks());

let signings = 0;
const signer = (ttlMs?: number): Signer & { calls: number } => {
  const s = {
    calls: 0,
    ttlMs,
    async sign(paths: string[]) {
      s.calls++;
      const n = ++signings;
      return Object.fromEntries(paths.map((p) => [p, `signed://${n}${join(CORPUS, p)}`]));
    },
  };
  return s;
};

const catalogs = async (): Promise<string[]> =>
  (await engine.query(`SELECT database_name AS d FROM duckdb_databases()`)).map((r) => String(r['d']));

describe('open(name, { engine, host })', () => {
  it('namespaces files and views per corpus, so two corpora of one shape never meet', async () => {
    const one = await open('jobs/1', { engine, host: signer() });
    const two = await open('jobs/2', { engine, host: signer() });
    const lent = [...engine.lent.keys()];
    expect(lent.some((n) => n.startsWith('jobs/1/vertex/Person/'))).toBe(true);
    expect(lent.some((n) => n.startsWith('jobs/2/vertex/Person/'))).toBe(true);
    expect(lent.every((n) => n.startsWith('jobs/1/') || n.startsWith('jobs/2/'))).toBe(true);

    const [a, b] = [await one.relations(), await two.relations()];
    const person = (rs: typeof a) => rs.find((r) => r.name === 'Person')!;
    expect(person(a).sql).toBe('"jobs/1"."Person"');
    expect(person(b).sql).toBe('"jobs/2"."Person"');
    expect(person(a).rows).toBe(VERTEX_COUNT);
    expect(person(b).rows).toBe(VERTEX_COUNT);

    // Closing one takes its files and its catalog and nothing of the other's.
    await one.close();
    expect([...engine.lent.keys()].some((n) => n.startsWith('jobs/1/'))).toBe(false);
    expect(await catalogs()).not.toContain('jobs/1');
    expect((await two.schema()).vertices[0]!.count).toBe(VERTEX_COUNT);
    await two.close();
  }, 60_000);

  it('re-opens a corpus under fresh signatures by swapping the lease, not refusing it', async () => {
    const first = await open('jobs/3', { engine, host: signer() });
    await first.schema();
    const swaps = engine.swaps;
    const again = await open('jobs/3', { engine, host: signer() });
    expect(engine.swaps).toBeGreaterThan(swaps);
    expect((await again.schema()).vertices[0]!.count).toBe(VERTEX_COUNT);

    // The first to close leaves the second whole.
    await first.close();
    expect((await again.schema()).vertices[0]!.count).toBe(VERTEX_COUNT);
    await again.close();
  }, 60_000);

  it('signs again before the ttl runs out', async () => {
    const host = signer(1_000);
    const corpus = await open('jobs/4', { engine, host });
    const opened = host.calls;
    await corpus.schema();
    expect(host.calls).toBe(opened);

    const now = Date.now();
    vi.spyOn(Date, 'now').mockReturnValue(now + 600);
    const swaps = engine.swaps;
    await corpus.schema();
    expect(host.calls).toBe(opened + 1);
    expect(engine.swaps).toBeGreaterThan(swaps);
    await corpus.close();
  }, 60_000);

  it('close() gives back every file and view it took', async () => {
    const corpus = await open('jobs/5', { engine, host: signer() });
    await corpus.schema();
    expect(await catalogs()).toContain('jobs/5');
    await corpus.close();
    await corpus.close();
    expect([...engine.lent.keys()].some((n) => n.startsWith('jobs/5/'))).toBe(false);
    expect(await catalogs()).not.toContain('jobs/5');
  }, 60_000);
});
