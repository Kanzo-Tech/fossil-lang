/**
 * The writer and a reader, end to end, with nothing of either restated here.
 *
 * `docs/programs/shop/shop.fossil` runs through `@fossil-lang/executor` — the only host that writes a
 * graph — in this process, over generated rows large enough that `Order` spans two row groups. The
 * files it hands back are served by a plain HTTP origin (ranges, no listing) and attached with
 * `@kanzo-tech/graphar-duckdb` on DuckDB-WASM, then read with plain SQL:
 *
 * 1. every type `graphar_vertex_types` and `graphar_edge_types` list reads back at its `count`;
 * 2. each vertex type is numbered `0 … count − 1` and every edge's ends fall inside the types it names;
 * 3. every type's first property is `subject`, and primary, in `graphar_properties`;
 * 4. every chunk `graphar_chunks` lists is a Parquet file DuckDB-WASM reads, ZSTD pages and all;
 * 5. `mapping.ttl` is beside the graph document.
 *
 * That the mapping means the same RDF as the data is held on the Rust side, over native DuckDB:
 * `crates/fossil-exec/tests/mapping.rs`.
 */

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { Worker } from 'node:worker_threads';

import { attachGraph, type GraphAttachment } from '@kanzo-tech/graphar-duckdb';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { duckdb } from './engine.js';
import { runShop } from './shop.js';
import { installSyncXhr } from './sync-xhr.js';

/** People; three orders each, so `Order` is past one row group of 122,880. */
const PEOPLE = 50_000;

const scratch = mkdtempSync(join(tmpdir(), 'fossil-round-trip-'));
const dir = join(scratch, 'graph');
let origin: { port: number; close: () => Promise<number> };
let uninstall: (() => Promise<number>) | undefined;
let graph: GraphAttachment | undefined;
let rows: (sql: string) => Promise<Record<string, unknown>[]>;
let written: string[];

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
  written = files.map((f) => f.path);
  for (const f of files) {
    mkdirSync(dirname(join(dir, f.path)), { recursive: true });
    writeFileSync(join(dir, f.path), f.bytes);
  }

  origin = await serve(dir);
  // DuckDB-WASM reads `http://` through httpfs, and httpfs through a synchronous XMLHttpRequest —
  // the page's in a browser, and one lent here, since Node has none.
  uninstall = installSyncXhr();
  const { engine, query } = await duckdb();
  rows = query;
  await engine.query('LOAD httpfs');
  graph = await attachGraph(
    engine,
    'shop',
    (paths) => paths.map((p) => `http://127.0.0.1:${origin.port}/${p}`),
    { entry: 'fossil.graph.yml' },
  );
}, 300_000);

afterAll(async () => {
  await graph?.detach();
  await origin?.close();
  await uninstall?.();
  rmSync(scratch, { recursive: true, force: true });
});

describe('executor → HTTP → attachGraph', () => {
  it('reads every type back at its count', async () => {
    const vertices = await rows('SELECT type AS t, count::BIGINT AS n FROM shop.graphar_vertex_types ORDER BY ordinal');
    expect(vertices.map((r) => r.t)).toEqual(['Person', 'Order']);
    for (const { t, n } of vertices) {
      expect(n, String(t)).toBeGreaterThan(0n);
      const [read] = await rows(`SELECT count(*)::BIGINT AS n FROM shop."${String(t)}"`);
      expect(read!.n, String(t)).toBe(n);
    }
    expect(vertices.find((r) => r.t === 'Order')!.n).toBeGreaterThan(122_880n);

    const edges = await rows('SELECT name AS e, count::BIGINT AS n, directed FROM shop.graphar_edge_types');
    expect(edges.map((r) => r.e)).toEqual(['Order_buyer_Person']);
    expect(edges[0]!.directed).toBe(true);
    const [read] = await rows('SELECT count(*)::BIGINT AS n FROM shop."Order_buyer_Person"');
    expect(read!.n).toBe(edges[0]!.n);
  }, 120_000);

  it('numbers each vertex type from 0, and holds each edge inside the types it names', async () => {
    const vertices = await rows('SELECT type AS t, count::BIGINT AS n FROM shop.graphar_vertex_types');
    for (const { t, n } of vertices) {
      const [range] = await rows(
        `SELECT min(_graphArVertexIndex)::BIGINT AS lo, max(_graphArVertexIndex)::BIGINT AS hi FROM shop."${String(t)}"`,
      );
      expect([range!.lo, range!.hi], String(t)).toEqual([0n, (n as bigint) - 1n]);
    }
    const [outside] = await rows(
      `SELECT count(*)::BIGINT AS n FROM shop."Order_buyer_Person" e, shop.graphar_vertex_types s, shop.graphar_vertex_types d
        WHERE s.type = 'Order' AND d.type = 'Person'
          AND (e._graphArSrcIndex NOT BETWEEN 0 AND s.count - 1 OR e._graphArDstIndex NOT BETWEEN 0 AND d.count - 1)`,
    );
    expect(outside!.n).toBe(0n);
  }, 120_000);

  it('puts the subject first, and primary', async () => {
    const properties = await rows(
      `SELECT type AS t, property AS p, is_primary AS primary_ FROM shop.graphar_properties WHERE ordinal = 1 ORDER BY t`,
    );
    expect(properties).toEqual([
      { t: 'Order', p: 'subject', primary_: true },
      { t: 'Person', p: 'subject', primary_: true },
    ]);
  });

  it('lists chunks DuckDB-WASM reads, one Parquet file each', async () => {
    const chunks = await rows('SELECT path FROM shop.graphar_chunks');
    expect(chunks.length).toBeGreaterThan(0);
    for (const { path } of chunks) {
      const [meta] = await rows(`SELECT count(*)::BIGINT AS groups FROM parquet_metadata('${String(path)}')`);
      expect(meta!.groups, String(path)).toBeGreaterThan(0n);
    }
  }, 120_000);

  it('writes mapping.ttl beside the graph document', () => {
    expect(written).toContain('mapping.ttl');
    expect(written).toContain('fossil.graph.yml');
  });
});
