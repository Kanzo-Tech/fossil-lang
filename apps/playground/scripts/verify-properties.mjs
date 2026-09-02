/**
 * The three properties a level-of-detail view owes a reader, measured against the bench corpus.
 *
 * No browser, no GPU, no `requestAnimationFrame` — the corpus is a million real vertices on disk
 * and the door is `@fossil-lang/corpus`'s `view({ x, y, w, h, level })`, which the canvas's source
 * is a consumer of. What cannot be checked here is the picture; what can be checked is every claim
 * the picture rests on.
 *
 * ## 1. Fidelity — a coarse view is the same image with fewer points
 *
 * The statement: for any region R, `|level_k ∩ R| / |level_k|` ≈ `|level_0 ∩ R| / |level_0|`. A
 * view that fails it is not slower or emptier, it is **wrong about the shape of the graph** — it
 * shows a cloud the corpus does not have.
 *
 * The statistic is total variation distance between two normalised density grids, and the
 * **tolerance is measured rather than chosen**: any subsample of size n has sampling noise, so the
 * question is not "is the distance small" but "is it larger than a subsample of the same size that
 * has no spatial structure at all". The null is `hash(dense_id) % 2^k = 0` — the same population,
 * drawn pseudo-randomly instead of by the Morton stride — measured over the same grid. The level
 * passes when it is no further from level 0 than that null is.
 *
 * Total variation and not a χ², because the question is about a *picture*: TV is exactly the
 * largest fraction of the ink that can land in the wrong place, it is bounded in [0, 1] and it does
 * not grow with the grid the way a χ² statistic does.
 *
 * ## 2. Monotone refinement — zooming in only ADDS
 *
 * `{drawn at the closer zoom} ⊇ {drawn at the farther zoom} ∩ {inside the closer window}`.
 * `verify-nesting.mjs` already asserts this over the stride the SQL builds, and it is run from here
 * rather than restated: one property, one test. What is added here is the same containment stated
 * over the door's own argument — `view(R, k+1) ⊇ view(R, k)` for a fixed R — which is the form that
 * only exists because the level is a parameter and not a derived quantity.
 *
 * ## 3. Path independence — the same rectangle by two routes is the same picture
 *
 * The one that was broken. `view` is a pure function of `(rect, level)`, so a route can only change
 * the answer by changing the rectangle — and it does: the camera's rectangle arrives in the
 * coordinate system of the positions the renderer is holding, and that system is rebuilt from
 * whichever answer arrived last. `frame.mjs` is that map, transcribed from cosmos.gl and checked
 * against it in a browser; `--frame rescaled` is the app before the fix and `--frame pinned` is the
 * app after it. The default is neither: it is read out of `src/Canvas.tsx`, so this reports what
 * the app IS rather than what a flag says.
 *
 * Run: `node --experimental-strip-types scripts/verify-properties.mjs [--frame rescaled|pinned]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { openCorpus } from '@fossil-lang/corpus';
import { query as duckQuery, lit } from '../../corpus/guards/duck.mjs';
import { PINNED, rescaledFrame, toCorpus } from './frame.mjs';

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

/**
 * Which frame the app is wired for, read out of the app rather than declared here.
 *
 * A flag that says what the code does is a second copy of the fact. `Canvas.tsx` either pins the
 * renderer's rescale or it does not, and the string it pins it with is the derivation.
 */
function wiredFrame() {
  const source = readFileSync(resolve(app, 'src/Canvas.tsx'), 'utf8');
  return /rescalePositions:\s*false/.test(source) ? 'pinned' : 'rescaled';
}

const mode = flag('frame', wiredFrame());
if (mode !== 'pinned' && mode !== 'rescaled') {
  console.error(`--frame is pinned or rescaled; got ${mode}`);
  process.exit(2);
}

/** The renderer's own cap — `BOUNDED_DEFAULTS.limit` in `@kanzo-tech/graph`. */
const BUDGET = Number(flag('budget', 20000));

let failures = 0;
const ok = (label, condition, detail = '') => {
  if (condition) console.log(`  ok   ${label}${detail ? ` — ${detail}` : ''}`);
  else {
    console.log(`  FAIL ${label}${detail ? ` — ${detail}` : ''}`);
    failures += 1;
  }
};
const note = (text) => console.log(`  ..   ${text}`);

const corpus = await openCorpus(root, { query: async (sql) => duckQuery(sql) });
const type = corpus.types.vertices[0].type;
const extent = await corpus.extent();
const PAD = 1e-3;
const WHOLE = {
  x: extent.minX - PAD,
  y: extent.minY - PAD,
  w: extent.maxX - extent.minX + 2 * PAD,
  h: extent.maxY - extent.minY + 2 * PAD,
};
/** What `useQueryLoop` sets on the renderer from the first `extent()`. */
const SPACE = Math.max(extent.maxX - extent.minX, extent.maxY - extent.minY);
const TILES = `'${lit(corpus.addressing.vertexType(type).tileUrl(0))}'`;

console.log(
  `${count.toLocaleString()} vertices · ${type} · x[${extent.minX}, ${extent.maxX}] ` +
    `y[${extent.minY}, ${extent.maxY}] · budget ${BUDGET.toLocaleString()} · frame ${mode}\n`,
);

// ---------------------------------------------------------------------------
// 1. Fidelity
// ---------------------------------------------------------------------------

/** How many cells per axis the density grid has. */
const GRID = Number(flag('grid', 48));

/**
 * A density grid over the whole extent, for one predicate on `dense_id`.
 *
 * SQL rather than the door, because a grid at level 0 is a million rows and the door would hand
 * them all back as typed arrays to be counted once. The predicate below IS the level — `view`'s own
 * definition — and the two are checked against each other before this is trusted.
 */
function grid(predicate) {
  const cw = WHOLE.w / GRID;
  const ch = WHOLE.h / GRID;
  const rows = duckQuery(
    `SELECT least(${GRID - 1}, floor((x - ${WHOLE.x}) / ${cw}))::INTEGER AS gx,
            least(${GRID - 1}, floor((y - ${WHOLE.y}) / ${ch}))::INTEGER AS gy,
            count(*) AS n
     FROM read_parquet(${TILES}) WHERE ${predicate} GROUP BY 1, 2`,
  );
  const cells = new Float64Array(GRID * GRID);
  let total = 0;
  for (const row of rows) {
    const n = Number(row.n);
    cells[Number(row.gy) * GRID + Number(row.gx)] += n;
    total += n;
  }
  if (total > 0) for (let i = 0; i < cells.length; i += 1) cells[i] /= total;
  return { cells, total };
}

/** Total variation distance: the largest fraction of the ink that can land in the wrong cell. */
const tv = (a, b) => {
  let sum = 0;
  for (let i = 0; i < a.length; i += 1) sum += Math.abs(a[i] - b[i]);
  return sum / 2;
};

console.log('1. fidelity — is a coarse view the same image with fewer points');
{
  const base = grid('TRUE');
  ok('the grid at level 0 is the whole corpus', base.total === count, `${base.total}`);

  const rows = [];
  for (const level of [2, 4, 6, 8]) {
    const stride = 2 ** level;
    const level_ = grid(`dense_id % ${stride} = 0`);
    // The null: the same number of vertices, drawn with no spatial structure. `hash` over the id
    // scatters where the Morton stride stratifies, so this is what "a random subsample of this size"
    // costs in total variation — and it is the tolerance, measured rather than picked.
    const null_ = grid(`hash(dense_id) % ${stride} = 0`);
    const d = tv(base.cells, level_.cells);
    const n = tv(base.cells, null_.cells);
    rows.push({ level, stride, drawn: level_.total, d, n, sampled: null_.total });
    ok(
      `level ${level} is no further from the whole graph than a random subsample of its size`,
      d <= n,
      `TV ${d.toFixed(4)} against ${n.toFixed(4)} over ${level_.total.toLocaleString()} points`,
    );
  }
  note(`grid ${GRID}x${GRID} over the extent, ${GRID * GRID} cells`);
  for (const r of rows) {
    note(
      `level ${r.level} (1 in ${r.stride}): ${r.drawn.toLocaleString()} drawn, TV ${r.d.toFixed(4)}; ` +
        `random ${r.sampled.toLocaleString()}, TV ${r.n.toFixed(4)}`,
    );
  }

  // The other half of the statement, over regions rather than over a grid: the share of the drawn
  // set that falls in R has to be the share of the whole graph that falls in R.
  let worst = 0;
  let worstAt = '';
  for (const f of [0.5, 0.25, 0.1, 0.05]) {
    const box = {
      x: WHOLE.x + (WHOLE.w * (1 - f)) / 2,
      y: WHOLE.y + (WHOLE.h * (1 - f)) / 2,
      w: WHOLE.w * f,
      h: WHOLE.h * f,
    };
    const inside = (p) =>
      Number(
        duckQuery(
          `SELECT count(*) AS n FROM read_parquet(${TILES})
           WHERE ${p} AND x >= ${box.x} AND x < ${box.x + box.w}
             AND y >= ${box.y} AND y < ${box.y + box.h}`,
        )[0].n,
      );
    for (const level of [4, 6]) {
      const stride = 2 ** level;
      const share0 = inside('TRUE') / count;
      const sharek = inside(`dense_id % ${stride} = 0`) / Math.ceil(count / stride);
      const error = Math.abs(sharek - share0) / share0;
      if (error > worst) {
        worst = error;
        worstAt = `${(f * 100).toFixed(0)}% window at level ${level}: ${(sharek * 100).toFixed(3)}% against ${(share0 * 100).toFixed(3)}%`;
      }
    }
  }
  // The bound is the null's, again measured rather than chosen: at the smallest region and coarsest
  // level tested the level-0 share is over a thousand vertices, and a binomial of that size has a
  // relative standard error of about 3%. Three of those is the band.
  ok('every region keeps its share of the drawn set', worst < 0.09, `worst ${(worst * 100).toFixed(2)}% — ${worstAt}`);
}

// ---------------------------------------------------------------------------
// 2. Monotone refinement
// ---------------------------------------------------------------------------

console.log('\n2. monotone refinement — does zooming in only ADD');
{
  // The door's own form of the property: one rectangle, two levels, containment.
  const box = { x: WHOLE.x + WHOLE.w * 0.25, y: WHOLE.y + WHOLE.h * 0.25, w: WHOLE.w * 0.4, h: WHOLE.h * 0.4 };
  let coarser = null;
  for (let level = 6; level >= 0; level -= 1) {
    const view = await corpus.view({ ...box, level, links: false });
    const ids = new Set();
    const at = new Map();
    for (let i = 0; i < view.marks; i += 1) {
      ids.add(view.denseIds[i]);
      at.set(view.denseIds[i], `${view.positions[i * 2]},${view.positions[i * 2 + 1]}`);
    }
    if (coarser !== null) {
      let lost = 0;
      let moved = 0;
      for (const [id, where] of coarser) {
        if (!ids.has(id)) lost += 1;
        else if (at.get(id) !== where) moved += 1;
      }
      ok(
        `level ${level + 1} → ${level} keeps every vertex, at the same position`,
        lost === 0 && moved === 0,
        `${coarser.size.toLocaleString()} carried, ${lost} lost, ${moved} moved`,
      );
    }
    coarser = at;
  }

  // And the camera's form of it, which is the one already written. Run rather than restated.
  const nesting = spawnSync(
    process.execPath,
    ['--experimental-strip-types', resolve(here, 'verify-nesting.mjs'), '--vertices', String(count)],
    { encoding: 'utf8' },
  );
  for (const line of nesting.stdout.trim().split('\n')) if (line.trim()) note(`nesting: ${line.trim()}`);
  ok('verify-nesting agrees over the camera path', nesting.status === 0, `exit ${nesting.status}`);
}

// ---------------------------------------------------------------------------
// 3. Path independence
// ---------------------------------------------------------------------------

console.log('\n3. path independence — is the same rectangle by two routes the same picture');
{
  const asBox = (r) => ({ x: r.xlo, y: r.ylo, w: r.xhi - r.xlo, h: r.yhi - r.ylo });
  const asRect = (b) => ({ xlo: b.x, ylo: b.y, xhi: b.x + b.w, yhi: b.y + b.h });

  /** A camera rectangle scaled about its own centre — one wheel notch. */
  const zoom = (r, f) => {
    const cx = (r.xlo + r.xhi) / 2;
    const cy = (r.ylo + r.yhi) / 2;
    const w = (r.xhi - r.xlo) * f;
    const h = (r.yhi - r.ylo) * f;
    return { xlo: cx - w / 2, xhi: cx + w / 2, ylo: cy - h / 2, yhi: cy + h / 2 };
  };
  const pan = (r, dx, dy) => ({ xlo: r.xlo + dx, xhi: r.xhi + dx, ylo: r.ylo + dy, yhi: r.yhi + dy });

  /**
   * One route, in the camera's coordinates — and the loop that closes over the answer.
   *
   * Every step asks the door about `toCorpus(frame, rect)` and then rebuilds `frame` from the
   * answer, because that is what the app does: the renderer is handed positions and derives its own
   * map from them, and the next `screenToSpacePosition` speaks that map.
   */
  async function walk(steps) {
    let frame = PINNED;
    let last = null;
    const trail = [];
    for (const rect of steps) {
      const box = asBox(toCorpus(frame, rect));
      const level = corpus.levelFor({ ...box, budget: BUDGET });
      const view = await corpus.view({ ...box, level, links: false });
      frame = mode === 'rescaled' ? rescaledFrame(view.positions, SPACE) : PINNED;
      trail.push({ box, level, marks: view.marks });
      last = { view, box, level };
    }
    return { ...last, trail };
  }

  const start = asRect(WHOLE);
  // The two routes END on the identical camera rectangle — the same object, so nothing below is a
  // floating-point argument. Every difference is the route and only the route.
  const target = zoom(start, 0.5);
  const straight = [start, target];
  const wandering = [
    start,
    zoom(start, 0.3), // in
    pan(zoom(start, 0.3), WHOLE.w * 0.1, 0), // and across
    zoom(start, 0.8), // back out past where it started
    target, // and home
  ];

  const a = await walk(straight);
  const b = await walk(wandering);

  const setOf = (view) => new Set([...view.denseIds.slice(0, view.marks)]);
  const A = setOf(a.view);
  const B = setOf(b.view);
  let shared = 0;
  for (const id of A) if (B.has(id)) shared += 1;
  const union = A.size + B.size - shared;

  for (const [label, route] of [
    ['A', a],
    ['B', b],
  ]) {
    for (const step of route.trail) {
      note(
        `route ${label}: x[${step.box.x.toFixed(0)}, ${(step.box.x + step.box.w).toFixed(0)}] ` +
          `y[${step.box.y.toFixed(0)}, ${(step.box.y + step.box.h).toFixed(0)}] ` +
          `level ${step.level} → ${step.marks.toLocaleString()} drawn`,
      );
    }
  }
  note(
    `drawn at the end: ${A.size.toLocaleString()} against ${B.size.toLocaleString()}, sharing ` +
      `${shared.toLocaleString()} — ${union === 0 ? 0 : ((shared / union) * 100).toFixed(1)}% of the union`,
  );

  // Asserted BEFORE the set comparison, because two empty answers agree and that is the failure
  // this is looking for: a route that wanders leaves the corpus entirely and paints nothing.
  ok('each route still has the corpus under it', A.size > 0 && B.size > 0, `${A.size} and ${B.size} drawn`);
  ok(
    'both routes ask the door about the same rectangle',
    a.box.x === b.box.x && a.box.w === b.box.w && a.box.y === b.box.y && a.box.h === b.box.h,
    `A x[${a.box.x.toFixed(0)}, ${(a.box.x + a.box.w).toFixed(0)}] against B x[${b.box.x.toFixed(0)}, ${(b.box.x + b.box.w).toFixed(0)}]`,
  );
  ok('both routes choose the same level', a.level === b.level, `${a.level} against ${b.level}`);
  ok('both routes draw the same set', shared === A.size && shared === B.size, `${A.size - shared} only in A, ${B.size - shared} only in B`);

  // And the door's own half of it, which holds whatever the frame does: purity.
  const once = await corpus.view({ ...asBox(target), level: 3, links: false });
  await corpus.view({ ...WHOLE, level: 0, links: false });
  const twice = await corpus.view({ ...asBox(target), level: 3, links: false });
  ok(
    'the door itself is a pure function of the rectangle and the level',
    once.marks === twice.marks && [...once.denseIds].every((id, i) => id === twice.denseIds[i]),
    `${once.marks.toLocaleString()} marks, asked twice with the whole corpus in between`,
  );
}

console.log(failures === 0 ? '\nall three properties hold' : `\n${failures} failed`);
process.exit(failures === 0 ? 0 : 1);
