import { mkdtempSync, rmSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { openCorpus, type Corpus, type View } from '../src/corpus.js';
import { initFossilGraphWasm } from '../src/load.js';
import type { QueryFn, QueryRow } from '../src/query.js';

/**
 * The door a camera goes through: `levels`, `levelFor`, `view` — against the conformance corpus.
 *
 * **This file is about the CONTRACT and not about a picture.** The three properties the request is
 * really after — fidelity, monotone refinement, path independence — are measured against the
 * million-vertex bench corpus by `apps/playground/scripts/verify-properties.mjs`, because they are
 * statistical and a 300-vertex fixture cannot carry a density grid. What is asserted here is what a
 * conformance corpus CAN carry and a large one cannot check cheaply:
 *
 * 1. **`view` is a pure function of its arguments.** The same rectangle at the same level, asked
 *    twice with different questions in between, is the same answer down to the row order. That is
 *    the Zarr property in one sentence and it is the whole reason the level is an argument.
 * 2. **Level 0 selects what `window` selects.** The two members take a rectangle and are not the
 *    same question; this is the invariant that keeps them one contract rather than two.
 * 3. **A level nests.** `dense_id % 2^(k+1) == 0` is a strict subset of `dense_id % 2^k == 0`, so
 *    refining only ever adds — asserted over every level the type has, which at 300 vertices is
 *    nine of them.
 * 4. **`levelFor` never lies about the direction.** A smaller rectangle never needs a coarser level
 *    than a bigger one containing it, and a bigger budget never needs a coarser level than a
 *    smaller one. Both are monotonicity, and both are what makes a camera's zoom sequence sane.
 */

const require = createRequire(import.meta.url);
const CORPUS = fileURLToPath(new URL('../../../apps/corpus/conformance/corpus', import.meta.url));

/** The manifest's own numbers, so a hard-coded stride shows up as a failure. */
const VERTEX_COUNT = 300;
const CHUNK_SIZE = 64;

let query: QueryFn;
let corpus: Corpus;
const scratch: string[] = [];

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
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
  const spill = mkdtempSync(join(tmpdir(), 'fossil-view-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);
  query = async (sql: string): Promise<QueryRow[]> =>
    conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());
  await initFossilGraphWasm({
    wasmUrl: (await readFile(
      fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
    )) as unknown as URL,
  });
  corpus = await openCorpus(CORPUS, { query });
}, 60_000);

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

const drawn = (view: View): bigint[] => [...view.denseIds.slice(0, view.marks)].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));

describe('levels — the multiscale metadata', () => {
  it('lists every level from all of it down to one vertex', () => {
    const levels = corpus.levels();
    expect(levels[0]).toMatchObject({ level: 0, stride: 1, count: VERTEX_COUNT, written: false });
    // 2^9 = 512 is the first stride at or above 300, so nine levels above zero.
    expect(levels).toHaveLength(10);
    expect(levels.at(-1)).toMatchObject({ level: 9, stride: 512, count: 1 });
    for (const { level, stride, count } of levels) {
      expect(stride).toBe(2 ** level);
      expect(count).toBe(Math.ceil(VERTEX_COUNT / stride));
    }
  });

  it('reports every level as unwritten, because nothing writes one yet', () => {
    // The day `vertex/<Type>/l{k}/` exists this flips and NO OTHER ANSWER CHANGES — which is the
    // property that lets the pyramid be added later without becoming a second contract.
    expect(corpus.levels().every((l) => !l.written)).toBe(true);
  });
});

describe('levelFor — the budget, and it is a separate function', () => {
  it('answers 0 when the whole corpus fits', async () => {
    expect(corpus.levelFor({ ...(await everything()), budget: VERTEX_COUNT })).toBe(0);
    expect(corpus.levelFor({ ...(await everything()), budget: VERTEX_COUNT * 10 })).toBe(0);
  });

  it('is monotone in the budget: more room is never a coarser level', async () => {
    const box = await everything();
    let previous = Number.POSITIVE_INFINITY;
    for (const budget of [1, 2, 4, 8, 16, 32, 64, 128, 256, 512]) {
      const level = corpus.levelFor({ ...box, budget });
      expect(level).toBeLessThanOrEqual(previous);
      previous = level;
    }
  });

  it('is monotone in the rectangle: a sub-rectangle is never a coarser level', async () => {
    const outer = await everything();
    for (const fraction of [0.8, 0.5, 0.25, 0.1]) {
      const inner = {
        x: outer.x + (outer.w * (1 - fraction)) / 2,
        y: outer.y + (outer.h * (1 - fraction)) / 2,
        w: outer.w * fraction,
        h: outer.h * fraction,
      };
      expect(corpus.levelFor({ ...inner, budget: 32 })).toBeLessThanOrEqual(
        corpus.levelFor({ ...outer, budget: 32 }),
      );
    }
  });

  it('estimates over tiles rather than area, so a 64-row budget lands within one level', async () => {
    // The estimate is `tiles × chunk_size` capped by the count, so over the whole corpus it is
    // exactly `5 × 64 = 320` against 300 rows — and `ceil(log2(320 / 64))` is 3 where the true
    // answer over 300 is also 3. Asserted against the arithmetic rather than against a constant
    // transcribed from a run.
    const level = corpus.levelFor({ ...(await everything()), budget: CHUNK_SIZE });
    expect(level).toBe(Math.ceil(Math.log2(320 / CHUNK_SIZE)));
    expect(corpus.levels()[level]!.count).toBeLessThanOrEqual(CHUNK_SIZE);
  });
});

describe('view — the same rectangle at the same level is the same answer', () => {
  it('is a pure function of its arguments, whatever was asked in between', async () => {
    const box = await everything();
    const first = await corpus.view({ ...box, level: 2 });
    // Three questions that would move any state a session-dependent reader could be keeping: a
    // different level, a different rectangle, and the other member entirely.
    await corpus.view({ ...box, level: 0 });
    await corpus.view({ x: box.x, y: box.y, w: box.w / 4, h: box.h / 4, level: 4 });
    await corpus.window(box);
    const again = await corpus.view({ ...box, level: 2 });

    expect(again.marks).toBe(first.marks);
    expect(again.matched).toBe(first.matched);
    expect([...again.denseIds]).toEqual([...first.denseIds]);
    expect([...again.positions]).toEqual([...first.positions]);
    expect([...again.links]).toEqual([...first.links]);
    expect(again.cost.tiles).toBe(first.cost.tiles);
    expect(again.cost.bytes).toBe(first.cost.bytes);
  });

  it('selects at level 0 exactly what window selects over the same rectangle', async () => {
    const box = await everything();
    const view = await corpus.view({ ...box, level: 0 });
    const rows = await corpus.window(box);
    expect(view.marks).toBe(rows.vertices.length);
    expect(view.matched).toBe(rows.vertices.length);
    expect(drawn(view)).toEqual(rows.vertices.map((v) => v.denseId).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)));
  });

  it('nests: every level is a subset of the one below it, at the same positions', async () => {
    const box = await everything();
    const place = new Map<bigint, string>();
    let coarser: bigint[] | null = null;
    // Descending, so `previous` is always the coarser of the pair and containment is the claim.
    for (let level = corpus.levels().length - 1; level >= 0; level -= 1) {
      const view = await corpus.view({ ...box, level });
      const ids = drawn(view);
      for (let i = 0; i < view.marks; i += 1) {
        const at = `${view.positions[i * 2]},${view.positions[i * 2 + 1]}`;
        const held = place.get(view.denseIds[i]!);
        if (held !== undefined) expect(at).toBe(held);
        place.set(view.denseIds[i]!, at);
      }
      if (coarser !== null) for (const id of coarser) expect(ids).toContain(id);
      expect(view.stride).toBe(2 ** level);
      coarser = ids;
    }
  });

  it('counts what the rectangle holds at level 0, not what came back', async () => {
    const box = await everything();
    for (const level of [0, 1, 3, 5]) {
      const view = await corpus.view({ ...box, level });
      expect(view.matched).toBe(VERTEX_COUNT);
      expect(view.marks).toBe(Math.ceil(VERTEX_COUNT / 2 ** level));
    }
  });

  it('reports which artefact addressed the tiles and which one answered the level', async () => {
    const view = await corpus.view({ ...(await everything()), level: 1 });
    // The conformance corpus publishes `codes:`, so the tiles come from the anchor and no
    // rectangle-to-tile question reaches a Parquet reader.
    expect(view.cost.addressed).toBe('anchor');
    expect(view.cost.read).toBe('strided');
    expect(view.cost.tiles).toBeGreaterThan(0);
    expect(view.cost.bytes).toBeGreaterThan(0);
    expect(view.cost.runs).toBeLessThanOrEqual(view.cost.tiles);
  });

  it('brings a pin back whatever the level, and puts its tile on the ledger', async () => {
    const box = await everything();
    // A `dense_id` that is odd is in no level above zero — which is exactly what a pin is for.
    const odd = 1n;
    const bare = await corpus.view({ ...box, level: 5 });
    const pinned = await corpus.view({ ...box, level: 5, pinned: [odd] });
    expect(drawn(bare)).not.toContain(odd);
    expect(drawn(pinned)).toContain(odd);
    // Every other drawn vertex is still there: a pin adds and never replaces.
    for (const id of drawn(bare)) expect(drawn(pinned)).toContain(id);
    // Its tile was already in the rectangle here, so the ledger does not grow — what matters is
    // that the pin did not enter the answer without being counted.
    expect(pinned.cost.tiles).toBeGreaterThanOrEqual(bare.cost.tiles);
  });

  it('refuses a level that is not a level rather than answering for a nearby one', async () => {
    const box = await everything();
    await expect(corpus.view({ ...box, level: -1 })).rejects.toThrow(/non-negative integer/);
    await expect(corpus.view({ ...box, level: 1.5 })).rejects.toThrow(/non-negative integer/);
  });

  it('answers an empty rectangle with an empty view rather than with everything', async () => {
    const extent = (await corpus.extent())!;
    const away = { x: extent.maxX + 1e6, y: extent.maxY + 1e6, w: 1, h: 1, level: 0 };
    const view = await corpus.view(away);
    expect(view.marks).toBe(0);
    expect(view.matched).toBe(0);
    expect(view.cost.tiles).toBe(0);
  });
});
