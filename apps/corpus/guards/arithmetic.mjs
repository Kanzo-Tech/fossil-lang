/**
 * The arithmetic of a fossil corpus, as a second implementation.
 *
 * Every function here also exists in Rust, and that is the point: the corpus is read by code that
 * was not compiled against the code that wrote it, so the arithmetic a reader re-derives is where
 * the format goes wrong. Iceberg publishes its bucket transform as a formula plus a table of
 * vectors — `34 → 2017239379` — and searching for that constant finds it in the tests of
 * independent implementations that copied it. PMTiles dispatches its Hilbert curve with a link to
 * Wikipedia, and every port re-derives it differently.
 *
 * This file is written to be copied. It has no imports.
 *
 * **The operands, spelled out**, because the same three characters mean three different things
 * across the layers that have to agree:
 *
 *   - `dense_id` is an unsigned integer that may carry more than 53 bits, so it is a `BigInt` here
 *     and `>>` on a `BigInt` is a logical shift on an arbitrary-width value.
 *   - In Rust `>>` on a signed type is arithmetic; `TILE_SHIFT` is applied to a `u64`.
 *   - In JavaScript `>>` on a `Number` truncates to 32 bits *before* shifting, which is why
 *     `tileOf` refuses a `Number`. `node-s2` was a binding — not a port — and it returned wrong
 *     cell ids anyway, because a `Number` carries 53 bits and a cell id carries 64. A binding does
 *     not protect a language boundary that cannot hold the value.
 *
 * The Morton half is 32-bit by construction: two `u16` quantised coordinates interleave into a
 * `u32`, and every operation below is masked to stay inside it.
 */

/** How many bits a `dense_id` is shifted right by to name the tile holding it. */
export const TILE_SHIFT = 12n;

/** Rows per tile — `1 << TILE_SHIFT`. A tile *is* the range `[i·4096, (i+1)·4096)`. */
export const TILE_ROWS = 1n << TILE_SHIFT;

/**
 * The tile a `dense_id` lives in — the whole of the addressing scheme.
 *
 * There is no tile tree and nothing to discover: tile `i` is the range `[i·4096, (i+1)·4096)`, its
 * parent is a further shift, and the lowest common ancestor of two vertices is the common prefix of
 * their ids. A reader computes every URL it wants before it emits the first request.
 *
 * @param {bigint} denseId
 * @param {bigint} [shift] the corpus's own shift, from its manifest; defaults to {@link TILE_SHIFT}
 * @returns {bigint}
 */
export function tileOf(denseId, shift = TILE_SHIFT) {
  if (typeof denseId !== "bigint") {
    throw new TypeError(
      `tileOf takes a BigInt; got ${typeof denseId}. A dense_id carries more bits than a Number ` +
        `can hold, and JavaScript's >> truncates to 32 before it shifts.`,
    );
  }
  if (denseId < 0n) throw new RangeError(`dense_id is unsigned; got ${denseId}`);
  return denseId >> shift;
}

/**
 * The shift that addresses a tile of `rows` rows, or `null` if no shift does.
 *
 * A tile size that is not a power of two forces a division where a shift does, which is the reason
 * 122,880 — DuckDB's default row-group size, and the corpus's tile size before it was measured —
 * cannot be one.
 *
 * @param {bigint} rows
 * @returns {bigint | null}
 */
export function shiftFor(rows) {
  if (rows <= 0n || (rows & (rows - 1n)) !== 0n) return null;
  let shift = 0n;
  for (let r = rows; r > 1n; r >>= 1n) shift += 1n;
  return shift;
}

/** Spread the low 16 bits of `n` into the even bit positions of a `u32`. */
function spread(n) {
  let v = n & 0xffff;
  v = (v | (v << 8)) & 0x00ff00ff;
  v = (v | (v << 4)) & 0x0f0f0f0f;
  v = (v | (v << 2)) & 0x33333333;
  v = (v | (v << 1)) & 0x55555555;
  return v >>> 0;
}

/**
 * Interleave two quantised coordinates into a 32-bit Morton (Z-order) code — `x` in the even bits,
 * `y` in the odd ones.
 *
 * `spread(y) << 1` reaches bit 31, so the result is a signed negative `Number` unless it is coerced
 * back to unsigned. That coercion is not a detail: a port that omits it sorts the top half of the
 * plane before the bottom half and produces a corpus that satisfies every count-based check and
 * addresses nothing.
 *
 * @param {number} x 0..65535
 * @param {number} y 0..65535
 * @returns {number} 0..4294967295
 */
export function morton2(x, y) {
  return (spread(x) | (spread(y) << 1)) >>> 0;
}

/**
 * Quantise one coordinate onto `0..65535` over the extent it is being ranked within.
 *
 * The extent is the bounding box of **one vertex type's own positions**, because each type is laid
 * out and renumbered independently. A degenerate axis — every value equal — maps to 0 rather than
 * dividing by zero.
 *
 * **Every step is binary32**, which is why each one is wrapped in `Math.fround`. The writer is
 * `morton_codes` in `crates/fossil-layout/src/layout.rs`, whose positions, extent and intermediate
 * ratio are all `f32`; JavaScript's own arithmetic is binary64, so a literal transcription of the
 * formula is a *different function*. It differs on 8 of the 44,850 integer cases with `lo = 0` and
 * `hi ∈ 2..299` — `quantize(147, 0, 167)` is 57687 in binary32 and 57686 in binary64 — and one unit
 * here is a different Morton code, a different rank, a different `dense_id` and a different tile.
 * `vectors.json` carries that case; the four rows beside it are exact in both widths and cannot.
 *
 * The DuckDB half of the guard (`mortonSql` in `guards.mjs`) spells the same thing `::FLOAT`.
 *
 * @param {number} v
 * @param {number} lo
 * @param {number} hi
 * @returns {number}
 */
export function quantize(v, lo, hi) {
  if (hi <= lo) return 0;
  const f = Math.fround;
  const t = Math.min(Math.max(f(f(f(v) - f(lo)) / f(f(hi) - f(lo))), 0), 1);
  return Math.round(f(t * 65535));
}

/**
 * The Morton code of a position within an extent — quantise, then interleave.
 *
 * @param {number} x
 * @param {number} y
 * @param {{minX: number, maxX: number, minY: number, maxY: number}} extent
 * @returns {number}
 */
export function mortonOf(x, y, extent) {
  return morton2(quantize(x, extent.minX, extent.maxX), quantize(y, extent.minY, extent.maxY));
}
