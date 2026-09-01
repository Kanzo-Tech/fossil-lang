/**
 * What the arithmetic costs against what the footers cost, over the same windows.
 *
 * `/docs/design/corpus` left one number unwritten on purpose: *"the cost of the arithmetic-only
 * path is measurable against the 1.13× over-read the footer path already records. It is not written
 * down until it is measured."* This file is the measurement.
 *
 * ## The two paths, and the third that is the point
 *
 * Both real ones answer *which tiles hold a vertex inside this rectangle*, and both over-cover,
 * because a tile is the unit that has an address:
 *
 *   - **Geometric.** Every tile whose Parquet-footer `x`/`y` box intersects the rectangle. Needs a
 *     Parquet reader and, at five million vertices, 1.15 MB of footer. It is what
 *     `apps/playground/src/stream.ts` does and what the design page's 1.13× describes.
 *   - **Arithmetic.** {@link mortonTilesFor} — descend the Z-order quadtree, stop where the answer
 *     stops changing. No reader, no engine, no promise; one number per tile end
 *     ({@link TileCodes}), which is 8 B per tile.
 *   - **Uniform.** The arithmetic with **no anchor at all**, tile `k` assumed to hold the `k`th
 *     equal slice of the code space — which is what *"arithmetic on `chunk_size` alone"* would have
 *     to mean. `dense_id` is a RANK and not a code, so that assumption is not conservative, and
 *     `missed` is the column that says so. It is measured rather than argued because the design
 *     page asked for a number and this is the one that decides the shape.
 *
 * The denominator is not a model. DuckDB is asked for the tiles that actually hold a vertex inside
 * the rectangle, and a path returning fewer has not over-read less — it has drawn a wrong picture.
 *
 * ## The corpus
 *
 * Written here by `apps/corpus/guards/fixture.mjs`, which is JavaScript against the published
 * conventions and imports nothing of ours — the same second implementation `cost.test.ts` uses, for
 * the same reason. Point `FOSSIL_ADDRESS_CORPUS` at a corpus directory (with
 * `FOSSIL_ADDRESS_COUNT`) to run the same windows over a bigger one; that is how the million-vertex
 * row on the design page was produced, against `apps/playground/scripts/bench-corpus.mjs`'s output.
 *
 * Needs the `duckdb` binary, and skips without one rather than failing — the guards next door make
 * the same trade for the same reason.
 */

import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import {
  type Box,
  type TileCodes,
  type TileCodesDocument,
  gridBoxOf,
  mortonOf,
  parseTileCodes,
  tilesForGrid,
} from '../src/address.js';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../../../apps/corpus/guards/fixture.mjs';
// @ts-expect-error — same reason: the guards reach a corpus through the `duckdb` binary and nothing
// of ours, so a measurement that uses them measures what a stranger would see.
import { query } from '../../../apps/corpus/guards/duck.mjs';

const hasDuckdb = spawnSync('duckdb', ['-c', 'select 1'], { encoding: 'utf8' }).status === 0;

/** The corpus's own `chunk_size`. 4,096 is what the writer emits and what the fixture defaults to. */
const CHUNK = 4096;
const SHIFT = 12;
/** Big enough that the tile count is in the tens and a 2% window is not the whole corpus. */
const VERTICES = 200_000;

const scratch: string[] = [];
afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

const lit = (value: string): string => value.replace(/'/g, "''");
const num = (v: unknown): number => Number(v);

/** One tile as the footer describes it: its bytes, and the box its geometry fills. */
interface TileBox extends Box {
  tile: number;
  start: number;
  bytes: number;
}

describe.skipIf(!hasDuckdb)('the arithmetic against the footers, same windows', () => {
  const fromEnv = process.env.FOSSIL_ADDRESS_CORPUS;
  let root: string;
  let count: number;
  if (fromEnv) {
    root = fromEnv;
    count = Number(process.env.FOSSIL_ADDRESS_COUNT ?? VERTICES);
  } else {
    root = mkdtempSync(join(tmpdir(), 'fossil-address-cost-'));
    scratch.push(root);
    write(root, { count: VERTICES, clusters: 128, layout: 'rowgroups', chunkSize: CHUNK });
    count = VERTICES;
  }
  const payload = join(root, 'vertex/Person/tiles.parquet');

  // ---- the geometric path's input ----
  const boxes: TileBox[] = query(`
    SELECT row_group_id AS tile,
      min(coalesce(dictionary_page_offset, data_page_offset)) AS start,
      sum(total_compressed_size) AS bytes,
      min(CASE WHEN path_in_schema = 'x' THEN CAST(stats_min_value AS DOUBLE) END) AS xlo,
      max(CASE WHEN path_in_schema = 'x' THEN CAST(stats_max_value AS DOUBLE) END) AS xhi,
      min(CASE WHEN path_in_schema = 'y' THEN CAST(stats_min_value AS DOUBLE) END) AS ylo,
      max(CASE WHEN path_in_schema = 'y' THEN CAST(stats_max_value AS DOUBLE) END) AS yhi
    FROM parquet_metadata('${lit(payload)}') GROUP BY row_group_id ORDER BY row_group_id
  `).map((r: Record<string, unknown>) => ({
    tile: num(r.tile),
    start: num(r.start),
    bytes: num(r.bytes),
    xlo: num(r.xlo),
    xhi: num(r.xhi),
    ylo: num(r.ylo),
    yhi: num(r.yhi),
  }));
  const tiles = boxes.length;

  /** The extent every position was quantised against — the type's own bounding box. */
  const extent: Box = boxes.reduce<Box>(
    (a, b) => ({
      xlo: Math.min(a.xlo, b.xlo),
      xhi: Math.max(a.xhi, b.xhi),
      ylo: Math.min(a.ylo, b.ylo),
      yhi: Math.max(a.yhi, b.yhi),
    }),
    { ...boxes[0]! },
  );

  // ---- the arithmetic path's input ----
  //
  // Two rows per tile, not two hundred: `dense_id` is the rank BY code, so the rows inside a tile
  // are in code order too and its first and last rows ARE its range.
  //
  // **This used to be the measurement's anchor and it is now its control.** It stood in for "a
  // manifest field that does not exist yet"; the field exists — `codes:` on the vertex manifest,
  // `vertex/<Type>/codes.json` on disk — and the corpus publishes one. So the numbers below are
  // measured against the PUBLISHED anchor and this derivation is what proves the published one is
  // the corpus's own: same codes, same extent, tile for tile. A measurement that reconstructs its
  // own input is measuring a function, not a format.
  const derived: TileCodes = { lo: new Uint32Array(tiles), hi: new Uint32Array(tiles) };
  const ends = query(`
    SELECT dense_id, CAST(x AS DOUBLE) AS x, CAST(y AS DOUBLE) AS y
    FROM read_parquet('${lit(payload)}')
    WHERE dense_id % ${CHUNK} = 0 OR dense_id % ${CHUNK} = ${CHUNK - 1} OR dense_id = ${count - 1}
    ORDER BY dense_id
  `) as Array<Record<string, unknown>>;
  const seenLo = new Set<number>();
  const seenHi = new Set<number>();
  for (const row of ends) {
    const dense = num(row.dense_id);
    const tile = dense >>> SHIFT;
    const code = mortonOf(Math.fround(num(row.x)), Math.fround(num(row.y)), extent);
    if (dense % CHUNK === 0) {
      (derived.lo as Uint32Array)[tile] = code;
      seenLo.add(tile);
    }
    if (dense % CHUNK === CHUNK - 1 || dense === count - 1) {
      (derived.hi as Uint32Array)[tile] = code;
      seenHi.add(tile);
    }
  }

  // ---- the published anchor: what the writer put on disk ----
  //
  // Read the way a stranger reads it — `JSON.parse` and nothing else, no Parquet reader in the
  // path — and it carries its own extent, so the arithmetic column below takes NOTHING from the
  // footer query above. That is the whole claim being measured: no footer, no reader, no engine.
  const anchorPath = join(root, 'vertex/Person/codes.json');
  const published: TileCodesDocument | null = existsSync(anchorPath)
    ? parseTileCodes(readFileSync(anchorPath, 'utf8'), anchorPath)
    : null;
  const codes: TileCodes = published ?? derived;

  /** No anchor at all: tile `k` assumed to hold the `k`th equal slice of the code space. */
  const uniform: TileCodes = { lo: new Uint32Array(tiles), hi: new Uint32Array(tiles) };
  for (let k = 0; k < tiles; k += 1) {
    (uniform.lo as Uint32Array)[k] = Math.floor((k * 2 ** 32) / tiles);
    (uniform.hi as Uint32Array)[k] = Math.floor(((k + 1) * 2 ** 32) / tiles) - 1;
  }

  /** Maximal runs of adjacent tiles whose bytes abut — one `Range` request each. */
  const requests = (selected: readonly number[]): number => {
    let runs = 0;
    let last = -2;
    let end = -1;
    for (const tile of selected) {
      const box = boxes[tile]!;
      if (tile === last + 1 && end === box.start) runs -= 1;
      runs += 1;
      last = tile;
      end = box.start + box.bytes;
    }
    return runs;
  };

  const windowIn = (fraction: number, cx: number, cy: number): Box => {
    const w = (extent.xhi - extent.xlo) * fraction;
    const h = (extent.yhi - extent.ylo) * fraction;
    const x = extent.xlo + (extent.xhi - extent.xlo) * cx;
    const y = extent.ylo + (extent.yhi - extent.ylo) * cy;
    return { xlo: x - w / 2, xhi: x + w / 2, ylo: y - h / 2, yhi: y + h / 2 };
  };

  interface Row {
    label: string;
    need: number;
    geo: number;
    geoReq: number;
    geoKb: number;
    ari: number;
    ariReq: number;
    ariKb: number;
    ariMissed: number;
    uni: number;
    uniMissed: number;
  }

  const rows: Row[] = [];
  for (const fraction of [0.02, 0.1, 0.3]) {
    for (const [cx, cy] of [
      [0.5, 0.5],
      [0.25, 0.25],
      [0.75, 0.6],
    ] as const) {
      const box = windowIn(fraction, cx, cy);
      const need = (
        query(`
        SELECT DISTINCT dense_id >> ${SHIFT} AS tile FROM read_parquet('${lit(payload)}')
        WHERE x BETWEEN ${box.xlo} AND ${box.xhi} AND y BETWEEN ${box.ylo} AND ${box.yhi}
      `) as Array<Record<string, unknown>>
      ).map((r) => num(r.tile));
      if (need.length === 0) continue;

      const geometric = boxes
        .filter((b) => b.xlo <= box.xhi && b.xhi >= box.xlo && b.ylo <= box.yhi && b.yhi >= box.ylo)
        .map((b) => b.tile);
      const grid = gridBoxOf(box, codes.extent ?? extent);
      const arithmetic = grid === null ? [] : tilesForGrid(grid, codes);
      const naive = grid === null ? [] : tilesForGrid(grid, uniform);

      const missed = (selected: readonly number[]): number => {
        const held = new Set(selected);
        return need.filter((t) => !held.has(t)).length;
      };
      const kb = (selected: readonly number[]): number =>
        selected.reduce((a, t) => a + boxes[t]!.bytes, 0) / 1024;

      rows.push({
        label: `${(fraction * 100).toFixed(0)}% @ ${cx},${cy}`,
        need: need.length,
        geo: geometric.length,
        geoReq: requests(geometric),
        geoKb: kb(geometric),
        ari: arithmetic.length,
        ariReq: requests(arithmetic),
        ariKb: kb(arithmetic),
        ariMissed: missed(arithmetic),
        uni: naive.length,
        uniMissed: missed(naive),
      });
    }
  }

  const mean = (of: (row: Row) => number): number =>
    rows.reduce((a, r) => a + of(r), 0) / rows.length;
  const total = (of: (row: Row) => number): number => rows.reduce((a, r) => a + of(r), 0);

  it('addresses every tile of the corpus', () => {
    expect(seenLo.size).toBe(tiles);
    expect(seenHi.size).toBe(tiles);
    expect(rows.length).toBeGreaterThan(0);
  });

  it('reads the anchor the corpus publishes, and it is the corpus\'s own', () => {
    // Not "an anchor parses". The published `lo`/`hi` must be the codes of the first and last row
    // of every tile, recomputed here from the rows themselves through a different route — a
    // DuckDB read of the two ends plus `mortonOf` — and the extent must be the one the footers
    // describe. A writer that published a plausible anchor computed from something else would
    // give a plausible picture of the wrong vertices, and no count would notice.
    expect(published).not.toBeNull();
    const anchor = published!;
    expect(anchor.mortonBits).toBe(16);
    expect(anchor.chunkSize).toBe(CHUNK);
    expect(anchor.tiles).toBe(tiles);
    expect(anchor.lo).toEqual([...(derived.lo as Uint32Array)]);
    expect(anchor.hi).toEqual([...(derived.hi as Uint32Array)]);
    for (const side of ['xlo', 'xhi', 'ylo', 'yhi'] as const) {
      expect(Math.fround(anchor.extent[side])).toBe(Math.fround(extent[side]));
    }
  });

  it('costs two u32 per tile, against a footer that costs a reader', () => {
    // The size the design page quotes, checked against the file rather than argued: the anchor is
    // JSON, so it is bigger than the 8 B per tile a packed array would be, and it is still two
    // orders of magnitude under the footer it replaces.
    const anchorBytes = statSync(anchorPath).size;
    const footerBytes = statSync(payload).size - boxes.reduce((a, b) => a + b.bytes, 0);
    expect(anchorBytes).toBeLessThan(footerBytes / 10);
  });

  it('misses nothing: every tile holding a matching vertex is addressed', () => {
    // The one that has to hold. A decomposition that comes back smaller has not saved bytes, it has
    // dropped vertices that are inside the window — and a picture is wrong in a way no count sees.
    expect(total((r) => r.ariMissed)).toBe(0);
  });

  it('over-reads no more than the footer path, window for window', () => {
    for (const row of rows) expect(row.ari).toBeLessThanOrEqual(row.geo);
  });

  it('needs the anchor: the ranks are not uniform in the code space', () => {
    // `dense_id` is a rank, so assuming it tracks the code space is a guess and not a bound. This
    // is the measured reason {@link TileCodes} is data and not arithmetic.
    expect(total((r) => r.uniMissed)).toBeGreaterThan(0);
  });

  it('reports the table', () => {
    const bytes = statSync(payload).size;
    const footerBytes = bytes - boxes.reduce((a, b) => a + b.bytes, 0);
    const lines = [
      `corpus   ${count} vertices · ${tiles} tiles · payload ${(bytes / 1024 / 1024).toFixed(1)} MB`,
      `inputs   footer ${(footerBytes / 1024).toFixed(0)} kB + a Parquet reader · ` +
        `anchor ${statSync(anchorPath).size} B published (${tiles * 8} B packed) + nothing`,
      '',
      'window            need |  geometric              |  arithmetic             | uniform, no anchor',
      '                       |  tiles     × req     kB |  tiles     × req     kB |  tiles     × missed',
      ...rows.map(
        (r) =>
          `${r.label.padEnd(16)} ${String(r.need).padStart(4)} | ` +
          `${String(r.geo).padStart(6)} ${(r.geo / r.need).toFixed(2)} ${String(r.geoReq).padStart(3)} ${r.geoKb.toFixed(0).padStart(6)} | ` +
          `${String(r.ari).padStart(6)} ${(r.ari / r.need).toFixed(2)} ${String(r.ariReq).padStart(3)} ${r.ariKb.toFixed(0).padStart(6)} | ` +
          `${String(r.uni).padStart(6)} ${(r.uni / r.need).toFixed(2)} ${String(r.uniMissed).padStart(6)}`,
      ),
      '',
      `mean over-read   geometric ${mean((r) => r.geo / r.need).toFixed(2)}× · ` +
        `arithmetic ${mean((r) => r.ari / r.need).toFixed(2)}× · uniform ${mean((r) => r.uni / r.need).toFixed(2)}×`,
      `mean bytes       geometric ${mean((r) => r.geoKb).toFixed(0)} kB in ${mean((r) => r.geoReq).toFixed(1)} requests · ` +
        `arithmetic ${mean((r) => r.ariKb).toFixed(0)} kB in ${mean((r) => r.ariReq).toFixed(1)}`,
      `tiles missed     arithmetic ${total((r) => r.ariMissed)} · uniform ${total((r) => r.uniMissed)} of ${total((r) => r.need)} needed`,
    ];
    console.log(`\n${lines.join('\n')}\n`);
    expect(lines.length).toBeGreaterThan(0);
  });
});
