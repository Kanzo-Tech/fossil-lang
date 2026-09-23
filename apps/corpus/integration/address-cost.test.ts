/**
 * What a rectangle costs through the footers, over the same windows.
 *
 * **The footers ARE the index** — `footer-is-the-index` — and this is the measurement of the one
 * path that survives that convention: every tile whose Parquet-footer `x`/`y` box intersects the
 * rectangle, which is what `intersecting` in `packages/corpus/src/corpus.ts` does with the boxes
 * `open` reads once. Both halves of the question are here: it must MISS nothing, and what it over-reads
 * for that is reported in tiles, in `Range` requests and in kilobytes.
 *
 * The denominator is not a model. DuckDB is asked for the tiles that actually hold a vertex inside
 * the rectangle, and a path returning fewer has not over-read less — it has drawn a wrong picture.
 *
 * **A second index over this question was measured and deleted.** A published `codes.json` anchor
 * answered it by descending the Z-order quadtree, and it bought 15 tiles in 4 requests and 1,294 kB
 * against these 17 in 6 and 1,466 — a 13% over-read, for a document, a request per corpus, a
 * parser, a guard, a code path and its fallback. `/docs/design/corpus` has the argument. Do not
 * re-measure it here; this file measures what is left.
 *
 * `cost.test.ts` asserts the same path as a SHAPE — never every tile, about one more than the
 * answer needed, a smaller share of a bigger corpus — through `open` and the host's own
 * `query`. This one reports the numbers, at the DuckDB-CLI level, over a corpus this repository's
 * own reader never touches.
 *
 * ## The corpus
 *
 * Written here by `apps/corpus/guards/fixture.mjs`, which is JavaScript against the published
 * conventions and imports nothing of ours — the same second implementation `cost.test.ts` uses, for
 * the same reason. Point `FOSSIL_ADDRESS_CORPUS` at a corpus directory (with
 * `FOSSIL_ADDRESS_COUNT`) to run the same windows over a bigger one.
 *
 * It needs the `duckdb` binary, which is why it lives in `apps/corpus/integration/` beside the
 * guards that speak to it and not in `packages/corpus/tests/`. The dependency is DECLARED —
 * `pnpm --filter @fossil-lang/corpus-contract test:integration` is the script that has it, and the
 * package's own `pnpm test` no longer does. It is not probed and this file does not skip:
 * `describe.skipIf` skips the TESTS and runs the describe BODY, so the probe it was guarding
 * never guarded the fixture write, and a suite that vanishes with its dependency reads as
 * covered while asserting nothing.
 */

import { mkdtempSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

// @ts-expect-error — the fixture is JavaScript on purpose: it is the second implementation the
// conventions ask for, and it must not import a type of ours to be one.
import { write } from '../guards/fixture.mjs';
// @ts-expect-error — same reason: the guards reach a corpus through the `duckdb` binary and nothing
// of ours, so a measurement that uses them measures what a stranger would see.
import { query } from '../guards/duck.mjs';
// @ts-expect-error — and the tile size this fixture is written at comes from there too. It used to
// come from `TILE_SHIFT` in `packages/corpus/src/address.ts`, which was a second implementation of
// the reader and is deleted; the guards' copy is the one a third party actually reads.
import { TILE_ROWS, TILE_SHIFT } from '../guards/arithmetic.mjs';

/** The corpus's own `chunk_size`, and the shift that addresses it. */
const SHIFT = Number(TILE_SHIFT);
const CHUNK = Number(TILE_ROWS);
/** Big enough that the tile count is in the tens and a 2% window is not the whole corpus. */
const VERTICES = 200_000;

const scratch: string[] = [];
afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

const lit = (value: string): string => value.replace(/'/g, "''");
const num = (v: unknown): number => Number(v);

/** A rectangle in the corpus's own coordinates, both ends inclusive. */
interface Rect {
  xlo: number;
  xhi: number;
  ylo: number;
  yhi: number;
}

/** One tile as the footer describes it: its bytes, and the box its geometry fills. */
interface TileBox extends Rect {
  tile: number;
  start: number;
  bytes: number;
}

describe('what the footers cost, window for window', () => {
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

  // The index, and the only one: one row per tile out of `parquet_metadata`, carrying the box that
  // decides whether a rectangle can touch it and the byte interval it costs to open.
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

  /** The type's own bounding box, which the windows below are cut out of. */
  const extent: Rect = boxes.reduce<Rect>(
    (a, b) => ({
      xlo: Math.min(a.xlo, b.xlo),
      xhi: Math.max(a.xhi, b.xhi),
      ylo: Math.min(a.ylo, b.ylo),
      yhi: Math.max(a.yhi, b.yhi),
    }),
    { ...boxes[0]! },
  );

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

  const windowIn = (fraction: number, cx: number, cy: number): Rect => {
    const w = (extent.xhi - extent.xlo) * fraction;
    const h = (extent.yhi - extent.ylo) * fraction;
    const x = extent.xlo + (extent.xhi - extent.xlo) * cx;
    const y = extent.ylo + (extent.yhi - extent.ylo) * cy;
    return { xlo: x - w / 2, xhi: x + w / 2, ylo: y - h / 2, yhi: y + h / 2 };
  };

  interface Row {
    label: string;
    need: number;
    got: number;
    req: number;
    kb: number;
    missed: number;
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

      const selected = boxes
        .filter((b) => b.xlo <= box.xhi && b.xhi >= box.xlo && b.ylo <= box.yhi && b.yhi >= box.ylo)
        .map((b) => b.tile);
      const held = new Set(selected);

      rows.push({
        label: `${(fraction * 100).toFixed(0)}% @ ${cx},${cy}`,
        need: need.length,
        got: selected.length,
        req: requests(selected),
        kb: selected.reduce((a, t) => a + boxes[t]!.bytes, 0) / 1024,
        missed: need.filter((t) => !held.has(t)).length,
      });
    }
  }

  const mean = (of: (row: Row) => number): number =>
    rows.reduce((a, r) => a + of(r), 0) / rows.length;
  const total = (of: (row: Row) => number): number => rows.reduce((a, r) => a + of(r), 0);

  it('addresses every tile of the corpus', () => {
    expect(tiles).toBe(Math.ceil(count / CHUNK));
    expect(rows.length).toBeGreaterThan(0);
  });

  it('misses nothing: every tile holding a matching vertex is addressed', () => {
    // The one that has to hold. A selection that comes back smaller has not saved bytes, it has
    // dropped vertices that are inside the window — and a picture is wrong in a way no count sees.
    expect(total((r) => r.missed)).toBe(0);
  });

  it('opens a proper subset of the corpus for a window smaller than it', () => {
    // The footers PRUNE. A path that named every tile would also miss nothing, and it is the
    // failure `cost.test.ts` was written for after a window did exactly that for as long as it
    // existed and answered correctly every time.
    for (const row of rows) expect(row.got).toBeLessThan(tiles);
  });

  it('costs no reader beyond the one that opens the payload', () => {
    // The footer is inside the file the rows are in, so the index costs no second artefact: this is
    // what the deleted anchor's 5.4 kB of JSON and its own request bought 13% off.
    const footerBytes = statSync(payload).size - boxes.reduce((a, b) => a + b.bytes, 0);
    expect(footerBytes).toBeGreaterThan(0);
  });

  it('reports the table', () => {
    const bytes = statSync(payload).size;
    const footerBytes = bytes - boxes.reduce((a, b) => a + b.bytes, 0);
    const lines = [
      `corpus   ${count} vertices · ${tiles} tiles · payload ${(bytes / 1024 / 1024).toFixed(1)} MB`,
      `index    footer ${(footerBytes / 1024).toFixed(0)} kB, read once per corpus, no second document`,
      '',
      'window            need |  tiles       ×     req       kB |  missed',
      ...rows.map(
        (r) =>
          `${r.label.padEnd(16)} ${String(r.need).padStart(4)} | ` +
          `${String(r.got).padStart(6)} ${(r.got / r.need).toFixed(2)} ${String(r.req).padStart(7)} ${r.kb.toFixed(0).padStart(8)} | ` +
          `${String(r.missed).padStart(7)}`,
      ),
      '',
      `mean over-read   ${mean((r) => r.got / r.need).toFixed(2)}×`,
      `mean bytes       ${mean((r) => r.kb).toFixed(0)} kB in ${mean((r) => r.req).toFixed(1)} requests`,
      `tiles missed     ${total((r) => r.missed)} of ${total((r) => r.need)} needed`,
    ];
    console.log(`\n${lines.join('\n')}\n`);
    expect(lines.length).toBeGreaterThan(0);
  });
});
