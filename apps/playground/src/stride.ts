/**
 * How a window is sampled down to what a renderer will draw, as one SQL scalar.
 *
 * Its own module, and it imports NOTHING. `src/tiles.ts` reaches `@kanzo-tech/graph` and
 * `@duckdb/duckdb-wasm` through its import graph, so a Node script that wanted this expression
 * out of that file would have to resolve a browser bundle to read a string. The verification
 * this expression exists to satisfy — `scripts/verify-nesting.mjs` — runs in Node, so the claim
 * lives where both a browser and a script can pick it up without either paying for the other.
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
