/**
 * **Residency — what is loaded, as opposed to what is drawn.**
 *
 * `/docs/design/camera` names three states a reader that pans across more data than it can hold
 * keeps apart: residency (what is loaded), visibility (what is drawn out of what is loaded) and the
 * camera. This module is the first of them, and it is a module of its own for the reason that page
 * gives — **it can be tested without a camera.** It knows no rectangle it was not handed and issues
 * no query: a host's seam is the one thing that calls the door, and it hands the answers here
 * afterwards. `apps/playground/src/tiles.ts` is that seam in fossil's own tree.
 *
 * ## What it holds, and why that is not tiles
 *
 * camera.mdx describes residency as *a cache over tiles*, and **a host on this seam cannot hold a
 * tile.** The reads are `read_parquet('<url>')` issued by DuckDB from inside its own Worker; the
 * calling thread never sees the bytes, which is the same fact `apps/playground/src/tiles.ts` and
 * every `bytes` column in that tree already say out loud. So the 807-fetches-over-245-tiles figure
 * in Table 3 of `apps/playground/scripts/measure-frame.mjs` is real and is **not this module's to
 * bank** — it is DuckDB's, and that app's `src/duckdb.ts, registerUrl` passes `directIO = false`
 * precisely so that it can.
 *
 * What this thread *does* hold is the answers: the `Frame`s the door has already returned, which are
 * bytes already in hand. That is enough for both halves of the separation:
 *
 *   - a rectangle already answered is **not re-asked** — {@link Residency.held}, which is sound
 *     because `Corpus.frame` documents itself as «a function of its arguments and nothing else»;
 *   - a rectangle *not* yet answered gets an interim picture out of what is held —
 *     {@link Residency.visible} — which is the visibility half: a rectangle, a stride and a
 *     predicate, over points nobody has to fetch.
 *
 * ## The interim is superseded, never merged
 *
 * {@link Residency.visible} is a different set from the settled answer by construction — it is
 * assembled out of whatever earlier rectangles happened to hold — so it can only ever be the frame
 * that is on screen *while* the settled one is in flight. `apps/playground/src/tiles.ts` pushes it
 * through `BoundedSource.watch` and returns the door's answer unchanged from `slice`, so the
 * measured property `apps/playground/scripts/verify-properties.mjs` calls **path independence** is
 * untouched: the same rectangle by two routes still settles on the same set.
 *
 * ## The stride is a power of two, for the reason `apps/playground/src/stride.ts` argues
 *
 * An interim that holds more points than the renderer's budget has to be cut, and it is cut by
 * `dense_id % 2^k == 0` — the same family the door's `4^k` is a subset of. A vertex the interim
 * drew therefore stays drawn at every finer cut, which is what makes leaving the interim up
 * *correct* rather than merely better than a gap. An arbitrary `ceil(n / limit)` would make the
 * interim disagree with itself between two camera moves.
 */
import type { Box, Frame } from '@fossil-lang/corpus';

/**
 * The picture visibility assembles out of what residency holds.
 *
 * Deliberately the same field names and the same layout as a `Frame` — marks first, then anchors —
 * so a host's seam converts a held answer and an interim with one function instead of two.
 */
export interface Visible {
  /** `dense_id` per row, in the vertex type's own numbering. Marks first, then anchors. */
  readonly denseIds: BigUint64Array;
  /** `x, y` per row. */
  readonly positions: Float32Array;
  /** The `fill` column per row, as it was written. Only the first {@link Visible.marks} mean anything. */
  readonly categories: Uint32Array;
  /** How many rows are drawn. The rest are far ends holding an edge down. */
  readonly marks: number;
  /** Pairs of row indices into {@link Visible.positions}. */
  readonly links: Uint32Array;
  /**
   * Held marks inside the rectangle **before** the stride cut them — the interim's denominator, and
   * the field `Slice.n` is read out of.
   *
   * The stride it was cut to and how many held answers contributed are NOT here. Both are true and
   * neither has a reader, and a field nothing reads is a field that goes stale where nobody looks.
   */
  readonly matched: number;
}

/** What is loaded — a set of answered frames, an eviction policy, and no camera. */
export interface Residency {
  /** How many answers are held. */
  readonly frames: number;
  /** How many rows those answers hold, which is what the budget is spent in. */
  readonly points: number;
  /**
   * The answer filed under `key`, if it is still held — and **the same object**, so a caller that
   * re-derives its own vocabulary from it gets a byte-identical result rather than a similar one.
   */
  held(key: string): Frame | undefined;
  /** File one answered frame. Evicts least-recently-used answers until the budget is met again. */
  hold(key: string, answer: Frame): void;
  /**
   * **Visibility.** The held marks that fall inside `box`, cut to at most `limit` of them by a
   * power-of-two stride, with the far ends of their edges carried as anchors.
   *
   * `null` when nothing held is inside the rectangle — which is the honest answer for a camera that
   * has moved somewhere nothing has been read, and the one that leaves whatever is on screen alone.
   */
  visible(box: Box, limit: number): Visible | null;
  /** Let go of everything. `BoundedSource.watch` returns this; it is the release the contract names. */
  release(): void;
}

/**
 * The budget, in rows held.
 *
 * Measured on the bench corpus rather than picked. The whole-extent frame is 75,761 rows, and the
 * ten rectangles of `apps/playground/scripts/measure-frame.mjs` leave **243,240** rows held at the
 * end of the path —
 * so half a million is a ceiling that path does not reach, and it costs roughly 10 MB: 8 bytes of
 * `dense_id`, 8 of position and 4 of category per row. A session that never leaves one corner holds
 * one frame.
 *
 * **It is also the bound on what {@link Residency.visible} costs**, which is the number that picks
 * it rather than the memory. The scan is linear in the rows and the links held; driven over that
 * same path it ran **7.9 ms to 17.6 ms**, growing with what had accumulated, against a settled
 * answer of 56–244 ms for the same rectangle. At the ceiling it would be near double that — still
 * inside the interval it exists to fill, and the reason to move this number is a camera that
 * evicts rather than a budget with room to spare.
 */
const POINTS = 500_000;

/**
 * How many times two divides a `dense_id`, which is the finest power-of-two stride it survives.
 *
 * Over `Number` rather than `bigint`: a `dense_id` indexes rows of one vertex type and is nowhere
 * near 2⁵³, and the `bigint` operators cost enough per row to show on a scan of half a million.
 * Capped, because zero is divisible by every power of two and a loop that believes it does not stop.
 */
const CAP = 24;
function twos(dense: number): number {
  if (dense === 0) return CAP;
  let value = dense;
  let k = 0;
  while (k < CAP && value % 2 === 0) {
    value /= 2;
    k += 1;
  }
  return k;
}

/** The smallest `k` whose stride leaves at most `limit` of the counted marks. */
function cutFor(byTwos: Int32Array, total: number, limit: number): number {
  let carried = total;
  for (let k = 0; k < CAP; k += 1) {
    if (carried <= limit) return k;
    carried -= byTwos[k]!;
  }
  return CAP;
}

/**
 * A residency over answered frames, with a least-recently-used eviction policy.
 *
 * A `Map` is the whole structure: it iterates in insertion order, so re-inserting on every read puts
 * the least recently used at the front and eviction is a walk from there. There is no second
 * implementation and no policy interface to pick between — one policy, one caller, and the day a
 * second one is wanted is the day the choice is worth naming.
 */
export function residency({ points = POINTS }: { points?: number } = {}): Residency {
  const store = new Map<string, Frame>();
  let rows = 0;

  const sizeOf = (answer: Frame): number => answer.denseIds.length;

  const drop = (key: string): void => {
    const going = store.get(key);
    if (going === undefined) return;
    store.delete(key);
    rows -= sizeOf(going);
  };

  return {
    get frames() {
      return store.size;
    },
    get points() {
      return rows;
    },

    held(key) {
      const answer = store.get(key);
      if (answer === undefined) return undefined;
      // Re-inserted, which is the whole of the recency order: a `Map` iterates oldest first.
      store.delete(key);
      store.set(key, answer);
      return answer;
    },

    hold(key, answer) {
      drop(key);
      store.set(key, answer);
      rows += sizeOf(answer);
      // Never the entry just held, even when it alone exceeds the budget: a residency that evicts
      // the answer it was given holds nothing at all on a corpus whose frames are large.
      for (const oldest of store.keys()) {
        if (rows <= points || store.size <= 1) break;
        if (oldest !== key) drop(oldest);
      }
    },

    visible(box, limit) {
      if (store.size === 0 || limit <= 0) return null;
      const xhi = box.x + box.w;
      const yhi = box.y + box.h;
      // Half-open on the far edge, which is what `Corpus.frame`'s own box predicate is. An interim
      // that were closed there would draw a vertex the settled answer is about to drop.
      const inside = (x: number, y: number): boolean => x >= box.x && x < xhi && y >= box.y && y < yhi;

      // Pass one: how many held marks the rectangle holds, bucketed by the finest stride each
      // survives, so the cut can be chosen without a second scan per candidate `k`.
      const byTwos = new Int32Array(CAP + 1);
      const seen = new Set<number>();
      let matched = 0;
      for (const answer of store.values()) {
        for (let i = 0; i < answer.marks; i += 1) {
          if (!inside(answer.positions[i * 2]!, answer.positions[i * 2 + 1]!)) continue;
          const dense = Number(answer.denseIds[i]!);
          if (seen.has(dense)) continue;
          seen.add(dense);
          matched += 1;
          // `twos` is capped at CAP and the array is CAP + 1 long, so the index is always in
          // bounds — spelled with the same `!` `cutFor` reads the array through, because this
          // package compiles under `noUncheckedIndexedAccess` and the app it moved from did not.
          const bucket = twos(dense);
          byTwos[bucket] = byTwos[bucket]! + 1;
        }
      }
      if (matched === 0) return null;
      const cut = cutFor(byTwos, matched, limit);
      const stride = 2 ** cut;

      // Pass two: the marks that survive the cut, deduplicated across the answers that hold them.
      const markAt = new Map<number, number>();
      const marks: number[] = [];
      const markCats: number[] = [];
      const markIds: number[] = [];
      for (const answer of store.values()) {
        for (let i = 0; i < answer.marks; i += 1) {
          const x = answer.positions[i * 2]!;
          const y = answer.positions[i * 2 + 1]!;
          if (!inside(x, y)) continue;
          const dense = Number(answer.denseIds[i]!);
          if (dense % stride !== 0 || markAt.has(dense)) continue;
          markAt.set(dense, markIds.length);
          markIds.push(dense);
          marks.push(x, y);
          markCats.push(answer.categories[i]!);
        }
      }
      if (markIds.length === 0) return null;

      // Pass three: the edges among them. A link is carried when at least one end is a surviving
      // mark; the other end rides along as an ANCHOR wherever it is, which is the same bargain the
      // door strikes and for the same measured reason — a stub clipped to the rectangle carries the
      // right direction and lies about the distance.
      const anchorAt = new Map<number, number>();
      const anchors: number[] = [];
      const anchorIds: number[] = [];
      const pairs: number[] = [];
      /** Where a held row lands in the interim — its mark index, or an anchor index appended past them. */
      const place = (answer: Frame, row: number): number => {
        const dense = Number(answer.denseIds[row]!);
        const mark = markAt.get(dense);
        if (mark !== undefined) return mark;
        const already = anchorAt.get(dense);
        if (already !== undefined) return markIds.length + already;
        const at = anchorIds.length;
        anchorAt.set(dense, at);
        anchorIds.push(dense);
        anchors.push(answer.positions[row * 2]!, answer.positions[row * 2 + 1]!);
        return markIds.length + at;
      };
      for (const answer of store.values()) {
        for (let i = 0; i + 1 < answer.links.length; i += 2) {
          const a = answer.links[i]!;
          const b = answer.links[i + 1]!;
          const aDense = Number(answer.denseIds[a]!);
          const bDense = Number(answer.denseIds[b]!);
          if (!markAt.has(aDense) && !markAt.has(bDense)) continue;
          pairs.push(place(answer, a), place(answer, b));
        }
      }

      const rowsOut = markIds.length + anchorIds.length;
      const denseIds = new BigUint64Array(rowsOut);
      const positions = new Float32Array(rowsOut * 2);
      const categories = new Uint32Array(rowsOut);
      for (let i = 0; i < markIds.length; i += 1) {
        denseIds[i] = BigInt(markIds[i]!);
        positions[i * 2] = marks[i * 2]!;
        positions[i * 2 + 1] = marks[i * 2 + 1]!;
        categories[i] = markCats[i]!;
      }
      for (let i = 0; i < anchorIds.length; i += 1) {
        const at = markIds.length + i;
        denseIds[at] = BigInt(anchorIds[i]!);
        positions[at * 2] = anchors[i * 2]!;
        positions[at * 2 + 1] = anchors[i * 2 + 1]!;
      }

      return { denseIds, positions, categories, marks: markIds.length, links: Uint32Array.from(pairs), matched };
    },

    release() {
      store.clear();
      rows = 0;
    },
  };
}
