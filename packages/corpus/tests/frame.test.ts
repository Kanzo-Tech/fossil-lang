import { mkdtempSync, rmSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import './boot.js';
import { levelsOf, rowsAt, strideOf } from '../src/address.js';
import { openCorpus, type Corpus, type Frame } from '../src/corpus.js';
import type { QueryFn, QueryRow } from '../src/query.js';

/**
 * The door a camera goes through: `levels` and `frame` — against the conformance corpus.
 *
 * **This file is about the CONTRACT and not about a picture.** The three properties the request is
 * really after — fidelity, monotone refinement, path independence — are measured against the
 * million-vertex bench corpus by `apps/playground/scripts/verify-properties.mjs`, because they are
 * statistical and a 300-vertex fixture cannot carry a density grid. What is asserted here is what a
 * conformance corpus CAN carry and a large one cannot check cheaply:
 *
 * 1. **`frame` is a pure function of its arguments.** The same rectangle at the same level, asked
 *    twice with different questions in between, is the same answer down to the row order. That is
 *    the Zarr property in one sentence, and it is why {@link FrameParams.level} survived the level
 *    moving inside `frame`: path independence cannot be *stated* over a derived level, because
 *    stating it means naming the same level twice. Every property test below spells the level.
 * 2. **Level 0 selects what `rows` selects.** The two members take a rectangle and are not the
 *    same question; this is the invariant that keeps them one contract rather than two.
 * 3. **A level nests.** A multiple of `strideOf(k+1)` is a multiple of `strideOf(k)`, so refining
 *    only ever adds — asserted over every level the type has, which in quarters at 300 vertices is
 *    five of them.
 * 4. **The derived level never lies about the direction.** A bigger canvas never needs a coarser
 *    level than a smaller one over the same rectangle, which is what makes a camera's zoom
 *    sequence sane. It is the one thing left of `levelFor`, and it is asserted through `frame`
 *    because there is no second function to ask.
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

const drawn = (frame: Frame): bigint[] =>
  [...frame.denseIds.slice(0, frame.marks)].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));

describe('levels — the multiscale metadata', () => {
  it('lists every level from all of it down to one vertex', () => {
    const levels = levelsOf(corpus.addressing);
    expect(levels[0]).toMatchObject({ level: 0, stride: 1, count: VERTEX_COUNT, written: false });
    // In quarters `strideOf(5)` = 1,024 is the first stride at or above 300, so five levels above
    // zero. In halves it was nine, and a camera crossed two of them per zoom step.
    expect(levels).toHaveLength(6);
    expect(levels.at(-1)).toMatchObject({ level: 5, stride: 1024, count: 1 });
    for (const { level, stride, count } of levels) {
      expect(stride).toBe(Number(strideOf(level)));
      expect(count).toBe(Math.ceil(VERTEX_COUNT / stride));
    }
  });

  it('reports which levels are written, and the corpus writes the two it plans', () => {
    // `VertexLevels::planned(300, 64)` answers `[1, 2]` and the conformance corpus is regenerated
    // with exactly that. Before it had a pyramid this asserted that NOTHING was written, and said
    // the day `l{k}/` existed it would flip and no other answer would change. It flipped; the test
    // below is the half that says nothing else did.
    expect(levelsOf(corpus.addressing).filter((l) => l.written).map((l) => l.level)).toEqual([1, 2]);
  });
});

describe('the level a canvas asks for, derived inside the frame', () => {
  it('draws every vertex onto a canvas with a pixel to spare for each', async () => {
    // One mark per pixel is the whole conversion, so a canvas of at least `VERTEX_COUNT` pixels
    // over the whole corpus is level 0 — every row, nothing decimated.
    const box = await everything();
    const wide = await corpus.frame({ ...box, pixels: { w: VERTEX_COUNT, h: 1 }, links: false });
    expect(wide.level).toBe(0);
    expect(wide.marks).toBe(VERTEX_COUNT);
  });

  it('is monotone in the canvas: more pixels is never a coarser level', async () => {
    const box = await everything();
    let previous = Number.POSITIVE_INFINITY;
    for (const side of [1, 2, 4, 8, 16, 32]) {
      const { level } = await corpus.frame({ ...box, pixels: { w: side, h: side }, links: false });
      expect(level).toBeLessThanOrEqual(previous);
      previous = level;
    }
  });

  it('answers the COARSEST level that fits the canvas, and not one coarser', async () => {
    // The estimate is `tiles × chunk_size` capped by the count — over the whole corpus, `5 × 64`
    // capped at 300 rows. Asserted as the definition rather than as a formula: the level it draws
    // fits the canvas and the one below it does not, which is what "coarsest that fits" means and
    // what restating `log4` here would only re-derive.
    const { level } = await corpus.frame({
      ...(await everything()),
      pixels: { w: CHUNK_SIZE, h: 1 },
      links: false,
    });
    expect(levelsOf(corpus.addressing)[level]!.count).toBeLessThanOrEqual(CHUNK_SIZE);
    expect(levelsOf(corpus.addressing)[level - 1]!.count).toBeGreaterThan(CHUNK_SIZE);
  });

  it('refuses a frame that names neither a canvas nor a level', async () => {
    await expect(corpus.frame({ ...(await everything()) })).rejects.toThrow(/needs a resolution/);
  });
});

describe('frame — the same rectangle at the same level is the same answer', () => {
  it('is a pure function of its arguments, whatever was asked in between', async () => {
    const box = await everything();
    const first = await corpus.frame({ ...box, level: 2 });
    // Three questions that would move any state a session-dependent reader could be keeping: a
    // different level, a different rectangle, and the other member entirely.
    await corpus.frame({ ...box, level: 0 });
    await corpus.frame({ x: box.x, y: box.y, w: box.w / 4, h: box.h / 4, level: 4 });
    await corpus.rows(box);
    const again = await corpus.frame({ ...box, level: 2 });

    expect(again.marks).toBe(first.marks);
    expect(again.matched).toBe(first.matched);
    expect([...again.denseIds]).toEqual([...first.denseIds]);
    expect([...again.positions]).toEqual([...first.positions]);
    expect([...again.links]).toEqual([...first.links]);
    expect(again.cost.tiles).toBe(first.cost.tiles);
    expect(again.cost.bytes).toBe(first.cost.bytes);
  });

  it('selects at level 0 exactly what rows selects over the same rectangle', async () => {
    const box = await everything();
    const frame = await corpus.frame({ ...box, level: 0 });
    const entire = await corpus.rows(box);
    expect(frame.marks).toBe(entire.vertices.length);
    expect(frame.matched).toBe(entire.vertices.length);
    expect(drawn(frame)).toEqual(entire.vertices.map((v) => v.denseId).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)));
  });

  it('nests: every level is a subset of the one below it, at the same positions', async () => {
    const box = await everything();
    const place = new Map<bigint, string>();
    let coarser: bigint[] | null = null;
    // Descending, so `previous` is always the coarser of the pair and containment is the claim.
    for (let level = levelsOf(corpus.addressing).length - 1; level >= 0; level -= 1) {
      const frame = await corpus.frame({ ...box, level });
      const ids = drawn(frame);
      for (let i = 0; i < frame.marks; i += 1) {
        const at = `${frame.positions[i * 2]},${frame.positions[i * 2 + 1]}`;
        const held = place.get(frame.denseIds[i]!);
        if (held !== undefined) expect(at).toBe(held);
        place.set(frame.denseIds[i]!, at);
      }
      if (coarser !== null) for (const id of coarser) expect(ids).toContain(id);
      expect(frame.stride).toBe(Number(strideOf(level)));
      coarser = ids;
    }
  });

  it('counts what the rectangle holds at the level it counted, and says which that was', async () => {
    const box = await everything();
    for (const level of [0, 1, 3, 5]) {
      const frame = await corpus.frame({ ...box, level });
      expect(frame.marks).toBe(Number(rowsAt(BigInt(VERTEX_COUNT), level)));
      // A read that came from `l{k}/` cannot count level 0 without opening the bytes the pyramid
      // exists to avoid, so `matched` is counted at `matchedAt` and never silently at zero. A
      // strided read opened the payload, so it can and does.
      expect(frame.matched).toBe(Number(rowsAt(BigInt(VERTEX_COUNT), frame.matchedAt)));
      // And `matchedAt` is the only thing that says which artefact replied — a level above zero is
      // a written `l{k}/` and nothing else can report one. There is no second field restating it.
      expect(frame.matchedAt).toBe(levelsOf(corpus.addressing)[level]!.written ? level : 0);
    }
  });

  it('draws off a written level exactly what the predicate selects from level 0', async () => {
    // The property the whole pyramid rests on: a written `l{k}/` is a CACHE of `dense_id % 4^k`,
    // so the corpus answers the same ids either way and a reader never has to know which artefact
    // replied. Untestable here until the conformance corpus had a pyramid, which it now does.
    const box = await everything();
    const zero = drawn(await corpus.frame({ ...box, level: 0 }));
    expect(zero).toHaveLength(VERTEX_COUNT);
    for (const level of [1, 2]) {
      const stride = strideOf(level);
      const selected = zero.filter((id) => id % stride === 0n);
      // Non-vacuity, and it is not ceremony: two empty lists are equal, and a `view` that answered
      // nothing would satisfy the assertion below without the corpus having a pyramid at all.
      expect(selected).toHaveLength(Number(rowsAt(BigInt(VERTEX_COUNT), level)));
      expect(drawn(await corpus.frame({ ...box, level }))).toEqual(selected);
    }
  });

  it('bills in requests and bytes, and a written level costs fewer of both', async () => {
    const box = await everything();
    // Level 1 is written, so `l1/` answers. Level 3 is not, so the payload is opened and strided —
    // the same rows, more bytes.
    const written = await corpus.frame({ ...box, level: 1 });
    const strided = await corpus.frame({ ...box, level: 3 });
    for (const frame of [written, strided]) {
      expect(frame.cost.tiles).toBeGreaterThan(0);
      expect(frame.cost.bytes).toBeGreaterThan(0);
      expect(frame.cost.requests).toBeLessThanOrEqual(frame.cost.tiles);
    }
    // And the point of writing it: the level read opens strictly fewer bytes than striding the
    // payload for a level that is FINER, so the saving is not an artefact of drawing less.
    expect(written.cost.bytes).toBeLessThan(strided.cost.bytes);
  });

  it('brings a pin back whatever the level, and puts its tile on the ledger', async () => {
    const box = await everything();
    // A `dense_id` that is odd is in no level above zero — which is exactly what a pin is for.
    const odd = 1n;
    const bare = await corpus.frame({ ...box, level: 5 });
    const pinned = await corpus.frame({ ...box, level: 5, pinned: [odd] });
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
    await expect(corpus.frame({ ...box, level: -1 })).rejects.toThrow(/non-negative integer/);
    await expect(corpus.frame({ ...box, level: 1.5 })).rejects.toThrow(/non-negative integer/);
  });

  it('answers an empty rectangle with an empty frame rather than with everything', async () => {
    const extent = (await corpus.extent())!;
    const away = { x: extent.maxX + 1e6, y: extent.maxY + 1e6, w: 1, h: 1, level: 0 };
    const frame = await corpus.frame(away);
    expect(frame.marks).toBe(0);
    expect(frame.matched).toBe(0);
    expect(frame.cost.tiles).toBe(0);
  });
});
