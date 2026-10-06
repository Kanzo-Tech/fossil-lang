/**
 * The writer and the reader, end to end, with nothing of either restated here.
 *
 * `docs/programs/shop/shop.fossil` runs through `@fossil-lang/executor` — the only host that writes a
 * corpus — in this process, over generated rows large enough that `Order` spans two row groups. The
 * files it hands back are served by a plain HTTP origin (ranges, no listing) and attached with
 * `@fossil-lang/corpus` on DuckDB-WASM, read with plain SQL, and the guards run over the same
 * directory:
 *
 * 1. every table `fossil_tables` lists reads back at its `rows`;
 * 2. each vertex table is the `dense_id` range `fossil_tables` gives it, and every edge's ends fall
 *    in the ranges of the tables it names;
 * 3. `guards/check.mjs` passes on what the writer wrote;
 * 4. every column the writer emits says what it IS — its `role` in `fossil_columns` — and a
 *    program's column says nothing;
 * 5. `triples` of what the writer wrote is the RDF the corpus holds — the vertices' classes and
 *    literals, and `buyer` joined subject to subject across two types.
 *
 * The writer compresses every page with ZSTD, so (1) is also the proof that DuckDB-WASM reads it.
 */

import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'node:worker_threads';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { open, type Close } from '../src/index.js';
import type { Manifest } from '../src/manifest.js';
import { duckdb } from '../tests/engine.js';
import { held, read } from '../tests/rdf.js';
import { runShop } from './shop.js';
import { installSyncXhr } from './sync-xhr.js';

const GUARDS = fileURLToPath(new URL('../guards/check.mjs', import.meta.url));
/** People; three orders each, so `Order` is past one row group of 122,880. */
const PEOPLE = 50_000;

const scratch = mkdtempSync(join(tmpdir(), 'fossil-round-trip-'));
const dir = join(scratch, 'corpus');
let origin: { port: number; close: () => Promise<number> };
let engine: Engine;
let uninstall: (() => Promise<number>) | undefined;
let close: Close | undefined;
let rows: (sql: string) => Promise<Record<string, unknown>[]>;

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
  const { files } = await runShop(PEOPLE);
  for (const f of files) {
    mkdirSync(dirname(join(dir, f.path)), { recursive: true });
    writeFileSync(join(dir, f.path), f.bytes);
  }

  origin = await serve(dir);
  // DuckDB-WASM reads `http://` through httpfs, and httpfs through a synchronous XMLHttpRequest —
  // the page's in a browser, and one lent here, since Node has none.
  uninstall = installSyncXhr();
  ({ engine, query: rows } = await duckdb());
  await engine.query('LOAD httpfs');
  close = await open('shop', { engine, url: `http://127.0.0.1:${origin.port}/` });
}, 300_000);

afterAll(async () => {
  await close?.();
  await origin?.close();
  await uninstall?.();
  rmSync(scratch, { recursive: true, force: true });
});

describe('executor → HTTP → corpus', () => {
  it('reads every table back at its rows', async () => {
    const tables = await rows('SELECT table_name AS t, rows::BIGINT AS n FROM shop.fossil_tables');
    expect(tables.map((r) => r.t).sort()).toEqual(['Order', 'Order_buyer_Person', 'Person']);
    for (const { t, n } of tables) {
      expect(n, String(t)).toBeGreaterThan(0n);
      const [read] = await rows(`SELECT count(*)::BIGINT AS n FROM shop."${String(t)}"`);
      expect(read!.n, String(t)).toBe(n);
    }
    expect(tables.find((r) => r.t === 'Order')!.n).toBeGreaterThan(122_880n);
  }, 120_000);

  it('holds each vertex table to its range, and each edge to the tables it names', async () => {
    const vertices = await rows(`SELECT table_name AS t, first_id::BIGINT AS first, rows::BIGINT AS n FROM shop.fossil_tables WHERE kind = 'vertex'`);
    expect(vertices.map((r) => r.first)).toEqual([0n, vertices[0]!.n]);
    for (const { t, first, n } of vertices) {
      const [range] = await rows(`SELECT min(dense_id)::BIGINT AS lo, max(dense_id)::BIGINT AS hi FROM shop."${String(t)}"`);
      expect([range!.lo, range!.hi], String(t)).toEqual([first, (first as bigint) + (n as bigint) - 1n]);
    }
    const [outside] = await rows(
      `SELECT count(*)::BIGINT AS n FROM shop."Order_buyer_Person" e, shop.fossil_tables s, shop.fossil_tables d
        WHERE s.table_name = 'Order' AND d.table_name = 'Person'
          AND (e.src NOT BETWEEN s.first_id AND s.first_id + s.rows - 1 OR e.dst NOT BETWEEN d.first_id AND d.first_id + d.rows - 1)`,
    );
    expect(outside!.n).toBe(0n);
  }, 120_000);

  it('says what each writer column IS, and nothing of a program’s', async () => {
    const columns = await rows('SELECT table_name AS t, column_name AS c, role FROM shop.fossil_columns ORDER BY t, ordinal');
    const roles = (t: string) => Object.fromEntries(columns.filter((r) => r.t === t && r.role !== null).map((r) => [r.c, r.role]));
    expect(roles('Person')).toEqual({ dense_id: 'address', subject: 'identity' });
    expect(roles('Order')).toEqual({ dense_id: 'address', subject: 'identity' });
    expect(roles('Order_buyer_Person')).toEqual({ src: 'endpoint', dst: 'endpoint' });
    expect(columns.filter((r) => r.t === 'Person' && r.role === null).length).toBeGreaterThan(0);
  });

  it('means, as RDF, what it holds: its triples are the corpus’s', async () => {
    const text = readFileSync(join(dir, 'fossil.json'), 'utf8');
    const XSD = 'http://www.w3.org/2001/XMLSchema#';
    const want = await held(JSON.parse(text) as Manifest, 'shop', rows, {
      string: `${XSD}string`,
      float: `${XSD}double`,
      double: `${XSD}double`,
    });
    const got = await read('shop.triples', rows);
    expect(want.size).toBeGreaterThan(PEOPLE);
    expect([...got].filter((t) => !want.has(t))).toEqual([]);
    expect([...want].filter((t) => !got.has(t))).toEqual([]);
    const [buyers] = await rows('SELECT count(*)::BIGINT AS n FROM shop."Order_buyer_Person"');
    expect(BigInt([...got].filter((t) => t.includes('<https://shop.example/voc#buyer>')).length)).toBe(buyers!.n);
  }, 300_000);

  it('passes the guards', () => {
    const run = spawnSync(process.execPath, [GUARDS, dir], { encoding: 'utf8' });
    expect(run.status, `${run.stdout}\n${run.stderr}`).toBe(0);
  }, 120_000);
});
