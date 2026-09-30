/**
 * What a window costs, as a shape rather than a number.
 *
 * `window` — the first reader of a rectangle, before `scan` — was handed every tile URL in the corpus
 * and pruned with a `WHERE` for as long as it existed. It answered correctly every time, and the eighty-eight tests beside it never saw
 * it: **a window returning the right answer looks exactly like a window returning it cheaply.** It
 * was found from outside the repository by timing one rectangle against two corpus sizes.
 *
 * So this file asserts cost, and it asserts it as a relationship to N. A pinned number would not
 * have caught that defect — the number was always correct and only the slope was wrong — so nothing
 * here compares against a recorded figure. Every expectation is either "smaller than the corpus" or
 * "the same at ten times the size".
 *
 * The seam it measures through is the one the package already has. `open` asks a host for one
 * `Engine`, so everything a reader does passes through `Engine.query` and counting is a decorator
 * around it: no instrumentation inside `packages/corpus/src`, and nothing
 * here can drift from what a real host would see.
 *
 * **Two corpora, written by the checker's own fixture** — `packages/corpus/guards/fixture.mjs`, which
 * is JavaScript against the published conventions and imports nothing of ours. Ten times the
 * vertices at the same `chunk_size`, so one is 20 tiles and the other 196, and the fixture lays its
 * clusters on a grid of fixed spacing: the same rectangle covers the same ground at both sizes and
 * only the density changes. That is what makes "the same window" mean something across two corpora.
 *
 * It needs the `duckdb` binary, which is why it lives in `packages/corpus/integration/` beside the
 * guards that speak to it and not in `packages/corpus/tests/`. The dependency is DECLARED —
 * `pnpm --filter @fossil-lang/corpus test:integration` is the script that has it, and the
 * package's own `pnpm test` no longer does. It is not probed and this file does not skip:
 * `describe.skipIf` skips the TESTS and runs the describe BODY, so the probe it was guarding
 * never guarded the fixture write, and a suite that vanishes with its dependency reads as
 * covered while asserting nothing.
 */

import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import '../tests/boot.js';
import { open, type Corpus, type TileAddress } from '../src/index.js';
import { duckdb } from '../tests/engine.js';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../guards/fixture.mjs';

/** Rows per tile, small enough that ten times the corpus is ten times the tiles and still cheap. */
const CHUNK = 1024;
const SMALL = 20_000;
const BIG = 200_000;

let corpora: { name: string; corpus: Corpus; tiles: number; seen: () => string[]; reset: () => void }[] = [];
const scratch: string[] = [];

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
  const spill = mkdtempSync(join(tmpdir(), 'fossil-cost-spill-'));
  scratch.push(spill);
  const { engine } = await duckdb(spill);

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
    const counted: Engine = {
      ...engine,
      query(sql, options) {
        seen.push(sql);
        return engine.query(sql, options);
      },
    };
    const corpus = await open(dir, { engine: counted });
    corpora.push({
      name,
      corpus,
      tiles: Math.ceil(count / CHUNK),
      seen: () => seen,
      reset: () => {
        seen = [];
      },
    });
  }
}, 120_000);

const filesIn = (sql: string): number => (sql.match(/\.parquet/g) ?? []).length;

/**
 * The same rectangle at both sizes — a quarter of the extent from the corner the extent reports —
 * read the way a view reads it: the scan's tasks at `Z`, then both halves of their edges.
 * Fixed ground, ten times the density, which is the control that makes the counts comparable.
 */
async function windowed(entry: (typeof corpora)[number]) {
  const type = entry.corpus.types.vertices[0]!.type;
  const set = entry.corpus.tileMatrix(type);
  const { x, y, w, h } = set.extent!;
  const scan = entry.corpus.scan({
    type,
    filter: {
      and: [
        { column: 'x', op: '>=', value: x },
        { column: 'x', op: '<', value: x + w * 0.25 },
        { column: 'y', op: '>=', value: y },
        { column: 'y', op: '<', value: y + h * 0.25 },
      ],
    },
    select: ['dense_id'],
  });
  const z = set.tileMatrices.length - 1;
  const tasks: TileAddress[] = scan.plan().filter((t) => t.z === z);
  entry.reset();
  const batches = await scan.read(tasks);
  const vertices = [...entry.seen()];
  entry.reset();
  await entry.corpus.edges({ from: tasks, direction: 'src' });
  await entry.corpus.edges({ from: tasks, direction: 'dst' });
  const edges = [...entry.seen()];
  const runs = tasks.filter((t, k) => k === 0 || t.tile !== tasks[k - 1]!.tile + 1).length;
  return {
    tasks,
    runs,
    landed: batches.filter((b) => b.numRows > 0).length,
    rows: batches.reduce((n, b) => n + b.numRows, 0),
    vertexFiles: vertices.reduce((n, sql) => n + filesIn(sql), 0),
    vertices,
    edges,
  };
}

describe('what a window costs', () => {
  it('never names every tile in the corpus', async () => {
    for (const entry of corpora) {
      const read = await windowed(entry);
      // The defect exactly: the vertex statement used to carry all of them, at every corpus size.
      expect(read.vertexFiles, `${entry.name}: vertex statements`).toBeLessThan(entry.tiles);
      expect(read.rows).toBeGreaterThan(0);
    }
  }, 120_000);

  it('opens the tiles its plan named, and about one more than its answer landed in', async () => {
    for (const entry of corpora) {
      const read = await windowed(entry);
      // A tile whose box meets the rectangle and whose rows do not is planned and returns nothing.
      // That overshoot is geometry — how the boxes overlap the corner — and not a term in N, which
      // is why it is a constant here and the slope is asserted below.
      expect(read.vertexFiles, `${entry.name}: vertex statements`).toBe(read.tasks.length);
      expect(read.tasks.length, `${entry.name}: planned`).toBeLessThanOrEqual(read.landed + 2);
    }
  }, 120_000);

  it('opens a smaller share of a bigger corpus', async () => {
    const shares: number[] = [];
    for (const entry of corpora) {
      const read = await windowed(entry);
      shares.push(read.vertexFiles / entry.tiles);
    }
    // Ten times the corpus for the same rectangle: the share has to fall. It was 1.0 at both sizes
    // when the statement listed everything, which is the only way this can be flat.
    expect(shares[1]!).toBeLessThan(shares[0]!);
  }, 120_000);

  it('is one statement per run of tiles, for the vertices and for each orientation', async () => {
    for (const entry of corpora) {
      const read = await windowed(entry);
      expect(read.vertices, `${entry.name}: vertices`).toHaveLength(read.runs);
      expect(read.edges.length, `${entry.name}: edges`).toBeLessThanOrEqual(2 * read.runs);
      // And the orientations were always addressed — kept here so a change that broke it could not
      // hide behind the vertex statement being right.
      for (const sql of read.edges) expect(filesIn(sql)).toBeLessThan(entry.tiles);
    }
  }, 120_000);
});
