import { describe, expect, it } from 'vitest';
import type { Frame } from '@fossil-lang/corpus';

import { residency } from '../src/index.js';

/**
 * A frame in the door's own shape — marks first, then anchors.
 *
 * `points` are `[denseId, x, y, category]` for the marks; `anchors` the same for rows that are
 * only holding an edge down. `links` are pairs of row indices, as `Frame.links` is.
 */
function frame(
  points: readonly [number, number, number, number][],
  links: readonly number[] = [],
  anchors: readonly [number, number, number][] = [],
): Frame {
  const rows = points.length + anchors.length;
  const denseIds = new BigUint64Array(rows);
  const positions = new Float32Array(rows * 2);
  const categories = new Uint32Array(rows);
  points.forEach(([dense, x, y, cat], i) => {
    denseIds[i] = BigInt(dense);
    positions[i * 2] = x;
    positions[i * 2 + 1] = y;
    categories[i] = cat;
  });
  anchors.forEach(([dense, x, y], i) => {
    const at = points.length + i;
    denseIds[at] = BigInt(dense);
    positions[at * 2] = x;
    positions[at * 2 + 1] = y;
  });
  return {
    denseIds,
    positions,
    categories,
    marks: points.length,
    links: Uint32Array.from(links),
  } as unknown as Frame;
}

const box = (x: number, y: number, w: number, h: number) => ({ x, y, w, h });

describe('residency — what is held', () => {
  it('hands back the SAME object, so a re-derivation is byte-identical', () => {
    const held = residency();
    const answer = frame([[0, 1, 1, 7]]);
    held.hold('k', answer);
    expect(held.held('k')).toBe(answer);
    expect(held.frames).toBe(1);
    expect(held.points).toBe(1);
  });

  it('evicts least-recently-used first, and a read is a use', () => {
    const held = residency({ points: 2 });
    held.hold('a', frame([[0, 0, 0, 0]]));
    held.hold('b', frame([[2, 0, 0, 0]]));
    held.held('a'); // `a` becomes the most recently used
    held.hold('c', frame([[4, 0, 0, 0]]));
    expect(held.held('b')).toBeUndefined();
    expect(held.held('a')).toBeDefined();
    expect(held.held('c')).toBeDefined();
  });

  it('never evicts the entry just held, even when it alone exceeds the budget', () => {
    const held = residency({ points: 1 });
    const big = frame([
      [0, 0, 0, 0],
      [2, 0, 0, 0],
      [4, 0, 0, 0],
    ]);
    held.hold('big', big);
    expect(held.held('big')).toBe(big);
    expect(held.frames).toBe(1);
  });

  it('release lets go of everything', () => {
    const held = residency();
    held.hold('k', frame([[0, 1, 1, 0]]));
    held.release();
    expect(held.frames).toBe(0);
    expect(held.points).toBe(0);
    expect(held.visible(box(0, 0, 10, 10), 10)).toBeNull();
  });
});

describe('residency — the interim picture', () => {
  it('is null when nothing held is inside the rectangle', () => {
    const held = residency();
    held.hold('k', frame([[0, 100, 100, 0]]));
    expect(held.visible(box(0, 0, 1, 1), 10)).toBeNull();
    expect(residency().visible(box(0, 0, 1, 1), 10)).toBeNull();
  });

  it('is half-open on the far edge, as the box predicate on the door is', () => {
    const held = residency();
    held.hold('k', frame([[0, 0, 0, 0], [2, 1, 1, 0]]));
    const seen = held.visible(box(0, 0, 1, 1), 10);
    expect(seen!.marks).toBe(1);
    expect(seen!.denseIds[0]).toBe(0n);
  });

  it('deduplicates a vertex two held answers both carry', () => {
    const held = residency();
    held.hold('a', frame([[2, 0.5, 0.5, 3]]));
    held.hold('b', frame([[2, 0.5, 0.5, 3]]));
    const seen = held.visible(box(0, 0, 1, 1), 10);
    expect(seen!.marks).toBe(1);
    expect(seen!.matched).toBe(1);
  });

  it('cuts to the limit by a power-of-two stride, and reports the uncut count', () => {
    const held = residency();
    const points = Array.from({ length: 8 }, (_, i) => [i, 0.5, 0.5, 0] as [number, number, number, number]);
    held.hold('k', frame(points));
    const seen = held.visible(box(0, 0, 1, 1), 4);
    expect(seen!.matched).toBe(8);
    // Every surviving `dense_id` is a multiple of the same power of two.
    const ids = [...seen!.denseIds.slice(0, seen!.marks)].map(Number);
    expect(ids.every((d) => d % 2 === 0)).toBe(true);
    expect(seen!.marks).toBeLessThanOrEqual(4);
  });

  it('is nested: a finer cut keeps every vertex a coarser one drew', () => {
    const held = residency();
    const points = Array.from({ length: 16 }, (_, i) => [i, 0.5, 0.5, 0] as [number, number, number, number]);
    held.hold('k', frame(points));
    const coarse = held.visible(box(0, 0, 1, 1), 2)!;
    const fine = held.visible(box(0, 0, 1, 1), 16)!;
    const fineIds = new Set([...fine.denseIds.slice(0, fine.marks)].map(Number));
    for (const id of [...coarse.denseIds.slice(0, coarse.marks)].map(Number)) {
      expect(fineIds.has(id)).toBe(true);
    }
  });

  it('carries a link whose far end is outside, as an anchor past the marks', () => {
    const held = residency();
    held.hold('k', frame([[0, 0.5, 0.5, 1]], [0, 1], [[2, 9, 9]]));
    const seen = held.visible(box(0, 0, 1, 1), 10)!;
    expect(seen.marks).toBe(1);
    expect(seen.links).toEqual(Uint32Array.from([0, 1]));
    expect(seen.denseIds.length).toBe(2);
    expect(seen.positions[2]).toBe(9);
  });

  it('drops a link with neither end drawn', () => {
    const held = residency();
    held.hold('k', frame([[0, 0.5, 0.5, 0]], [1, 2], [[2, 9, 9], [4, 9, 9]]));
    const seen = held.visible(box(0, 0, 1, 1), 10)!;
    expect(seen.links.length).toBe(0);
  });
});
