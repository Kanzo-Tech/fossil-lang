/**
 * The Z-order half of the addressing: a rectangle, without a reader.
 *
 * `address.ts` used to say which tiles a rectangle touches *"cannot be re-derived from the corpus
 * itself"*, and the layout pass falsified it by renumbering `dense_id` into Morton order. What is
 * under test here is the decomposition that replaced the sentence, and it is tested in three
 * layers, because each one can be self-consistently wrong:
 *
 *   1. **The primitives against the published table.** `quantize` and `morton2` also exist in Rust
 *      (`crates/fossil-layout/src/layout.rs`) and in JavaScript-that-imports-nothing
 *      (`apps/corpus/guards/arithmetic.mjs`); this is the third implementation, and all three read
 *      `apps/corpus/guards/vectors.json` rather than transcribing it. The width matters —
 *      `quantize(147, 0, 167)` is 57687 in binary32 and 57686 in binary64 — so a literal
 *      transcription of the formula is a *different function* and the table is what says so.
 *   2. **The decomposition against a corpus it did not build.** A synthetic renumbering — points,
 *      codes, a sort, tiles of a fixed size — and then the only property that matters, checked by
 *      brute force over every rectangle in a sweep: **every tile holding a point inside the
 *      rectangle is in the answer.** A decomposition that returns fewer has not saved bytes, it has
 *      dropped vertices that are on screen, and no count sees that.
 *   3. **The composition through `resolveCorpus`.** `tilesForBox` is `mortonTilesFor` fed into
 *      `tilesFor`, and what it must not do is answer with a different shape than the tile-number
 *      door beside it.
 */

import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import {
  type Box,
  type TileCodes,
  MORTON_BITS,
  gridBoxOf,
  morton2,
  mortonDecode,
  mortonOf,
  mortonTilesFor,
  quantize,
  resolveCorpus,
  tilesForGrid,
} from '../src/address.js';

const vectors = JSON.parse(
  await readFile(
    fileURLToPath(new URL('../../../apps/corpus/guards/vectors.json', import.meta.url)),
    'utf8',
  ),
) as {
  morton2: { vectors: Array<{ x: number; y: number; morton: number }> };
  quantize: { vectors: Array<{ v: number; lo: number; hi: number; q: number }> };
};

describe('the published arithmetic, executed rather than restated', () => {
  it('reproduces every `quantize` vector, in the width the table declares', () => {
    expect(vectors.quantize.vectors.length).toBeGreaterThan(0);
    for (const { v, lo, hi, q } of vectors.quantize.vectors) {
      expect(quantize(v, lo, hi)).toBe(q);
    }
  });

  it('reproduces every `morton2` vector, unsigned past bit 31', () => {
    expect(vectors.morton2.vectors.length).toBeGreaterThan(0);
    for (const { x, y, morton } of vectors.morton2.vectors) {
      expect(morton2(x, y)).toBe(morton);
      // Without the `>>> 0`, `spread(y) << 1` reaching bit 31 makes this negative — and a corpus
      // ordered by a negative code sorts the top half of the plane before the bottom half.
      expect(morton2(x, y)).toBeGreaterThanOrEqual(0);
    }
  });

  it('decodes back to the coordinates it interleaved', () => {
    for (const { x, y, morton } of vectors.morton2.vectors) {
      expect(mortonDecode(morton)).toEqual({ x, y });
    }
    for (let x = 0; x < 64; x += 7) {
      for (let y = 0; y < 64; y += 5) {
        expect(mortonDecode(morton2(x, y))).toEqual({ x, y });
      }
    }
  });
});

describe('gridBoxOf', () => {
  const extent: Box = { xlo: 0, xhi: 100, ylo: 0, yhi: 100 };

  it('is exact rather than padded, because quantise is monotone', () => {
    // `v >= box.xlo` implies `quantize(v) >= quantize(box.xlo)`, so no widening is needed for the
    // containment to hold — and widening would be over-read nobody asked for.
    const grid = gridBoxOf({ xlo: 25, xhi: 75, ylo: 25, yhi: 75 }, extent);
    expect(grid).toEqual({
      xlo: quantize(25, 0, 100),
      xhi: quantize(75, 0, 100),
      ylo: quantize(25, 0, 100),
      yhi: quantize(75, 0, 100),
    });
  });

  it('answers `null` for a rectangle the extent does not reach, rather than clamping into it', () => {
    // `quantize` CLAMPS, so a box entirely left of the extent would otherwise come back as the
    // column of cells at x = 0 — a full answer to a question with an empty one.
    expect(gridBoxOf({ xlo: -50, xhi: -10, ylo: 25, yhi: 75 }, extent)).toBeNull();
    expect(gridBoxOf({ xlo: 25, xhi: 75, ylo: 120, yhi: 200 }, extent)).toBeNull();
    expect(gridBoxOf({ xlo: 75, xhi: 25, ylo: 25, yhi: 75 }, extent)).toBeNull();
    // Touching at one edge is not disjoint.
    expect(gridBoxOf({ xlo: -50, xhi: 0, ylo: 0, yhi: 100 }, extent)).not.toBeNull();
  });
});

/**
 * A renumbering, done the way the layout pass does it: quantise, interleave, rank, cut into tiles.
 *
 * The points are a grid of clumps rather than a uniform scatter, because a uniform one is the only
 * distribution for which the ranks and the codes happen to agree — and agreeing is exactly what
 * `TileCodes` exists because they do not do.
 */
function renumber(chunkSize: number): {
  extent: Box;
  codes: TileCodes;
  tileOfPoint: (index: number) => number;
  points: Array<{ x: number; y: number }>;
} {
  const points: Array<{ x: number; y: number }> = [];
  for (let cluster = 0; cluster < 16; cluster += 1) {
    const cx = (cluster % 4) * 100;
    const cy = Math.floor(cluster / 4) * 100;
    // Denser clusters on one diagonal, so the ranks are emphatically not uniform in the codes.
    const n = 20 + cluster * 9;
    for (let k = 0; k < n; k += 1) {
      const angle = 2.3999632 * k;
      const radius = 2 * Math.sqrt(k);
      points.push({ x: cx + radius * Math.cos(angle), y: cy + radius * Math.sin(angle) });
    }
  }
  const extent: Box = points.reduce<Box>(
    (a, p) => ({
      xlo: Math.min(a.xlo, p.x),
      xhi: Math.max(a.xhi, p.x),
      ylo: Math.min(a.ylo, p.y),
      yhi: Math.max(a.yhi, p.y),
    }),
    { xlo: Infinity, xhi: -Infinity, ylo: Infinity, yhi: -Infinity },
  );

  const coded = points.map((p, index) => ({ index, code: mortonOf(p.x, p.y, extent) }));
  coded.sort((a, b) => a.code - b.code || a.index - b.index);
  const denseOf = new Array<number>(points.length);
  coded.forEach((p, dense) => {
    denseOf[p.index] = dense;
  });

  const tiles = Math.ceil(points.length / chunkSize);
  const lo = new Uint32Array(tiles);
  const hi = new Uint32Array(tiles);
  for (let k = 0; k < tiles; k += 1) {
    lo[k] = coded[k * chunkSize]!.code;
    hi[k] = coded[Math.min((k + 1) * chunkSize, coded.length) - 1]!.code;
  }
  return { extent, codes: { lo, hi }, tileOfPoint: (index) => Math.floor(denseOf[index]! / chunkSize), points };
}

describe('tilesForGrid over a corpus it did not build', () => {
  const chunkSize = 32;
  const { extent, codes, tileOfPoint, points } = renumber(chunkSize);

  it('is a renumbering with the shape the writer produces', () => {
    expect(codes.lo.length).toBe(Math.ceil(points.length / chunkSize));
    // Non-decreasing in k, which is what makes the two binary searches sound.
    for (let k = 1; k < codes.lo.length; k += 1) {
      expect(codes.lo[k]!).toBeGreaterThanOrEqual(codes.hi[k - 1]!);
    }
  });

  it('misses nothing, over every rectangle in a sweep', () => {
    let windows = 0;
    let overRead = 0;
    let needed = 0;
    for (const fraction of [0.05, 0.15, 0.4, 0.9]) {
      for (const cx of [0.2, 0.5, 0.8]) {
        for (const cy of [0.3, 0.5, 0.7]) {
          const w = (extent.xhi - extent.xlo) * fraction;
          const h = (extent.yhi - extent.ylo) * fraction;
          const x = extent.xlo + (extent.xhi - extent.xlo) * cx;
          const y = extent.ylo + (extent.yhi - extent.ylo) * cy;
          const box: Box = { xlo: x - w / 2, xhi: x + w / 2, ylo: y - h / 2, yhi: y + h / 2 };

          const need = new Set(
            points
              .map((p, index) => ({ p, index }))
              .filter(
                ({ p }) =>
                  p.x >= box.xlo && p.x <= box.xhi && p.y >= box.ylo && p.y <= box.yhi,
              )
              .map(({ index }) => tileOfPoint(index)),
          );
          if (need.size === 0) continue;
          windows += 1;
          const answered = new Set(mortonTilesFor({ box, extent, codes }));
          for (const tile of need) expect(answered.has(tile)).toBe(true);
          overRead += answered.size;
          needed += need.size;
        }
      }
    }
    expect(windows).toBeGreaterThan(10);
    // It over-covers, because the curve enters and leaves the rectangle. What it must not do is
    // over-cover without bound — a decomposition that answers "every tile" also misses nothing.
    expect(overRead / needed).toBeLessThan(1.5);
  });

  it('answers nothing for a rectangle beyond the extent', () => {
    const far = { xlo: 1e6, xhi: 2e6, ylo: 1e6, yhi: 2e6 };
    expect(mortonTilesFor({ box: far, extent, codes })).toEqual([]);
  });

  it('answers every tile for a rectangle that holds the whole extent', () => {
    const all = mortonTilesFor({ box: extent, extent, codes });
    expect(all.length).toBe(codes.lo.length);
  });

  it('descends no deeper than the grid', () => {
    expect(MORTON_BITS).toBe(16);
    // A single cell is the floor of the recursion: `level === 0` stops it, so a degenerate box
    // terminates rather than recursing on itself.
    expect(() => tilesForGrid({ xlo: 3, xhi: 3, ylo: 3, yhi: 3 }, codes)).not.toThrow();
    expect(tilesForGrid({ xlo: 0, xhi: 0, ylo: 0, yhi: 0 }, { lo: [], hi: [] })).toEqual([]);
  });
});

describe('tilesForBox through a resolved corpus', () => {
  const manifestFiles: Record<string, string> = {
    'graph.graph.yml': [
      'name: g',
      "prefix: ''",
      'container: rowgroups',
      'vertices:',
      '- person.vertex.yml',
      'edges: []',
      'version: gar/v1',
    ].join('\n'),
    'person.vertex.yml': [
      'type: Person',
      'chunk_size: 32',
      'prefix: vertex/Person/',
      'vertex_count: 96',
      'version: gar/v1',
    ].join('\n'),
  };

  const extent: Box = { xlo: 0, xhi: 100, ylo: 0, yhi: 100 };
  /**
   * Three tiles cutting the code space into three runs, aligned to the top-level quadrants.
   *
   * The quadrants are the four ranges of 2³⁰ the curve visits in order — bottom-left, bottom-right,
   * top-left, top-right — so tile 0 is the bottom-left quadrant, tile 1 the two middle ones, and
   * tile 2 the top-right. A code range and a *rectangle* are different shapes, which is the whole
   * reason the decomposition descends a tree instead of comparing two boxes.
   */
  const QUADRANT = 2 ** 30;
  const codes: TileCodes = {
    lo: [0, QUADRANT, 3 * QUADRANT],
    hi: [QUADRANT - 1, 3 * QUADRANT - 1, 2 ** 32 - 1],
  };

  it('answers the same shape the tile-number door does', () => {
    const corpus = resolveCorpus({ manifestFiles, base: '/bench' });
    const box: Box = { xlo: 70, xhi: 90, ylo: 70, yhi: 90 };
    const byBox = corpus.tilesForBox({ box, extent, codes });
    const byNumber = corpus.tilesFor({ tiles: mortonTilesFor({ box, extent, codes }) });
    expect(byBox).toEqual(byNumber);
    expect(byBox.type).toBe('Person');
    expect(byBox.vertexUrls).toEqual(['/bench/vertex/Person/tiles.parquet']);
    // The top-right quadrant is tile 2's ground and nobody else's.
    expect(byBox.tiles).toEqual([2]);
  });

  it('answers no URL at all for a rectangle off the extent', () => {
    const corpus = resolveCorpus({ manifestFiles });
    const off = corpus.tilesForBox({ box: { xlo: 500, xhi: 600, ylo: 0, yhi: 10 }, extent, codes });
    expect(off.tiles).toEqual([]);
    expect(off.vertexUrls).toEqual([]);
  });
});
