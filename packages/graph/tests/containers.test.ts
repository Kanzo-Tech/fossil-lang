/**
 * One corpus, two containers, the same answers — asked of the published module.
 *
 * A tile is a fixed range of `dense_id`. Which container carries it — one Parquet per tile with the
 * address in the name, or one Parquet whose row groups are the tiles — is a second question, and
 * `graph.graph.yml`'s `container` is what answers it. `openCorpus` reads both, and this is the file
 * that says so: the same graph is written both ways and every answer has to come back identical.
 *
 * **This is the diff a format change is allowed to arrive behind**, and its plain-Node twin is
 * `apps/corpus/conformance/containers.mjs`, which asks the same question of a reader that shares no
 * line with this one. Two implementations moved and compared BEFORE a writer emits the new
 * container; after it emits one, the only corpus that exists is the new one and there is nothing
 * left to compare against.
 *
 * **What it cannot prove.** That the row-group container is cheaper, which is the entire reason for
 * it — the measured win is HTTP requests per window and a local file makes none. What it asserts
 * instead is the countable half: the same window names strictly fewer files. `cost.test.ts` is
 * where the shape of that count against N lives.
 *
 * **Why `chunk_size` is 4,096 here and 1,024 next door.** DuckDB emits row groups in multiples of
 * its 2,048-row vector, so a `ROW_GROUP_SIZE` under that is silently clamped and the tiles land
 * somewhere else — 300 rows written at `ROW_GROUP_SIZE 64` come back as one row group of 300. It is
 * a limit of the tool that writes the fixture, not of the format, and it is why the checked-in
 * conformance corpus (`chunk_size` 64) cannot be repacked into the other container.
 */

import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { openCorpus, type Box, type Corpus } from '../src/corpus.js';
import type { QueryFn, QueryRow } from '../src/query.js';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../../../apps/corpus/guards/fixture.mjs';

const require = createRequire(import.meta.url);

const hasDuckdb = spawnSync('duckdb', ['-c', 'select 1'], { encoding: 'utf8' }).status === 0;

const CHUNK = 4096;
const COUNT = 20_000;
const CLUSTERS = 64;
/** A third of the fixture's grid of `ceil(sqrt(clusters))` discs at a spacing of 100. */
const SIDE = Math.ceil(Math.sqrt(CLUSTERS)) * 100;
const BOX: Box = { x: 0, y: 0, w: SIDE / 3, h: SIDE / 3 };

const IDS = [0, 1, COUNT / 2, COUNT - 1].map((i) => `https://example.org/person/${i}`);

interface Opened {
  readonly corpus: Corpus;
  /** The `.parquet` paths named by every statement since the last reset. */
  bill(): number;
  reset(): void;
}

const opened: Record<'files' | 'rowgroups', Opened> = {} as Record<'files' | 'rowgroups', Opened>;
const scratch: string[] = [];

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
  if (!hasDuckdb) return;
  const dist = dirname(require.resolve('@duckdb/duckdb-wasm'));
  const db = await createDuckDB(
    {
      mvp: {
        mainModule: resolve(dist, './duckdb-mvp.wasm'),
        mainWorker: resolve(dist, './duckdb-node-mvp.worker.cjs'),
      },
      eh: {
        mainModule: resolve(dist, './duckdb-eh.wasm'),
        mainWorker: resolve(dist, './duckdb-node-eh.worker.cjs'),
      },
    },
    new ConsoleLogger(),
    NODE_RUNTIME,
  );
  await db.instantiate();
  const conn = db.connect();
  const spill = mkdtempSync(join(tmpdir(), 'fossil-containers-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);

  const root = mkdtempSync(join(tmpdir(), 'fossil-containers-'));
  scratch.push(root);

  for (const layout of ['files', 'rowgroups'] as const) {
    const dir = join(root, layout);
    write(dir, { count: COUNT, clusters: CLUSTERS, layout, chunkSize: CHUNK });
    let files = 0;
    const query: QueryFn = async (sql: string): Promise<QueryRow[]> => {
      files += (sql.match(/\.parquet/g) ?? []).length;
      return conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());
    };
    opened[layout] = {
      corpus: await openCorpus(dir, { query }),
      bill: () => files,
      reset: () => {
        files = 0;
      },
    };
  }
}, 180_000);

/** Both answers, in a form `toEqual` can compare: no `dense_id` order left to the engine. */
const ordered = <T extends { denseId?: bigint; src?: bigint; dst?: bigint }>(rows: readonly T[]) =>
  [...rows].sort((a, b) =>
    String([a.denseId, a.src, a.dst]) < String([b.denseId, b.src, b.dst]) ? -1 : 1,
  );

describe.skipIf(!hasDuckdb)('one corpus, two containers', () => {
  it('declares the container it was written in, and they are not the same one', () => {
    expect(opened.files.corpus.addressing.container).toBe('files');
    expect(opened.rowgroups.corpus.addressing.container).toBe('rowgroups');
    // The comparison below is worthless if both trees are the same container, and that failure is
    // silent: two identical corpora agree about everything.
    expect(opened.files.corpus.addressing.vertexType().tileUrl(3)).toMatch(/chunk3\.parquet$/);
    expect(opened.rowgroups.corpus.addressing.vertexType().tileUrl(3)).toMatch(/tiles\.parquet$/);
  });

  it('says the same thing is inside', () => {
    expect(opened.rowgroups.corpus.types).toEqual(opened.files.corpus.types);
  });

  it('reports the same extent, which comes out of the footers either way', async () => {
    const files = await opened.files.corpus.extent();
    const rowgroups = await opened.rowgroups.corpus.extent();
    expect(rowgroups).toEqual(files);
    expect(files!.maxX).toBeGreaterThan(files!.minX);
  });

  it.each([['src'], ['src', 'dst']] as const)(
    'answers the same window for %s',
    async (...directions) => {
      const files = await opened.files.corpus.window({ ...BOX, directions });
      const rowgroups = await opened.rowgroups.corpus.window({ ...BOX, directions });
      // Non-vacuity, and it is not a formality: two windows that selected nothing are identical,
      // and a box wrong by a factor of a hundred is how this file comes to pass on air.
      expect(files.vertices.length).toBeGreaterThan(0);
      expect(files.vertices.length).toBeLessThan(COUNT);
      expect(files.edges.length).toBeGreaterThan(0);

      expect(rowgroups.tiles).toEqual(files.tiles);
      expect(rowgroups.complete).toBe(files.complete);
      expect(rowgroups.gaps).toEqual(files.gaps);
      expect(ordered(rowgroups.vertices)).toEqual(ordered(files.vertices));
      expect(ordered(rowgroups.edges)).toEqual(ordered(files.edges));
    },
    180_000,
  );

  it('resolves the same vertex by identity, through the index in both', async () => {
    for (const id of [...IDS, 'https://example.org/person/nobody']) {
      expect(await opened.rowgroups.corpus.node(id)).toEqual(await opened.files.corpus.node(id));
    }
    // The seek and not the scan, in both — otherwise this compares two scans and says nothing
    // about the index tiles, which are a payload set of their own and move containers with the rest.
    expect(opened.files.corpus.types.vertices[0]!.indexed).toBe(true);
    expect(opened.rowgroups.corpus.types.vertices[0]!.indexed).toBe(true);
  }, 180_000);

  it.each([1, 2])('walks the same neighbourhood at depth %i', async (depth) => {
    const files = await opened.files.corpus.neighbours(IDS.slice(0, 2), { depth });
    const rowgroups = await opened.rowgroups.corpus.neighbours(IDS.slice(0, 2), { depth });
    expect(files.edges.length).toBeGreaterThan(0);
    expect(ordered(rowgroups.vertices)).toEqual(ordered(files.vertices));
    expect(ordered(rowgroups.edges)).toEqual(ordered(files.edges));
    expect(rowgroups.frontier).toEqual(files.frontier);
    expect(rowgroups.complete).toBe(files.complete);
  }, 180_000);

  it('names strictly fewer files for the same window', async () => {
    const cost: Record<string, number> = {};
    for (const layout of ['files', 'rowgroups'] as const) {
      await opened[layout].corpus.window(BOX); // the first window also pays for the footer sweep
      opened[layout].reset();
      await opened[layout].corpus.window(BOX);
      cost[layout] = opened[layout].bill();
    }
    // The measured claim, in the only currency a local file has. Over HTTP the same shape is 22.3
    // requests per window against 5.6 at five million vertices; here it is a count of paths, and
    // what it has to be is smaller, not a particular number.
    expect(cost.rowgroups).toBeLessThan(cost.files!);
  }, 180_000);
});
