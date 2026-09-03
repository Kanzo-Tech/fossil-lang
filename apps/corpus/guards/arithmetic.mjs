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

/** The payload file of a row-group container: one per payload set, its row groups the tiles. */
export const TILES_FILE = "tiles.parquet";

/**
 * Where tile `k` of a payload set is, in whichever container the corpus declares.
 *
 * The manifest's `container` is what says which — `files`, one Parquet per tile with the address in
 * the name, or `rowgroups`, one Parquet per set whose row groups are the tiles. A reader over HTTP
 * has no directory to list, so this is the one thing about a corpus that cannot be worked out.
 *
 * `stem` is the manifest's own spelling: `chunk` for a vertex payload, `tile` for an identity index
 * and for an adjacency orientation.
 *
 * **The tile is a `BigInt` and it stays one.** A tile number above 2^53 put through a `Number`
 * composes a different filename — 2⁵³+1 prints as 2⁵³ — and the file it names is not there.
 * `vectors.json` publishes that border.
 *
 * @param {string} prefix dataset-relative, with a trailing separator
 * @param {"chunk" | "tile"} stem
 * @param {"files" | "rowgroups"} container
 * @param {bigint | number | string} tile
 * @returns {string}
 */
export function tileUrl(prefix, stem, container, tile) {
  if (container === "rowgroups") return `${prefix}${TILES_FILE}`;
  return `${prefix}${stem}${BigInt(tile)}.parquet`;
}

/**
 * How many tiles a declared row count occupies — the arithmetic the count exists for.
 *
 * A manifest declares `vertex_count` (or `edge_count`) and `chunk_size`, and between them a reader
 * knows **how far the corpus goes** before it opens a file. Nothing else says: tiles are addressed
 * and never listed, so without the count a tree holding `chunk0..chunk16` is indistinguishable from
 * a corpus that has seventeen tiles. A hole in the middle breaks the addressing and is caught; a
 * missing tail breaks nothing at all.
 *
 * `BigInt` for the same reason {@link tileOf} is: a count can exceed 2^53, and above that a
 * `Number` division silently loses the tail tile — which is precisely the tile this is here to
 * find. `vectors.json` publishes that border.
 *
 * @param {bigint} count rows in the logical table
 * @param {bigint} [chunkSize] rows per tile, from the manifest; defaults to {@link TILE_ROWS}
 * @returns {bigint | null} the tile count, or `null` if no shift addresses `chunkSize`
 */
export function tilesOf(count, chunkSize = TILE_ROWS) {
  if (typeof count !== "bigint") {
    throw new TypeError(
      `tilesOf takes a BigInt count; got ${typeof count}. Above 2^53 a Number loses the tail tile.`,
    );
  }
  if (count < 0n) throw new RangeError(`a row count is unsigned; got ${count}`);
  const shift = shiftFor(chunkSize);
  if (shift === null) return null;
  return (count + chunkSize - 1n) >> shift;
}

/**
 * How many rows the last tile holds — `chunk_size` for a count that divides, the remainder
 * otherwise, and `0` for an empty type, which has no last tile because it has none at all.
 *
 * The tail is where the off-by-one lives. A count that exactly fills a tile is one tile and not two,
 * and an emitter that writes the empty second one has written a file a reader pays a request for and
 * learns nothing from.
 *
 * @param {bigint} count
 * @param {bigint} [chunkSize]
 * @returns {bigint | null}
 */
export function tailRows(count, chunkSize = TILE_ROWS) {
  const tiles = tilesOf(count, chunkSize);
  if (tiles === null) return null;
  return tiles === 0n ? 0n : count - (tiles - 1n) * chunkSize;
}

/**
 * How many levels a written pyramid carries: **three**.
 *
 * Each level is one 2× zoom step and the coarsest fits in a single tile, so three of them cover a
 * 4× zoom range from there. Below the finest, a camera strides the payload — which nests, because
 * the stride is quantised to a power of two — so the pyramid does not have to reach level 0.
 */
export const LEVEL_WINDOW = 3n;

/**
 * The floor, in TILES: below this a type gets no pyramid.
 *
 * Expressed in tiles because the pyramid's cost is `2^LEVEL_WINDOW − 1` tiles' worth of rows
 * whatever the corpus is, so a floor in tiles is a floor as a fraction of the corpus: at 64 tiles
 * the pyramid is under 11% of the type and at 245 it is 2.7%, falling as the corpus grows. The
 * asymmetry is the argument — a floor set too high costs a corpus nothing but a slower zoom-out,
 * because the predicate over the payload draws the identical picture; a floor set too low charges
 * every small corpus bytes for a view it can already serve in a handful of range requests.
 */
export const LEVEL_FLOOR_TILES = 64n;

/**
 * How many rows level `k` of a type of `count` rows holds — `ceil(count / 2^k)`.
 *
 * @param {bigint} count
 * @param {bigint} level
 * @returns {bigint}
 */
export function levelRows(count, level) {
  const step = 1n << level;
  return (count + step - 1n) / step;
}

/**
 * **Which levels a writer writes**, or `null` where it writes none.
 *
 * Level `k` is the vertices whose `dense_id` is a multiple of `2^k`, which over a Morton-ordered
 * `dense_id` is one vertex per quadtree cell of depth `k`. This says which of those levels are
 * spent bytes on: the coarsest is the finest `k` whose level fits in ONE tile — coarser buys
 * nothing, because one tile is already one range request and the whole level is the minimum read —
 * and {@link LEVEL_WINDOW} levels run finer from there.
 *
 * **This is the second implementation of `fossil_sinks::manifest::VertexLevels::planned`**, in the
 * sense the rest of this directory is: written from the published convention rather than from
 * fossil's source, and pinned to it by the `level_plan` section of `vectors.json`, which both
 * halves execute. The plan is not something a READER derives — which levels exist is declared in
 * the manifest, because it is a policy — but it is something a WRITER has to decide, and there are
 * two writers.
 *
 * Computed by shifting rather than by a logarithm, because the answer has to be the same integer in
 * every language that re-implements it.
 *
 * @param {bigint} count rows in the vertex type
 * @param {bigint} [chunkSize] rows per tile, from the manifest
 * @returns {{ levels: number[], chunkSize: number } | null}
 */
export function levelPlan(count, chunkSize = TILE_ROWS) {
  if (typeof count !== "bigint" || typeof chunkSize !== "bigint") {
    throw new TypeError("levelPlan takes BigInt count and chunk_size, for tilesOf's reason");
  }
  if (chunkSize <= 0n || count <= chunkSize * LEVEL_FLOOR_TILES) return null;
  let coarsest = 1n;
  while (levelRows(count, coarsest) > chunkSize) coarsest += 1n;
  const finest = coarsest > LEVEL_WINDOW - 1n ? coarsest - (LEVEL_WINDOW - 1n) : 1n;
  const levels = [];
  for (let k = finest; k <= coarsest; k += 1n) levels.push(Number(k));
  return { levels, chunkSize: Number(chunkSize) };
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
