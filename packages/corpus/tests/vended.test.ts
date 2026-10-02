import { mkdtempSync, rmSync } from 'node:fs';
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
import type { Engine, Host } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import './boot.js';
import { open } from '../src/index.js';

// A job's corpus against a real DuckDB-WASM, vended as Azure: the one store the engine is lent
// file by file, so every file the corpus reads crosses `lend`. The SAS URL a lease gets is mapped
// back onto the conformance corpus through the node runtime's file protocol. The S3 half — a
// scoped secret over httpfs — needs a store, and is exercised against MinIO instead.

const require = createRequire(import.meta.url);
const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const HOST = 'acct.dfs.core.windows.net';

let db: DuckDBBindings;
let engine: Engine & { readonly lent: Map<string, string> };
const spill = mkdtempSync(join(tmpdir(), 'fossil-vended-spill-'));

const pathOf = (url: string): string =>
  join(CORPUS, new URL(url).pathname.replace(/^\/lake\/jobs\/[^/]+\//, ''));

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
  conn.query(`SET parquet_metadata_cache = true`);
  conn.query(`SET temp_directory = '${spill}'`);
  const lent = new Map<string, string>();
  engine = {
    lent,
    async query(sql) {
      return conn.query(sql);
    },
    async lend(files) {
      for (const [name, url] of Object.entries(files)) {
        if (lent.get(name) === url) continue;
        if (lent.has(name)) db.dropFile(name);
        db.registerFileURL(name, pathOf(url), DuckDBDataProtocol.NODE_FS, true);
        lent.set(name, url);
      }
    },
    async drop(names) {
      for (const name of names) if (lent.delete(name)) db.dropFile(name);
    },
  };
}, 60_000);

afterAll(() => rmSync(spill, { recursive: true, force: true }));

const host: Host = {
  connections: async () => ({}),
  async credentials(scope) {
    if (!('job' in scope)) throw new Error('a corpus asks for its job');
    return [
      {
        prefix: `abfss://lake@${HOST}/jobs/${scope.job}/`,
        config: { [`adls.sas-token.${HOST}`]: 'sv=2025&sr=d&sig=x' },
      },
    ];
  },
};

const catalogs = async (): Promise<string[]> =>
  Array.from((await engine.query(`SELECT database_name AS d FROM duckdb_databases()`)).getChild('d')!.toArray(), String);

const people = async (job: string): Promise<number> =>
  Number((await engine.query(`SELECT count(*) AS n FROM "${job}"."Person"`)).getChild('n')!.get(0));

const declared = async (job: string): Promise<number> =>
  Number((await engine.query(`SELECT rows FROM "${job}".fossil_tables WHERE table_name = 'Person'`)).getChild('rows')!.get(0));

describe('open(job, { engine, host })', () => {
  it('reads each job under its own prefix, so two corpora of one shape never meet', async () => {
    const one = await open('1', { engine, host });
    const two = await open('2', { engine, host });
    const lent = [...engine.lent.keys()];
    expect(lent).toContain('azure/acct/lake/jobs/1/fossil.json');
    expect(lent).toContain('azure/acct/lake/jobs/1/vertex/Person.parquet');
    expect(lent).toContain('azure/acct/lake/jobs/2/vertex/Person.parquet');
    expect([...engine.lent.values()].every((u) => u.startsWith('https://acct.blob.core.windows.net/lake/jobs/'))).toBe(true);

    const count = await declared('1');
    expect(await people('1')).toBe(count);
    expect(await people('2')).toBe(count);

    await one();
    expect([...engine.lent.keys()].some((n) => n.startsWith('azure/acct/lake/jobs/1/'))).toBe(false);
    expect(await catalogs()).not.toContain('1');
    expect(await people('2')).toBe(count);
    await two();
  }, 60_000);

  it('shares a job between two opens, and the first to close leaves the second whole', async () => {
    const first = await open('3', { engine, host });
    const again = await open('3', { engine, host });
    await first();
    expect(await people('3')).toBe(await declared('3'));
    await again();
    await again();
    expect([...engine.lent.keys()].some((n) => n.startsWith('azure/acct/lake/jobs/3/'))).toBe(false);
    expect(await catalogs()).not.toContain('3');
  }, 60_000);

  it('refuses a job the host vends nothing for', async () => {
    const empty: Host = { connections: async () => ({}), credentials: async () => [] };
    await expect(open('4', { engine, host: empty })).rejects.toThrow(
      expect.objectContaining({ name: 'FossilError', code: 'storage/no-credential', data: { scope: 'job 4', access: 'read' } }),
    );
  });
});
