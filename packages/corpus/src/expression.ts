/**
 * The predicates a read is filtered by, rendered as SQL — the rectangle today, and the seat of
 * Iceberg's expressions when a filter is more than a box.
 */

/**
 * A rectangle in the corpus's own coordinates. `w` and `h` extend from `x`/`y`.
 *
 * **Half-open on the far edge** — `x <= v.x < x + w`, and the same in `y`. It matters at exactly
 * one rectangle and it is the one a caller reaches for first: a box clamped to
 * {@link Corpus.extent} excludes the vertices sitting *on* the maximum, which is 17 of a million on
 * the bench corpus. A caller framing the whole extent nudges the far edge past it.
 */
export interface Box {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

export const boxOf = ({ x, y, w, h }: Box): string =>
  `x >= ${x} AND x < ${x + w} AND y >= ${y} AND y < ${y + h}`;
