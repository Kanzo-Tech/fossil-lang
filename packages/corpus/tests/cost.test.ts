/**
 * What a window costs, as a shape rather than a number.
 *
 * `window` was handed every tile URL in the corpus and pruned with a `WHERE` for as long as it
 * existed. It answered correctly every time, and the eighty-eight tests beside this file never saw
 * it: **a window returning the right answer looks exactly like a window returning it cheaply.** It
 * was found from outside the repository by timing one rectangle against two corpus sizes.
 *
 * So this file asserts cost, and it asserts it as a relationship to N. A pinned number would not
 * have caught that defect — the number was always correct and only the slope was wrong — so nothing
 * here compares against a recorded figure. Every expectation is either "smaller than the corpus" or
 * "the same at ten times the size".
 *
 * The seam it measures through is the one the package already has. `openCorpus` asks a host for a
 * single `query` callback, so everything a reader does passes through one function and counting is
 * a decorator around it: no instrumentation inside `corpus.ts`, and nothing here can drift from
 * what a real host would see.
 *
 * **Two corpora, written by the checker's own fixture** — `apps/corpus/guards/fixture.mjs`, which
 * is JavaScript against the published conventions and imports nothing of ours. Ten times the
 * vertices at the same `chunk_size`, so one is 20 tiles and the other 196, and the fixture lays its
 * clusters on a grid of fixed spacing: the same rectangle covers the same ground at both sizes and
 * only the density changes. That is what makes "the same window" mean something across two corpora.
 *
 * It needs the `duckdb` binary for the fixture, and skips without one rather than failing — the
 * guards next door make the same trade for the same reason.
 */

import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { openCorpus, type Corpus } from '../src/corpus.js';
import type { QueryFn, QueryRow } from '../src/query.js';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../../../apps/corpus/guards/fixture.mjs';

const require = createRequire(import.meta.url);

/** Whether a corpus can be written here at all. */
const hasDuckdb = spawnSync('duckdb', ['-c', 'select 1'], { encoding: 'utf8' }).status === 0;

/** Rows per tile, small enough that ten times the corpus is ten times the tiles and still cheap. */
const CHUNK = 1024;
const SMALL = 20_000;
const BIG = 200_000;

/**
 * One window's bill.
 *
 * `queries` is how many statements reached the host. `files` is the `.parquet` paths named in each
 * of them, in order — a window's first statement is the vertices and the rest are the orientations,
 * so the first entry is the one this file is about.
 */
interface Bill {
  readonly queries: number;
  readonly files: readonly number[];
}

let corpora: { name: string; corpus: Corpus; tiles: number; bill: () => Bill; reset: () => void }[] = [];
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
  const spill = mkdtempSync(join(tmpdir(), 'fossil-cost-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);

  const root = mkdtempSync(join(tmpdir(), 'fossil-cost-'));
  scratch.push(root);

  corpora = [];
  for (const [name, count] of [
    ['small', SMALL],
    ['big', BIG],
  ] as const) {
    const dir = join(root, name);
    write(dir, { count, clusters: 256, layout: 'files', chunkSize: CHUNK });

    let seen: string[] = [];
    const query: QueryFn = async (sql: string): Promise<QueryRow[]> => {
      seen.push(sql);
      return conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());
    };
    const corpus = await openCorpus(dir, { query });
    corpora.push({
      name,
      corpus,
      tiles: Math.ceil(count / CHUNK),
      bill: () => ({ queries: seen.length, files: seen.map((s) => (s.match(/\.parquet/g) ?? []).length) }),
      reset: () => {
        seen = [];
      },
    });
  }
}, 120_000);

/**
 * The same rectangle at both sizes — a quarter of the extent from the corner the extent reports.
 * Fixed ground, ten times the density, which is the control that makes the counts comparable.
 */
async function windowed(entry: (typeof corpora)[number]) {
  const type = entry.corpus.types.vertices[0]!.type;
  const extent = (await entry.corpus.extent(type))!;
  const box = {
    x: extent.minX,
    y: extent.minY,
    w: (extent.maxX - extent.minX) * 0.25,
    h: (extent.maxY - extent.minY) * 0.25,
    type,
  };
  await entry.corpus.window(box); // the first window of a corpus also pays for the footer sweep
  entry.reset();
  const answer = await entry.corpus.window(box);
  return { answer, bill: entry.bill() };
}

describe.skipIf(!hasDuckdb)('what a window costs', () => {
  it('never names every tile in the corpus', async () => {
    for (const entry of corpora) {
      const { answer, bill } = await windowed(entry);
      // The defect exactly: the vertex statement used to carry all of them, at every corpus size.
      expect(bill.files[0], `${entry.name}: vertex statement`).toBeLessThan(entry.tiles);
      expect(answer.vertices.length).toBeGreaterThan(0);
    }
  }, 120_000);

  it('opens the tiles its own answer landed in, and about one more', async () => {
    for (const entry of corpora) {
      const { answer, bill } = await windowed(entry);
      // A tile whose box meets the rectangle and whose rows do not is opened and returns nothing.
      // That overshoot is geometry — how the boxes overlap the corner — and not a term in N, which
      // is why it is a constant here and the slope is asserted below.
      expect(bill.files[0], `${entry.name}: vertex statement`).toBeGreaterThanOrEqual(answer.tiles.length);
      expect(bill.files[0], `${entry.name}: vertex statement`).toBeLessThanOrEqual(answer.tiles.length + 2);
    }
  }, 120_000);

  it('opens a smaller share of a bigger corpus', async () => {
    const shares: number[] = [];
    for (const entry of corpora) {
      const { bill } = await windowed(entry);
      shares.push(bill.files[0]! / entry.tiles);
    }
    // Ten times the corpus for the same rectangle: the share has to fall. It was 1.0 at both sizes
    // when the statement listed everything, which is the only way this can be flat.
    expect(shares[1]!).toBeLessThan(shares[0]!);
  }, 120_000);

  it('is one statement for the vertices and one per orientation', async () => {
    for (const entry of corpora) {
      const { bill } = await windowed(entry);
      expect(bill.queries, `${entry.name}`).toBe(3);
      // And the orientations were always addressed — this is the half that was already right, kept
      // here so a change that broke it could not hide behind the vertex statement being fixed.
      expect(bill.files[1]).toBeLessThan(entry.tiles);
      expect(bill.files[2]).toBeLessThan(entry.tiles);
    }
  }, 120_000);
});
