/**
 * How a window is sampled down to what a renderer will draw, as one SQL scalar.
 *
 * Its own module, and it imports NOTHING, so the one script that drives it —
 * `scripts/verify-nesting.mjs` — can read it in Node without resolving a browser bundle.
 *
 * **The app does not build this string any more, and that is not a reason to delete it.**
 * `src/tiles.ts` is a consumer of `corpus.levelFor` + `corpus.view` now, and the decimation is
 * `dense_id % 2^level = 0` inside the door. The property the expression below exists to state is
 * the same one the door's level IS — a power-of-two stride, so refining only ever adds — and
 * `verify-nesting.mjs` is what holds the claim to a measurement over a real corpus. What it no
 * longer is, is the app's own SQL; what it still is, is the argument, with its numbers.
 */

/**
 * The stride that samples a window, over a bound `matched`.
 *
 * `matched` is `count(*) OVER ()` in the caller's query: what the window HOLDS, not what came
 * back. One vertex in `stride` survives, so the sample lands near `limit` marks.
 *
 * **It is quantised UP to a power of two, and that is the whole of why a zoom refines instead
 * of flashing.** `ceil(matched / limit)` is an arbitrary integer and arbitrary integers do not
 * nest — the multiples of 37 and of 21 meet only at 777 — so two adjacent zoom levels share
 * almost no vertices and the camera move REPLACES the picture. Measured over a million vertices
 * at a 20,000 cap, counting only the vertices still inside the tighter window so that every loss
 * is resampling rather than leaving the frame, the arbitrary stride carried 4.8% / 33.3% / 25.0%
 * of the picture across the first three zoom steps. A power of two carries 100% across every
 * step, because `dense_id % 2^(k+1) = 0` is a strict subset of `dense_id % 2^k = 0`: a vertex
 * drawn once stays drawn, in the same place, at every closer zoom.
 *
 * It is a level of detail and not a budget adjustment. `dense_id` is Morton-ordered by the
 * layout pass, so `dense_id % 2^k = 0` is one vertex per quadtree cell of depth k — the stride
 * names a level of the same curve the tile address is read off. The price is at most one extra
 * halving of the sample. The argument and the table are in `/docs/design/corpus`.
 *
 * `- 0.5` keeps `log2` off the exact powers of two, where a double can land on either side of an
 * integer and buy a doubling nobody asked for: at `matched / limit` exactly 4, `ceil(log2(4))`
 * is 2 or 3 depending on the last bit, and the second costs half the marks.
 *
 * A pin is NOT exempt by virtue of this expression and never was — the caller OR-s its pinned
 * ids alongside the modulo, because an odd pin satisfies no stride above 1. Quantising does not
 * change that and does not fix it; removing the `OR` would reintroduce a defect this file's
 * caller has already had once.
 */
export function strideSql(limit: number): string {
  const raw = `greatest(1, CAST(ceil(matched / ${limit}.0) AS BIGINT))`;
  return `(1::BIGINT << CAST(floor(log2((${raw}) - 0.5)) + 1 AS BIGINT))`;
}
