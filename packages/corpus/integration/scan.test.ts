/**
 * `scan` and `edges` against a corpus fossil's own writer wrote — the payload, both adjacencies and
 * the whole cell pyramid with its quotients, through the real layout pass — and held to the two
 * obligations `/docs/design/backend` gives the conformance suite for step 3:
 *
 * 1. **`scan` at `Z` selects what `rows` selects over the same box.**
 * 2. **A rung read through `scan` keeps the aggregation obligations of `/docs/design/cells`**:
 *    membership and count against the level below, the centroid, and mass conservation — every
 *    edge is in the rung's quotient or in a cell's internal weight — with the quotient read
 *    through `edges`.
 *
 * The corpus comes out of `@fossil-lang/executor`, which runs `fossil_df` and the layout pass in
 * wasm, so this is the writer `fossil run` is and not a second one. The JS fixture writes no rung,
 * which is why the checked-in conformance corpus cannot carry obligation 2.
 */

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';

import { FossilExecutor, initFossilExecutor } from '@fossil-lang/executor';
import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import '../tests/boot.js';
import { duckdb } from '../tests/engine.js';
import { open, type Batch, type Box, type Corpus, type TileMatrixSet } from '../src/index.js';
import type { QueryFn } from '../src/query.js';

const require = createRequire(import.meta.url);
const PEOPLE = 20_000;
/** Each person knows the next one round a ring. */
const EDGES = PEOPLE;

/**
 * ONE mapping, where `fossil-cli`'s conformance program has two unioned. The union panics inside
 * the wasm executor — DataFusion 54's `JoinSet::spawn` with no runtime to spawn on — and hangs the
 * run, so the ring is written from the edge source alone: every person is in it once.
 */
const PROGRAM = `type { Person } := io.shex("person.shex")

Knows := io.csv("https://example.org/knows.csv")

Links : Person from Knows
    @subject = "https://example.org/person/{Knows.id}"
    name = Knows.name
    knows = Person(Knows.target)
`;

const SHAPE = JSON.stringify({
  '@context': 'http://www.w3.org/ns/shex.jsonld',
  type: 'Schema',
  shapes: [
    {
      type: 'ShapeDecl',
      id: 'https://example.org/Person',
      shapeExpr: {
        type: 'Shape',
        expression: {
          type: 'EachOf',
          expressions: [
            {
              type: 'TripleConstraint',
              predicate: 'https://example.org/name',
              valueExpr: { type: 'NodeConstraint', datatype: 'http://www.w3.org/2001/XMLSchema#string' },
            },
            { type: 'TripleConstraint', predicate: 'https://example.org/knows', valueExpr: 'https://example.org/Person', min: 0 },
          ],
        },
      },
    },
  ],
});

const scratch: string[] = [];
let engine: Engine;
let query: QueryFn;
let corpus: Corpus;
let set: TileMatrixSet;

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
  await initFossilExecutor(await readFile(require.resolve('@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm')));
  let knows = 'id,name,target\n';
  for (let i = 0; i < PEOPLE; i += 1) knows += `${i},person-${i},${(i + 1) % PEOPLE}\n`;
  const exec = new FossilExecutor(PROGRAM);
  let files;
  try {
    for (const d of exec.missingDocuments()) exec.registerDocument(d.key, SHAPE);
    const encode = (text: string) => new TextEncoder().encode(text);
    ({ files } = await exec.runInMemory(
      { 'https://example.org/knows.csv': encode(knows) },
      's3://jobs/scan',
    ));
  } finally {
    exec.free();
  }
  const root = mkdtempSync(join(tmpdir(), 'fossil-scan-corpus-'));
  scratch.push(root);
  for (const file of files) {
    mkdirSync(dirname(join(root, file.path)), { recursive: true });
    writeFileSync(join(root, file.path), file.bytes);
  }
  const spill = mkdtempSync(join(tmpdir(), 'fossil-scan-spill-'));
  scratch.push(spill);
  ({ engine, query } = await duckdb(spill));
  corpus = await open(root, { engine });
  set = corpus.tileMatrix('Person');
}, 180_000);

const wide = (batch: Batch, name: string): bigint[] =>
  Array.from(batch.getChild(name)!.toArray(), (v) => BigInt(v as number | bigint));
const floats = (batch: Batch, name: string): number[] => Array.from(batch.getChild(name)!.toArray(), Number);
const every = async (z: number, select?: readonly string[]): Promise<Batch[]> => {
  const scan = corpus.scan({ type: 'Person', ...(select === undefined ? {} : { select }) });
  return Promise.all(scan.plan().filter((t) => t.z === z).map((t) => scan.read(t)));
};

describe('the tile matrix set is the cell pyramid', () => {
  it('lists the payload at Z and every rung below it, rung k at z = Z − k', () => {
    const top = set.tileMatrices.length - 1;
    expect(top).toBeGreaterThan(3);
    expect(set.tileMatrices[top]).toMatchObject({ kind: 'rows', count: BigInt(PEOPLE), shift: 0 });
    for (let k = 1; k <= top; k += 1) {
      const matrix = set.tileMatrices[top - k]!;
      const shift = 4 + 2 * (k - 1);
      expect(matrix).toMatchObject({ z: top - k, kind: 'cells', shift });
      expect(matrix.count).toBe(BigInt(Math.max(1, Math.ceil(PEOPLE / 2 ** shift))));
      expect(matrix.tiles.reduce((n, t) => n + t.rows, 0)).toBe(Number(matrix.count));
    }
    expect(set.tileMatrices[0]!.count).toBe(1n);
  });
});

describe('scan at Z against rows — the rowgroups container fossil writes', () => {
  it('selects the same vertices over the same box', async () => {
    const extent = set.extent!;
    const boxes: Box[] = [
      { x: extent.x, y: extent.y, w: extent.w / 4, h: extent.h / 4 },
      { x: extent.x + extent.w / 3, y: extent.y + extent.h / 5, w: extent.w / 10, h: extent.h / 2 },
    ];
    for (const { x, y, w, h } of boxes) {
      const scan = corpus.scan({
        type: 'Person',
        select: ['dense_id'],
        filter: {
          and: [
            { column: 'x', op: '>=', value: x },
            { column: 'x', op: '<', value: x + w },
            { column: 'y', op: '>=', value: y },
            { column: 'y', op: '<', value: y + h },
          ],
        },
      });
      const top = set.tileMatrices.length - 1;
      const tasks = scan.plan().filter((t) => t.z === top);
      expect(tasks.length).toBeLessThan(set.tileMatrices[top]!.tiles.length + 1);
      const read = (await Promise.all(tasks.map((t) => scan.read(t)))).flatMap((b) => wide(b, 'dense_id'));
      const rows = (await corpus.rows({ x, y, w, h, directions: ['src'] })).vertices.map((v) => v.denseId);
      const order = (a: bigint, b: bigint) => (a < b ? -1 : a > b ? 1 : 0);
      expect(read.sort(order)).toEqual(rows.sort(order));
      expect(read.length).toBeGreaterThan(0);
    }
  });

  it('gives a filter on a payload column no task below Z, and a cell column none at Z', async () => {
    const top = set.tileMatrices.length - 1;
    const payload = corpus.scan({ type: 'Person', filter: { column: 'cluster_id', op: '=', value: 0 } });
    expect(new Set(payload.plan().map((t) => t.z))).toEqual(new Set([top]));
    await expect(payload.read({ type: 'Person', z: top - 1, tile: 0 })).rejects.toThrow(/no answer below/);
    const cells = corpus.scan({ type: 'Person', filter: { column: 'count', op: '>', value: 0 } });
    expect(cells.plan().some((t) => t.z === top)).toBe(false);
    expect(cells.plan().filter((t) => t.z === 0)).toHaveLength(1);
  });
});

describe('a rung read through scan keeps the aggregation obligations', () => {
  it('membership and count: a cell holds exactly the rows its id shifts from, at every rung', async () => {
    const top = set.tileMatrices.length - 1;
    const members = new Map<bigint, number>();
    const ids = (await every(top, ['dense_id'])).flatMap((b) => wide(b, 'dense_id'));
    expect(ids).toHaveLength(PEOPLE);
    let below = new Map(ids.map((id) => [id, 1]));
    let shiftBelow = 0;
    for (let z = top - 1; z >= 0; z -= 1) {
      const { shift } = set.tileMatrices[z]!;
      const expected = new Map<bigint, number>();
      for (const [id, count] of below) {
        const cell = id >> BigInt(shift - shiftBelow);
        expected.set(cell, (expected.get(cell) ?? 0) + count);
      }
      const cells = await every(z);
      const got = new Map<bigint, number>();
      for (const batch of cells) {
        const cellIds = wide(batch, 'cell_id');
        const counts = floats(batch, 'count');
        cellIds.forEach((c, i) => got.set(c, counts[i]!));
      }
      expect(got).toEqual(expected);
      below = got;
      shiftBelow = shift;
    }
    members.clear();
  });

  it('the centroid: a cell sits at the mean of its members', async () => {
    const top = set.tileMatrices.length - 1;
    const payload = await every(top, ['dense_id', 'x', 'y']);
    const { shift } = set.tileMatrices[top - 1]!;
    const sums = new Map<bigint, [number, number, number]>();
    for (const batch of payload) {
      const id = wide(batch, 'dense_id');
      const [xs, ys] = [floats(batch, 'x'), floats(batch, 'y')];
      id.forEach((d, i) => {
        const cell = d >> BigInt(shift);
        const s = sums.get(cell) ?? [0, 0, 0];
        sums.set(cell, [s[0] + xs[i]!, s[1] + ys[i]!, s[2] + 1]);
      });
    }
    for (const batch of await every(top - 1)) {
      const cells = wide(batch, 'cell_id');
      const [xs, ys] = [floats(batch, 'x'), floats(batch, 'y')];
      cells.forEach((c, i) => {
        const [sx, sy, n] = sums.get(c)!;
        expect(xs[i]).toBeCloseTo(sx / n, 1);
        expect(ys[i]).toBeCloseTo(sy / n, 1);
      });
    }
  });

  it('mass: every edge is in the rung’s quotient or a cell’s internal weight, and the finest quotient is the payload’s edges shifted, one row per cell pair', async () => {
    const top = set.tileMatrices.length - 1;
    const tilesAt = (z: number) => set.tileMatrices[z]!.tiles.map((t) => ({ type: 'Person', z, tile: t.tile }));
    const payload = new Map<string, bigint>();
    const { shift } = set.tileMatrices[top - 1]!;
    let edges = 0;
    for (const from of tilesAt(top)) {
      const { batches, declined } = await corpus.edges({ from, direction: 'src' });
      expect(declined).toEqual([]);
      for (const b of batches) {
        edges += b.src.length;
        b.src.forEach((s, i) => {
          // The quotient is undirected: one row per cell PAIR, keyed on the lower cell, which is
          // what `crates/fossil-layout/src/layout/cells.rs, pair_edges` writes.
          const [a, d] = [s >> BigInt(shift), b.dst[i]! >> BigInt(shift)];
          const [lo, hi] = a < d ? [a, d] : [d, a];
          if (lo !== hi) payload.set(`${lo}→${hi}`, (payload.get(`${lo}→${hi}`) ?? 0n) + 1n);
        });
      }
    }
    expect(edges).toBe(EDGES);
    let checked = 0;
    for (let z = top - 1; z >= 0; z -= 1) {
      const internal = (await every(z)).flatMap((b) => wide(b, 'internal')).reduce((a, b) => a + b, 0n);
      let cross = 0n;
      const quotient = new Map<string, bigint>();
      for (const from of tilesAt(z)) {
        const answer = await corpus.edges({ from, direction: 'src' });
        for (const b of answer.batches) {
          expect(b.weight).not.toBeNull();
          b.src.forEach((s, i) => {
            cross += b.weight![i]!;
            quotient.set(`${s}→${b.dst[i]}`, b.weight![i]!);
          });
        }
        if (answer.batches.length > 0) checked += 1;
        expect((await corpus.edges({ from, direction: 'dst' })).declined).toEqual([
          { edgeType: 'knows', direction: 'dst', reason: 'not-declared' },
        ]);
      }
      expect(cross + internal).toBe(BigInt(EDGES));
      if (z === top - 1) expect(quotient).toEqual(payload);
    }
    expect(checked).toBeGreaterThan(0);
  }, 60_000);
});

describe('edges at Z, both halves, against the adjacency on disk', () => {
  it('reads CSR at src and CSC at dst, one tile each', async () => {
    const top = set.tileMatrices.length - 1;
    const root = corpus.url;
    for (const tile of [0, set.tileMatrices[top]!.tiles.length - 1]) {
      for (const [direction, dir, key] of [
        ['src', 'by_source', 'src_dense'],
        ['dst', 'by_target', 'dst_dense'],
      ] as const) {
        const [batch] = (await corpus.edges({ from: { type: 'Person', z: top, tile }, direction })).batches;
        const truth = await query(
          `SELECT count(*) AS n FROM read_parquet('${root}/edge/Person_knows_Person/${dir}/tiles.parquet')
            WHERE ${key} >> 12 = ${tile}`,
        );
        expect(batch!.src.length).toBe(Number(truth[0]!['n']));
        expect(Array.from(direction === 'src' ? batch!.src : batch!.dst).every((v) => v >> 12n === BigInt(tile))).toBe(true);
      }
    }
  });
});
