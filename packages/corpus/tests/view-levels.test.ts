import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { strideOf } from '../src/address.js';
import { openCorpus, type Corpus } from '../src/corpus.js';
import { initFossilGraphWasm } from '../src/load.js';
import type { QueryFn, QueryRow } from '../src/query.js';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../../../apps/corpus/guards/fixture.mjs';

/**
 * **The read side of the pyramid**, against a corpus that has one.
 *
 * `view.test.ts` cannot ask any of this: the conformance corpus is 300 vertices in 5 tiles and
 * writes no `l{k}/`, deliberately, because giving it one would mean growing the artefact every
 * implementation of this format is checked against.
 *
 * **The corpus here is written by `apps/corpus/guards/fixture.mjs`, and this file writes no
 * Parquet of its own.** It used to: it built a payload and a pyramid out of inline SQL, which made
 * it a FOURTH writer of level bytes beside the layout pass, the fixture and the guards — and it
 * existed only because a floor constant kept the conformance corpus from ever having a pyramid to
 * read. The floor is gone with the move to quarters, the fixture takes the levels it is told, and a
 * test that writes the bytes it then reads is testing its own SQL.
 *
 * **Which levels those are is NOT decided here.** `VertexLevels::planned` is the one
 * implementation of that policy; {@link LEVELS} is the plan for this corpus's size transcribed as a
 * list, exactly as the fixture's own header asks for. What the assertions are about is the read.
 *
 * **What is asserted is the one property the pyramid is not allowed to break.** A level file is a
 * cache of `dense_id % strideOf(k) == 0` over the payload and nothing else, so for every written
 * level the rows a level read answers with are exactly the rows the predicate selects out of
 * `window` — same ids, same positions, same order. If those two can disagree there are two
 * contracts, and a reader would have to know which artefact answered in order to know what it was
 * looking at.
 *
 * The cost is asserted too, in the only direction that is a fact rather than a compressor's
 * opinion, and against the ONE control that isolates it: **the same corpus written without a
 * pyramid.** Same vertices, same positions, same tiling, same rectangle, same level — the only
 * difference is whether `l{k}/` is on disk, so the byte difference is what the pyramid buys and
 * nothing else. Asking for links used to be the control, and it stopped being one the day the
 * fixture began writing the relation's levels too.
 *
 * Needs the `duckdb` binary for the fixture, and skips without one rather than failing — the guards
 * next door make the same trade for the same reason.
 */

const require = createRequire(import.meta.url);

/** Whether a corpus can be written here at all. */
const hasDuckdb = spawnSync('duckdb', ['-c', 'select 1'], { encoding: 'utf8' }).status === 0;

/** The corpus this file reads: 600 vertices at 16 to a tile is 38 tiles. */
const ROWS = 600;
const CHUNK = 16;
/**
 * What `VertexLevels::planned(600, 16)` plans, and this fixture is therefore told to write.
 *
 * Complete, `1..=coarsest`: `ceil(600 / strideOf(3))` is 10, which fits one tile of 16, and
 * `ceil(600 / strideOf(2))` is 38, which does not. Transcribed and not derived — a second copy of
 * the plan here is the drift the fixture's own header refuses.
 */
const LEVELS = [1, 2, 3];

let query: QueryFn;
/** The corpus with the pyramid, and the identical one without it. */
let corpus: Corpus;
let flat: Corpus;
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
  const spill = mkdtempSync(join(tmpdir(), 'fossil-levels-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);
  query = async (sql: string): Promise<QueryRow[]> =>
    conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());

  // The same options twice but for `levels`, which is what makes the second one a control: the
  // fixture is deterministic in its inputs, so the two corpora differ in the `l{k}/` sets and in
  // nothing else.
  const options = { count: ROWS, clusters: 16, layout: 'files' as const, chunkSize: CHUNK };
  const withLevels = mkdtempSync(join(tmpdir(), 'fossil-levels-corpus-'));
  const withoutLevels = mkdtempSync(join(tmpdir(), 'fossil-levels-flat-'));
  scratch.push(withLevels, withoutLevels);
  write(withLevels, { ...options, levels: LEVELS });
  write(withoutLevels, { ...options, levels: [] });

  await initFossilGraphWasm({
    wasmUrl: (await readFile(
      fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
    )) as unknown as URL,
  });
  corpus = await openCorpus(withLevels, { query });
  flat = await openCorpus(withoutLevels, { query });
}, 180_000);

/** The whole corpus as a rectangle, from the footers rather than from a constant. */
async function everything() {
  const extent = (await corpus.extent())!;
  const pad = 1e-3;
  return {
    x: extent.minX - pad,
    y: extent.minY - pad,
    w: extent.maxX - extent.minX + 2 * pad,
    h: extent.maxY - extent.minY + 2 * pad,
  };
}

describe.skipIf(!hasDuckdb)('a corpus that declares a pyramid', () => {
  it('reports exactly the written levels, and every other level as answerable anyway', () => {
    const written = corpus.levels('Person').filter((l) => l.written);
    expect(written.map((l) => l.level)).toEqual(LEVELS);
    // Every level is in the list, written or not: a level is a predicate, and `written` is a cost.
    const all = corpus.levels('Person');
    expect(all[0]!.level).toBe(0);
    expect(all[0]!.written).toBe(false);
    for (const { level, stride, count } of all) {
      expect(stride).toBe(strideOf(level));
      expect(count).toBe(Math.ceil(ROWS / stride));
    }
    // The control declares none, and every level is still listed and still answerable.
    expect(flat.levels('Person').every((l) => !l.written)).toBe(true);
    expect(flat.levels('Person').map((l) => l.level)).toEqual(all.map((l) => l.level));
  });
});

describe.skipIf(!hasDuckdb)('a level read answers with what the predicate selects', () => {
  it('is the same rows as striding the payload, at every written level', async () => {
    const box = await everything();
    const rows = await corpus.window({ ...box });
    for (const level of LEVELS) {
      const step = BigInt(strideOf(level));
      const expected = rows.vertices
        .map((v) => v.denseId)
        .filter((id) => id % step === 0n)
        .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      const view = await corpus.view({ ...box, level, links: false });
      expect(view.cost.read).toBe('level');
      expect([...view.denseIds.slice(0, view.marks)]).toEqual(expected);
      // The positions come out of a different FILE and have to be the same numbers: a level that
      // carried the right ids at the wrong coordinates draws a picture that is wrong about where
      // everything is, and an id-only assertion is green for it.
      for (let i = 0; i < view.marks; i += 1) {
        const id = view.denseIds[i]!;
        const row = rows.vertices.find((v) => v.denseId === id)!;
        expect(view.positions[i * 2]).toBeCloseTo(row.x, 5);
        expect(view.positions[i * 2 + 1]).toBeCloseTo(row.y, 5);
      }
    }
  });

  it('opens strictly fewer bytes than the same corpus without a pyramid', async () => {
    const box = await everything();
    const level = LEVELS[0]!;
    const cheap = await corpus.view({ ...box, level, links: false });
    const dear = await flat.view({ ...box, level, links: false });
    expect(cheap.cost.read).toBe('level');
    // Same rectangle, same level, same rows — and the control has to stride the payload for them,
    // because it has no `l{k}/` to read them out of.
    expect(dear.cost.read).toBe('strided');
    expect([...cheap.denseIds.slice(0, cheap.marks)]).toEqual([
      ...dear.denseIds.slice(0, dear.marks),
    ]);
    expect(cheap.cost.bytes).toBeLessThan(dear.cost.bytes);
    expect(cheap.cost.tiles).toBeLessThan(dear.cost.tiles);
  });

  it('draws the lines off the pyramid too, because the relation has one', async () => {
    // A level file of a RELATION carries both endpoints' coordinates, so a view that asked for
    // links is still answerable off the pyramid — which is the whole reason the edge levels exist.
    // Where any incident relation is missing its level set the read falls back to the payload, and
    // `cost.read` is where that is reported rather than in a second contract.
    const box = await everything();
    const level = LEVELS[0]!;
    const linked = await corpus.view({ ...box, level, links: true });
    expect(linked.cost.read).toBe('level');
    expect(await flat.view({ ...box, level, links: true }).then((v) => v.cost.read)).toBe('strided');
  });

  it('counts `matched` at the level it read, and says which level that is', async () => {
    const box = await everything();
    const level = LEVELS[1]!;
    const cheap = await corpus.view({ ...box, level, links: false });
    const strided = await flat.view({ ...box, level, links: false });
    expect(cheap.matchedAt).toBe(level);
    expect(cheap.matched).toBe(Math.ceil(ROWS / strideOf(level)));
    // The payload read still counts level 0, because it read level 0's bytes.
    expect(strided.matchedAt).toBe(0);
    expect(strided.matched).toBe(ROWS);
  });

  it('strides the payload for a level nobody wrote, and says so', async () => {
    const box = await everything();
    const level = LEVELS[LEVELS.length - 1]! + 1;
    expect(corpus.levels('Person').find((l) => l.level === level)?.written).toBe(false);
    const view = await corpus.view({ ...box, level, links: false });
    expect(view.cost.read).toBe('strided');
    expect(view.matchedAt).toBe(0);
    expect(view.marks).toBe(Math.ceil(ROWS / strideOf(level)));
  });

  it('brings a pin back off the payload, because no level carries an odd id', async () => {
    const box = await everything();
    const level = LEVELS[0]!;
    const bare = await corpus.view({ ...box, level, links: false });
    const pinned = await corpus.view({ ...box, level, links: false, pinned: [7] });
    expect(pinned.cost.read).toBe('level');
    expect([...pinned.denseIds.slice(0, pinned.marks)]).toContain(7n);
    expect(pinned.marks).toBe(bare.marks + 1);
    // The pin's PAYLOAD tile was opened for it, and a fetch that happened is a fetch on the ledger.
    expect(pinned.cost.bytes).toBeGreaterThan(bare.cost.bytes);
  });
});
