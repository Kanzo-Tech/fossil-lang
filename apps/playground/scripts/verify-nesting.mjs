/**
 * Check that the canvas sample NESTS across a zoom step, against the bench corpus, in Node.
 *
 * The symptom this exists to catch is not a wrong answer — every stride returns vertices that
 * are really in the window, so nothing here is incorrect in the sense a schema or a count
 * would catch. The symptom is that the picture is REPLACED on every camera move: a reader
 * zooming in watches the cloud they were looking at disappear and a different one arrive.
 *
 * The cause is arithmetic. `s = ceil(matched / limit)` is an arbitrary integer, and arbitrary
 * integers share almost nothing: the multiples of 37 and the multiples of 21 meet only at 777.
 * So the property to assert is not a size or a rate, it is containment —
 *
 *     { drawn at the closer zoom } ⊇ { drawn at the farther zoom } ∩ { inside the closer window }
 *
 * — which holds for every power-of-two stride and fails for almost every other pair. Restricting
 * to the closer window is what makes the assertion about resampling rather than about the camera:
 * a vertex that left the frame is supposed to stop being drawn.
 *
 * It imports `strideSql` from `src/stride.ts` — the same function `src/tiles.ts` builds its
 * query from — rather than restating it, so an edit that un-quantises the stride turns this
 * red instead of leaving a test of a copy passing.
 *
 * Run: `node --experimental-strip-types scripts/verify-nesting.mjs [--vertices 1000000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { query } from '../../corpus/guards/duck.mjs';
import { strideSql } from '../src/stride.ts';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
};

const stampPath = resolve(app, 'public/bench/index.json');
if (!existsSync(stampPath)) {
  console.error('no bench corpus — run `node scripts/bench-corpus.mjs` first');
  process.exit(2);
}
const stamp = JSON.parse(readFileSync(stampPath, 'utf8'));
const count = Number(flag('vertices', stamp.count));
const root = resolve(app, 'public/bench', String(count));
const payload = join(root, 'vertex/Person/tiles.parquet');
if (!existsSync(payload)) {
  console.error(`no corpus at ${root} — run \`node scripts/bench-corpus.mjs --vertices ${count}\``);
  process.exit(2);
}

/** The renderer's own cap — `BOUNDED_DEFAULTS.limit` in `@kanzo-tech/graph`. */
const LIMIT = Number(flag('limit', 20000));

/** Concentric windows, each a strict subset of the one before it: five zoom steps. */
const FRACTIONS = [0.6, 0.4, 0.25, 0.15, 0.1, 0.06];

let failures = 0;
const ok = (label, condition, detail = '') => {
  if (condition) console.log(`  ok   ${label}${detail ? ` — ${detail}` : ''}`);
  else {
    console.log(`  FAIL ${label}${detail ? ` — ${detail}` : ''}`);
    failures++;
  }
};

const lit = (s) => `'${s.replace(/'/g, "''")}'`;
const stride = strideSql(LIMIT);

// The extent, and the centred rectangle at each fraction of it — the same geometry a camera
// zooming towards the middle of the corpus produces.
const [extent] = await query(
  `SELECT min(x) AS x0, max(x) AS x1, min(y) AS y0, max(y) AS y1 FROM read_parquet(${lit(payload)})`,
);
const cx = (Number(extent.x0) + Number(extent.x1)) / 2;
const cy = (Number(extent.y0) + Number(extent.y1)) / 2;
const w = Number(extent.x1) - Number(extent.x0);
const h = Number(extent.y1) - Number(extent.y0);
const rectAt = (f) => ({
  xlo: cx - (w * f) / 2,
  xhi: cx + (w * f) / 2,
  ylo: cy - (h * f) / 2,
  yhi: cy + (h * f) / 2,
});

/**
 * The drawn set for one window, expressed exactly as `corpusSource` expresses it: `matched` is
 * `count(*) OVER ()` over the rectangle, and the stride is the app's own scalar over it.
 */
const drawnSql = (r) => `
  WITH pool AS (
    SELECT dense_id, count(*) OVER () AS matched
    FROM read_parquet(${lit(payload)})
    WHERE x BETWEEN ${r.xlo} AND ${r.xhi} AND y BETWEEN ${r.ylo} AND ${r.yhi}
  )
  SELECT dense_id, matched, ${stride} AS s FROM pool WHERE dense_id % ${stride} = 0`;

console.log(`nesting of the canvas sample — ${count.toLocaleString()} vertices, cap ${LIMIT.toLocaleString()}\n`);

let previous = null;
for (const f of FRACTIONS) {
  const rect = rectAt(f);
  const rows = await query(drawnSql(rect));
  const ids = new Set(rows.map((r) => Number(r.dense_id)));
  const s = rows.length > 0 ? Number(rows[0].s) : 1;
  const matched = rows.length > 0 ? Number(rows[0].matched) : 0;

  ok(
    `stride at ${(f * 100).toFixed(0)}% is a power of two`,
    s > 0 && (s & (s - 1)) === 0,
    `matched ${matched.toLocaleString()}, s = ${s}, drawn ${ids.size.toLocaleString()}`,
  );

  if (previous) {
    // Of what the farther zoom drew, the part still inside THIS window — so everything lost
    // below is lost to resampling, not to leaving the frame.
    const carried = [...previous.ids].filter((id) => {
      const p = previous.pos.get(id);
      return p.x >= rect.xlo && p.x <= rect.xhi && p.y >= rect.ylo && p.y <= rect.yhi;
    });
    const kept = carried.filter((id) => ids.has(id));
    const pct = carried.length === 0 ? 100 : (kept.length / carried.length) * 100;
    ok(
      `${(previous.f * 100).toFixed(0)}% → ${(f * 100).toFixed(0)}% keeps every vertex still in frame`,
      kept.length === carried.length,
      `${kept.length.toLocaleString()} of ${carried.length.toLocaleString()} (${pct.toFixed(1)}%), s ${previous.s} → ${s}`,
    );
  }

  // Positions, so the next step can tell "left the frame" from "was resampled away".
  const pos = new Map();
  for (const row of await query(`
    SELECT dense_id, x, y FROM read_parquet(${lit(payload)})
    WHERE x BETWEEN ${rect.xlo} AND ${rect.xhi} AND y BETWEEN ${rect.ylo} AND ${rect.yhi}
      AND dense_id % ${s} = 0`)) {
    pos.set(Number(row.dense_id), { x: Number(row.x), y: Number(row.y) });
  }
  previous = { f, ids, pos, s };
}

console.log(failures === 0 ? '\nnesting holds at every step' : `\n${failures} failed`);
process.exit(failures === 0 ? 0 : 1);
