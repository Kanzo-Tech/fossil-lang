/**
 * One corpus, two containers, the same answers — asked of the published module.
 *
 * A tile is a fixed range of `dense_id`. Which container carries it — one Parquet per tile with the
 * address in the name, or one Parquet whose row groups are the tiles — is a second question, and
 * `graph.graph.yml`'s `container` is what answers it. `open` reads both, and this is the file
 * that says so: the same graph is written both ways and every answer has to come back identical.
 *
 * **This is the diff a format change is allowed to arrive behind**, and its plain-Node twin is
 * `packages/corpus/conformance/containers.mjs`, which asks the same question of a reader that shares no
 * line with this one. Two implementations moved and compared BEFORE a writer emits the new
 * container; after it emits one, the only corpus that exists is the new one and there is nothing
 * left to compare against.
 *
 * **What it cannot prove.** That the row-group container is cheaper, which is the entire reason for
 * it — the measured win is HTTP requests per window and a local file makes none. What it asserts
 * instead is the countable half: the same window names strictly fewer files.
 *
 * **Why `chunk_size` is 4,096 here and 1,024 next door.** DuckDB emits row groups in multiples of
 * its 2,048-row vector, so a `ROW_GROUP_SIZE` under that is silently clamped and the tiles land
 * somewhere else — 300 rows written at `ROW_GROUP_SIZE 64` come back as one row group of 300. It is
 * a limit of the tool that writes the fixture, not of the format, and it is why the checked-in
 * conformance corpus (`chunk_size` 64) cannot be repacked into the other container.
 *
 * It needs the `duckdb` binary, which is why it lives in `packages/corpus/integration/` beside the
 * guards that speak to it and not in `packages/corpus/tests/`. The dependency is DECLARED —
 * `pnpm --filter @fossil-lang/corpus test:integration` is the script that has it, and the
 * package's own `pnpm test` no longer does. It is not probed and this file does not skip:
 * `describe.skipIf` skips the TESTS and runs the describe BODY, so the probe it was guarding
 * never guarded the fixture write, and a suite that vanishes with its dependency reads as
 * covered while asserting nothing.
 */

import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import '../tests/boot.js';
import { open, type Batch, type Box, type Corpus, type Direction, type Filter, type TileAddress } from '../src/index.js';
import { duckdb } from '../tests/engine.js';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../guards/fixture.mjs';

const CHUNK = 4096;
const COUNT = 20_000;
const CLUSTERS = 64;
/** A third of the fixture's grid of `ceil(sqrt(clusters))` discs at a spacing of 100. */
const SIDE = Math.ceil(Math.sqrt(CLUSTERS)) * 100;
const BOX: Box = { x: 0, y: 0, w: SIDE / 3, h: SIDE / 3 };
const IN_BOX: Filter = {
  and: [
    { column: 'x', op: '>=', value: BOX.x },
    { column: 'x', op: '<', value: BOX.x + BOX.w },
    { column: 'y', op: '>=', value: BOX.y },
    { column: 'y', op: '<', value: BOX.y + BOX.h },
  ],
};

interface Opened {
  readonly dir: string;
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
  const spill = mkdtempSync(join(tmpdir(), 'fossil-containers-spill-'));
  scratch.push(spill);
  const { engine } = await duckdb(spill);

  const root = mkdtempSync(join(tmpdir(), 'fossil-containers-'));
  scratch.push(root);

  for (const layout of ['files', 'rowgroups'] as const) {
    const dir = join(root, layout);
    write(dir, { count: COUNT, clusters: CLUSTERS, layout, chunkSize: CHUNK });
    let files = 0;
    const counted: Engine = {
      ...engine,
      query(sql, options) {
        files += (sql.match(/\.parquet/g) ?? []).length;
        return engine.query(sql, options);
      },
    };
    opened[layout] = {
      dir,
      corpus: await open(dir, { engine: counted }),
      bill: () => files,
      reset: () => {
        files = 0;
      },
    };
  }
}, 180_000);

const values = (batch: Batch, name: string): string[] =>
  Array.from(batch.getChild(name)!.toArray(), (v) => String(v));

/** The box's vertices and every edge of the tiles holding them, in a form `toEqual` can compare. */
async function window(corpus: Corpus, directions: readonly Direction[]) {
  const scan = corpus.scan({ type: 'Person', filter: IN_BOX, select: ['dense_id', 'x', 'y'] });
  const payloadZ = corpus.tileMatrix('Person').tileMatrices.length - 1;
  const tasks: TileAddress[] = scan.plan().filter((t) => t.z === payloadZ);
  const vertices = (await scan.read(tasks)).flatMap((b) => {
    const [ids, xs, ys] = ['dense_id', 'x', 'y'].map((c) => values(b, c));
    return ids!.map((id, i) => `${id} ${xs![i]} ${ys![i]}`);
  });
  const edges: string[] = [];
  for (const direction of directions) {
    for (const answer of await corpus.edges({ from: tasks, direction })) {
      for (const batch of answer.batches) {
        batch.src.forEach((src, i) => edges.push(`${direction} ${src} ${batch.dst[i]}`));
      }
    }
  }
  return { tiles: tasks.map((t) => t.tile), vertices: vertices.sort(), edges: edges.sort() };
}

describe('one corpus, two containers', () => {
  it('was written in the container it declares, and they are not the same one', () => {
    // The comparison below is worthless if both trees are the same container, and that failure is
    // silent: two identical corpora agree about everything.
    expect(existsSync(join(opened.files.dir, 'vertex/Person/chunk3.parquet'))).toBe(true);
    expect(existsSync(join(opened.files.dir, 'vertex/Person/tiles.parquet'))).toBe(false);
    expect(existsSync(join(opened.rowgroups.dir, 'vertex/Person/tiles.parquet'))).toBe(true);
    expect(existsSync(join(opened.rowgroups.dir, 'vertex/Person/chunk3.parquet'))).toBe(false);
  });

  it('says the same thing is inside', () => {
    expect(opened.rowgroups.corpus.types).toEqual(opened.files.corpus.types);
  });

  it('publishes the same tile matrix, boxes and extent included', () => {
    const files = opened.files.corpus.tileMatrix('Person');
    expect(opened.rowgroups.corpus.tileMatrix('Person')).toEqual(files);
    expect(files.extent!.w).toBeGreaterThan(0);
  });

  it.each([['src'], ['src', 'dst']] as const)(
    'answers the same window for %s',
    async (...directions) => {
      const files = await window(opened.files.corpus, directions);
      const rowgroups = await window(opened.rowgroups.corpus, directions);
      // Non-vacuity, and it is not a formality: two windows that selected nothing are identical,
      // and a box wrong by a factor of a hundred is how this file comes to pass on air.
      expect(files.vertices.length).toBeGreaterThan(0);
      expect(files.vertices.length).toBeLessThan(COUNT);
      expect(files.edges.length).toBeGreaterThan(0);
      expect(rowgroups).toEqual(files);
    },
    180_000,
  );

  it('names strictly fewer files for the same window', async () => {
    const cost: Record<string, number> = {};
    for (const layout of ['files', 'rowgroups'] as const) {
      opened[layout].reset();
      await window(opened[layout].corpus, ['src', 'dst']);
      cost[layout] = opened[layout].bill();
    }
    // The measured claim, in the only currency a local file has. Over HTTP the same shape is 22.3
    // requests per window against 5.6 at five million vertices; here it is a count of paths, and
    // what it has to be is smaller, not a particular number.
    expect(cost.rowgroups).toBeLessThan(cost.files!);
  }, 180_000);
});
