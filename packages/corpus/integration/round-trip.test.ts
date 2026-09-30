/**
 * The writer and the reader, end to end, with nothing of either restated here.
 *
 * `docs/programs/shop/shop.fossil` runs through `@fossil-lang/executor` — the only host that writes a
 * corpus — in this process, over generated rows large enough that `Order` spans two row groups. The
 * files it hands back are served by a plain HTTP origin (ranges, no listing) and opened with
 * `@fossil-lang/corpus` on DuckDB-WASM, and the guards run over the same directory:
 *
 * 1. every table the manifest declares scans back at its `record_count`;
 * 2. a box scan returns exactly the rows of a full scan that fall in the box, and some that do not
 *    are left out;
 * 3. `guards/check.mjs` passes on what the writer wrote.
 *
 * The writer compresses every page with ZSTD, so (1) is also the proof that DuckDB-WASM reads it.
 */

import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'node:worker_threads';

import { FossilExecutor, initFossilExecutor } from '@fossil-lang/executor';
import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { open, type Corpus, type VertexTable } from '../src/index.js';
import { duckdb } from '../tests/engine.js';
import { installSyncXhr } from './sync-xhr.js';

const PROGRAM = new URL('../../../docs/programs/shop/', import.meta.url);
const GUARDS = fileURLToPath(new URL('../guards/check.mjs', import.meta.url));
/** People; three orders each, so `Order` is past one row group of 122,880. */
const PEOPLE = 50_000;

const scratch = mkdtempSync(join(tmpdir(), 'fossil-round-trip-'));
const dir = join(scratch, 'corpus');
let origin: { port: number; close: () => Promise<number> };
let engine: Engine;
let uninstall: (() => Promise<number>) | undefined;
let corpus: Corpus;

/** The program's two CSVs, generated: a fixed LCG so a failure reproduces. */
function sources(): { users: string; orders: string } {
  let state = 7;
  const next = () => (state = (state * 1_103_515_245 + 12_345) >>> 0) / 2 ** 32;
  const users = ['id,email,name,age'];
  for (let i = 0; i < PEOPLE; i += 1) users.push(`${i},u${i}@shop.example,User ${i},${10 + Math.floor(next() * 70)}`);
  const orders = ['id,user_id,amount'];
  for (let i = 0; i < 3 * PEOPLE; i += 1) orders.push(`${i},${Math.floor(next() * PEOPLE)},${(next() * 500).toFixed(2)}`);
  return { users: `${users.join('\n')}\n`, orders: `${orders.join('\n')}\n` };
}

/**
 * A static origin over `root` — `GET`, `HEAD` and single `Range`s, no directory listing — on a
 * thread of its own. The node bindings of DuckDB-WASM are blocking: their HTTP request holds this
 * thread's event loop until it is answered, so an origin on this thread never is.
 */
function serve(root: string): Promise<{ port: number; close: () => Promise<number> }> {
  const origin = new Worker(
    `
    const { createReadStream, statSync } = require('node:fs');
    const { createServer } = require('node:http');
    const { join, normalize } = require('node:path');
    const { parentPort, workerData: root } = require('node:worker_threads');
    const server = createServer((req, res) => {
      const path = normalize(join(root, decodeURIComponent(new URL(req.url, 'http://x').pathname)));
      let size;
      try {
        if (!path.startsWith(root) || !statSync(path).isFile()) throw new Error();
        size = statSync(path).size;
      } catch {
        res.writeHead(404).end();
        return;
      }
      const range = /^bytes=(\\d+)-(\\d*)$/.exec(req.headers.range ?? '');
      const [start, end] = range ? [Number(range[1]), range[2] ? Number(range[2]) : size - 1] : [0, size - 1];
      res.writeHead(range ? 206 : 200, {
        'accept-ranges': 'bytes',
        'content-length': end - start + 1,
        ...(range ? { 'content-range': 'bytes ' + start + '-' + end + '/' + size } : {}),
      });
      if (req.method === 'HEAD') res.end();
      else createReadStream(path, { start, end }).pipe(res);
    });
    server.listen(0, '127.0.0.1', () => parentPort.postMessage(server.address().port));
    `,
    { eval: true, workerData: root },
  );
  return new Promise((resolve) =>
    origin.once('message', (port: number) => resolve({ port, close: () => origin.terminate() })),
  );
}

beforeAll(async () => {
  const wasm = createRequire(import.meta.url).resolve('@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm');
  await initFossilExecutor(readFileSync(wasm));
  const base = 'https://local.test/shop/';
  const exec = new FossilExecutor(readFileSync(new URL('shop.fossil', PROGRAM), 'utf8'), `${base}shop.fossil`);
  let files;
  try {
    for (const d of exec.missingDocuments()) {
      exec.registerDocument(d.key, readFileSync(new URL(d.locator.slice(base.length), PROGRAM), 'utf8'));
    }
    const { users, orders } = sources();
    const bytes: Record<string, Uint8Array> = {};
    for (const s of exec.sources()) bytes[s.uri] = new TextEncoder().encode(s.uri.endsWith('users.csv') ? users : orders);
    ({ files } = await exec.runInMemory(bytes, 'memory://shop'));
  } finally {
    exec.free();
  }
  for (const f of files) {
    mkdirSync(dirname(join(dir, f.path)), { recursive: true });
    writeFileSync(join(dir, f.path), f.bytes);
  }

  origin = await serve(dir);
  // DuckDB-WASM reads `http://` through httpfs, and httpfs through a synchronous XMLHttpRequest —
  // the page's in a browser, and one lent here, since Node has none.
  uninstall = installSyncXhr();
  ({ engine } = await duckdb());
  await engine.query('LOAD httpfs');
  corpus = await open(`http://127.0.0.1:${origin.port}/`, { engine });
}, 300_000);

afterAll(async () => {
  await corpus?.close();
  await origin?.close();
  await uninstall?.();
  rmSync(scratch, { recursive: true, force: true });
});

describe('executor → HTTP → corpus', () => {
  it('scans every table back at its record_count', async () => {
    const tables = [...corpus.manifest.vertex_tables, ...corpus.manifest.edge_tables];
    expect(tables.map((t) => t.name).sort()).toEqual(['Order', 'Order_buyer_Person', 'Person']);
    for (const table of tables) {
      expect(table.record_count, table.name).toBeGreaterThan(0);
      const scan = corpus.scan({ table: table.name, select: [table.properties[0]!.name] });
      const batches = await scan.read(scan.plan());
      expect(batches.reduce((n, b) => n + b.numRows, 0), table.name).toBe(table.record_count);
    }
    const order = corpus.manifest.vertex_tables.find((t) => t.name === 'Order')!;
    expect(order.record_count).toBeGreaterThan(122_880);
  }, 120_000);

  it('returns from a box scan exactly the rows in the box', async () => {
    const person = corpus.manifest.vertex_tables.find((t) => t.name === 'Person') as VertexTable;
    const { x, y } = person.position!;
    const read = async (filter?: { bbox: [number, number, number, number] }) => {
      const scan = corpus.scan({ table: person.name, select: ['dense_id', x, y], ...(filter ? { filter } : {}) });
      const rows: [number, number, number][] = [];
      for (const b of await scan.read(scan.plan())) {
        const [ids, xs, ys] = ['dense_id', x, y].map((c) => b.getChild(c)!);
        for (let i = 0; i < b.numRows; i += 1) rows.push([Number(ids!.get(i)), xs!.get(i) as number, ys!.get(i) as number]);
      }
      return rows;
    };
    const all = await read();
    const xs = all.map((r) => r[1]);
    const ys = all.map((r) => r[2]);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const box: [number, number, number, number] = [x0, y0, x0 + (x1 - x0) / 3, y0 + (y1 - y0) / 3];

    const boxed = await read({ bbox: box });
    const inside = (r: [number, number, number]) => r[1] >= box[0] && r[1] <= box[2] && r[2] >= box[1] && r[2] <= box[3];
    expect(boxed.length).toBeGreaterThan(0);
    expect(boxed.length).toBeLessThan(all.length);
    expect(boxed.every(inside)).toBe(true);
    expect(boxed.map((r) => r[0]).sort((a, b) => a - b)).toEqual(
      all.filter(inside).map((r) => r[0]).sort((a, b) => a - b),
    );
  }, 120_000);

  it('passes the guards', () => {
    const run = spawnSync(process.execPath, [GUARDS, dir], { encoding: 'utf8' });
    expect(run.status, `${run.stdout}\n${run.stderr}`).toBe(0);
  }, 120_000);
});
