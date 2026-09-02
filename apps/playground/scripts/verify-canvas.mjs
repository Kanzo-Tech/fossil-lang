/**
 * Check the canvas's SOURCE against the corpus, in Node, with no browser and no GPU.
 *
 * `src/tiles.ts` is the half of the canvas that is fossil's, and it is a CONSUMER now: a viewport
 * becomes a finite box, `corpus.levelFor` picks the level, `corpus.view` answers it, and a `View`
 * becomes the parallel typed arrays `@kanzo-tech/graph` uploads. The addressing, the sampling and
 * the SQL are `@fossil-lang/corpus`'s — see `/docs/design/one-door`. The other half — cosmos.gl's
 * lifetime, the query loop, the buffers — is kanzo's, and is tested next door. What this script
 * checks is the composition: that the door, driven through the contract, answers what the window
 * claims.
 *
 * **This exists because the renderer's half cannot be driven here and the source's half can.**
 * cosmos.gl draws from `requestAnimationFrame`, and a browser does not fire one in a tab that is
 * not visible; a tab opened under automation is not. So a pixel read is not available to a check
 * that has to run in CI, and asserting on *the picture* was never on the table. What IS on the
 * table is everything up to the upload: the same `BoundedSource` the component constructs,
 * answered by the same DuckDB SQL, over the same corpus, with every array checked against the
 * window it claims to describe. If this passes and the canvas is blank, the fault is in the
 * renderer or in the five CSS tokens above it — not in what fossil hands over.
 *
 * What it asserts:
 *
 *   1. `extent()` and `total()` answer with **zero queries** — the opening frame issues none.
 *   2. Every drawn mark is inside the window rectangle, and every one of them comes out of a tile
 *      the run list named. A mark from an unopened tile would mean the predicate and the byte
 *      ledger disagree, which is the whole claim. The ledger counts the tiles the DOOR opened,
 *      which is the code anchor's selection and so never more than the footer boxes' — tighter is
 *      the improvement, and this checks the direction rather than an equality.
 *   3. `marks` is a PREFIX: `mark` is true down the sample and false down the anchors, and the
 *      answer is ordered so that a caller can slice rather than filter.
 *   4. Every anchor is an end some link needs, out of a tile that was already read — NOT
 *      necessarily outside the window, which is what this assertion said before it was run.
 *   5. Every link index addresses a row that exists, and at least one of its two ends is a mark.
 *   6. `n` is what the window HELD, not what came back — checked against a plain `count(*)`.
 *   7. Over `limit` the sample is a picture of the WINDOW and not of one corner of it: the drawn
 *      bounding box covers most of the rectangle rather than a prefix arc of the Morton curve.
 *   8. A pinned vertex outside the rectangle comes back anyway, in the same numbering, and its
 *      tile is COUNTED — the fetch it costs is on the ledger rather than hidden in it.
 *   9. An unbounded viewport (`±Infinity`, which is what `shouldSlice` false produces) is a query
 *      DuckDB can run — the `Referenced column "Infinity" not found` regression.
 *
 * Run: `node --experimental-strip-types scripts/verify-canvas.mjs [--vertices 200000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { openCorpus } from '@fossil-lang/corpus';
import { resolveCorpus } from '@fossil-lang/corpus/address';
import { BOUNDED_DEFAULTS, denseOf, typeOf, vertexId } from '@kanzo-tech/graph';

import { query as duckQuery } from '../../corpus/guards/duck.mjs';

/**
 * `./stream.js` from a `.ts` file is TypeScript's own spelling of a relative import, and Node's
 * type stripping resolves it literally. Vite rewrites it; nothing does here, so this hook does —
 * a `.js` specifier with no `.js` beside it, and a `.ts` that is, resolves to the `.ts`.
 *
 * Registered before the modules under test are imported, which is why those two are dynamic: a
 * static import is hoisted above this call and would resolve under the old rules.
 */
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith('.') && specifier.endsWith('.js') && context.parentURL) {
      const asJs = new URL(specifier, context.parentURL);
      const asTs = new URL(`${specifier.slice(0, -3)}.ts`, context.parentURL);
      if (!existsSync(fileURLToPath(asJs)) && existsSync(fileURLToPath(asTs))) {
        return nextResolve(`${specifier.slice(0, -3)}.ts`, context);
      }
    }
    return nextResolve(specifier, context);
  },
});

const { FOOTER_SQL, extentOf, runsOf, selectTiles, toTileBox, windowIn } = await import('../src/stream.ts');
const { corpusSource } = await import('../src/tiles.ts');

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
if (!existsSync(join(root, 'graph.graph.yml'))) {
  console.error(`no corpus at ${root} — run \`node scripts/bench-corpus.mjs --vertices ${count}\``);
  process.exit(2);
}

let failures = 0;
const ok = (label, condition, detail = '') => {
  if (condition) console.log(`  ok   ${label}${detail ? ` — ${detail}` : ''}`);
  else {
    console.log(`  FAIL ${label}${detail ? ` — ${detail}` : ''}`);
    failures++;
  }
};
const note = (text) => console.log(`  ..   ${text}`);

// ---- the addressing, exactly as `bench.ts` builds it, with the filesystem as the base ----
const indexText = readFileSync(join(root, 'graph.graph.yml'), 'utf8');
const manifestPaths = [];
{
  let inList = false;
  for (const raw of indexText.split('\n')) {
    if (/^(vertices|edges):\s*$/.test(raw)) {
      inList = true;
      continue;
    }
    const item = /^-\s+(\S+)\s*$/.exec(raw);
    if (inList && item) manifestPaths.push(item[1]);
    else if (!/^\s*$/.test(raw) && !item) inList = false;
  }
}
const manifestFiles = { 'graph.graph.yml': indexText };
for (const path of manifestPaths) manifestFiles[path] = readFileSync(join(root, path), 'utf8');

// A local directory is a legitimate base for the duckdb CLI: every `tileUrl` it composes is a
// path the binary can open, which is the same string a browser would have fetched.
const addressing = resolveCorpus({ manifestFiles, base: root });
const type = addressing.vertexType();

// ---- the host's one capability, counted ----
//
// No registration step. `openCorpus` composes `read_parquet('<url>')` against the real addresses,
// and so does everything under it; the app's own reads go straight at URLs too, so a name in a
// virtual filesystem would be a second addressing scheme for one of the two paths.
let queries = 0;
const query = async (sql) => {
  queries++;
  return duckQuery(sql);
};

const num = (v) => (typeof v === 'bigint' ? Number(v) : Number(v));

// ---- the footer, once, the way the panel buys it ----
const boxes = duckQuery(FOOTER_SQL(type.tileUrl(0))).map(toTileBox);
const whole = extentOf(boxes);
console.log(
  `corpus: ${count} vertices · ${boxes.length} tiles · x[${whole.xlo}, ${whole.xhi}] y[${whole.ylo}, ${whole.yhi}]`,
);

/** The last ledger the source reported, so the script can check what it says it opened. */
let lastCost = null;
// The door, opened against the same directory the addressing above was resolved from. Everything
// it costs — the manifests, one `DESCRIBE` per type, the tile-code anchors — is paid here, before
// the counter below starts, because none of it is a camera move.
const corpus = await openCorpus(root, { query });
const source = corpusSource({
  corpus,
  boxes,
  onCost: (cost) => {
    lastCost = cost;
  },
});

// ---- 1. the opening frame costs no query ----
console.log('\nopening');
{
  const before = queries;
  const extent = await source.extent();
  const total = await source.total();
  ok('extent() and total() issue no query', queries === before, `${queries - before} queries`);
  ok(
    'extent is the union of the tile boxes',
    extent.xMin === whole.xlo && extent.yMin === whole.ylo && extent.xMax === whole.xhi && extent.yMax === whole.yhi,
  );
  ok('total is the manifest count', total === count, `${total}`);
}

/** One window, sliced, with everything checked against what the window claims. */
async function window(fraction, { limit = BOUNDED_DEFAULTS.limit } = {}) {
  const rect = windowIn(whole, fraction);
  const view = { xMin: rect.xlo, yMin: rect.ylo, xMax: rect.xhi, yMax: rect.yhi };
  const chosen = selectTiles(boxes, rect);
  const runs = runsOf(chosen);
  const bytes = runs.reduce((a, r) => a + r.bytes, 0);
  const payload = boxes.reduce((a, b) => a + b.bytes, 0);

  console.log(`\nwindow ${(fraction * 100).toFixed(0)}% of each axis${limit === BOUNDED_DEFAULTS.limit ? '' : `, limit ${limit}`}`);

  const before = queries;
  const slice = await source.slice({ view, limit, fill: 'cluster_id' });
  const asked = queries - before;

  const rows = slice.positions.length / 2;
  const marks = slice.marks;
  const anchors = rows - marks;

  note(
    `${chosen.length}/${boxes.length} tiles · ${runs.length} runs · ${(bytes / 1024).toFixed(0)} kB` +
      ` = ${((bytes / payload) * 100).toFixed(2)}% of the payload · ${asked} queries`,
  );
  note(`${marks} marks + ${anchors} anchors, of ${slice.n} matched · ${slice.links.length / 2} links`);

  // 2. every mark is inside the rectangle and inside an opened tile.
  const opened = new Set(chosen.map((t) => t.tile));
  const { chunkSize } = type;
  let outsideRect = 0;
  let outsideTiles = 0;
  for (let i = 0; i < marks; i++) {
    const x = slice.positions[i * 2];
    const y = slice.positions[i * 2 + 1];
    const dense = denseOf(slice.vertices[i]);
    if (x < rect.xlo || x > rect.xhi || y < rect.ylo || y > rect.yhi) outsideRect++;
    if (!opened.has(Math.floor(dense / chunkSize))) outsideTiles++;
  }
  ok('every mark is inside the window rectangle', outsideRect === 0, `${outsideRect} outside`);
  ok('every mark comes from a tile the runs named', outsideTiles === 0, `${outsideTiles} from unopened tiles`);

  // 3. every anchor is an end some link needs, and it is free — a tile that was already read.
  //
  // NOT "outside the window": an anchor is a vertex the answer did not DRAW, and under a stride
  // sample a vertex inside the rectangle that was not sampled is one. This assertion said outside
  // and found 420 of the second kind, which is what corrected the wording rather than the code.
  const ends = new Set();
  for (const index of slice.links) ends.add(index);
  let anchorUnused = 0;
  let anchorUnopened = 0;
  let anchorsOutside = 0;
  for (let i = marks; i < rows; i++) {
    const x = slice.positions[i * 2];
    const y = slice.positions[i * 2 + 1];
    const dense = denseOf(slice.vertices[i]);
    if (!ends.has(i)) anchorUnused++;
    if (!opened.has(Math.floor(dense / chunkSize))) anchorUnopened++;
    if (x < rect.xlo || x > rect.xhi || y < rect.ylo || y > rect.yhi) anchorsOutside++;
  }
  ok('every anchor is an end some link needs', anchorUnused === 0, `${anchorUnused} unreferenced`);
  ok('every anchor is free — a tile already read', anchorUnopened === 0, `${anchorUnopened} would need a fetch`);
  if (anchors > 0) note(`${anchorsOutside} of ${anchors} anchors are outside the rectangle; the rest are held but unsampled`);

  // 4. identities are this type's, and distinct.
  const ids = new Set();
  let wrongType = 0;
  for (let i = 0; i < rows; i++) {
    if (typeOf(slice.vertices[i]) !== 0) wrongType++;
    ids.add(slice.vertices[i]);
  }
  ok('every identity carries the type ordinal', wrongType === 0);
  ok('no row is returned twice', ids.size === rows, `${rows - ids.size} duplicates`);

  // 5. links address rows that exist, and touch a mark.
  let badIndex = 0;
  let noMark = 0;
  for (let i = 0; i < slice.links.length; i += 2) {
    const a = slice.links[i];
    const b = slice.links[i + 1];
    if (!Number.isInteger(a) || !Number.isInteger(b) || a < 0 || b < 0 || a >= rows || b >= rows) badIndex++;
    else if (a >= marks && b >= marks) noMark++;
  }
  ok('every link addresses a returned row', badIndex === 0, `${badIndex} out of range`);
  ok('every link has at least one drawn end', noMark === 0, `${noMark} between two anchors`);

  // 6. `n` is what the window held, checked independently.
  const held = num(
    duckQuery(
      `SELECT count(*) AS c FROM read_parquet('${type.tileUrl(0)}')
       WHERE x >= ${rect.xlo} AND x <= ${rect.xhi} AND y >= ${rect.ylo} AND y <= ${rect.yhi}`,
    )[0].c,
  );
  ok('n is what the window held, not what came back', slice.n === held, `${slice.n} vs ${held}`);
  ok('marks never exceed the limit', marks <= limit, `${marks} ≤ ${limit}`);
  // The ledger is the DOOR's tile count, and the door addresses by the published code anchor where
  // one exists — a code range is a tighter description of a tile than the rectangular hull of an
  // arc that snakes. So this is not an equality against `chosen`, which is the footer path: it is
  // that the anchor never opens a tile the footer would not have, and never opens none.
  ok(
    'the ledger counts no more tiles than the footer would have',
    lastCost.tiles > 0 && lastCost.tiles <= chosen.length,
    `${lastCost.tiles} by ${lastCost.addressed} vs ${chosen.length} by footer box`,
  );

  return { slice, rect, marks, held, chosen, runs, bytes, payload };
}

const ten = await window(0.1);

// ---- 7. the sample is a picture of the window, not of a corner of it ----
console.log('\nsampling');
{
  const small = await window(0.1, { limit: 2000 });
  const { slice, rect } = small;
  let xlo = Infinity;
  let xhi = -Infinity;
  let ylo = Infinity;
  let yhi = -Infinity;
  for (let i = 0; i < small.marks; i++) {
    const x = slice.positions[i * 2];
    const y = slice.positions[i * 2 + 1];
    if (x < xlo) xlo = x;
    if (x > xhi) xhi = x;
    if (y < ylo) ylo = y;
    if (y > yhi) yhi = y;
  }
  const coverX = (xhi - xlo) / (rect.xhi - rect.xlo);
  const coverY = (yhi - ylo) / (rect.yhi - rect.ylo);
  note(`2,000 of ${small.held} held — drawn box covers ${(coverX * 100).toFixed(1)}% × ${(coverY * 100).toFixed(1)}% of the window`);
  ok('the sample spans the window in x', coverX > 0.9, `${(coverX * 100).toFixed(1)}%`);
  ok('the sample spans the window in y', coverY > 0.9, `${(coverY * 100).toFixed(1)}%`);
}

// ---- 8. a pin outside the rectangle comes back ----
console.log('\npinned');
{
  const rect = windowIn(whole, 0.1);
  // A vertex from the far corner of the corpus: outside a centred 10% window by construction.
  const far = num(
    duckQuery(
      `SELECT dense_id FROM read_parquet('${type.tileUrl(0)}') ORDER BY x + y DESC LIMIT 1`,
    )[0].dense_id,
  );
  const pin = vertexId(0, far);
  const view = { xMin: rect.xlo, yMin: rect.ylo, xMax: rect.xhi, yMax: rect.yhi };
  // The same window without the pin, so the +1 below is measured against the door's own addressing
  // rather than against a second implementation of it.
  await source.slice({ view, limit: BOUNDED_DEFAULTS.limit, fill: 'cluster_id' });
  const bare = lastCost.tiles;
  const slice = await source.slice({ view, limit: BOUNDED_DEFAULTS.limit, pinned: [pin], fill: 'cluster_id' });
  let found = false;
  for (let i = 0; i < slice.marks; i++) if (denseOf(slice.vertices[i]) === far) found = true;
  ok('a pinned vertex outside the window is returned', found, `dense_id ${far}`);
  ok('its tile is on the ledger, not hidden in it', lastCost.tiles === bare + 1, `${lastCost.tiles} vs ${bare} without it`);
  const other = await source.slice({ view, limit: BOUNDED_DEFAULTS.limit, pinned: [vertexId(9, far)], fill: 'cluster_id' });
  let leaked = false;
  for (let i = 0; i < other.marks; i++) if (denseOf(other.vertices[i]) === far) leaked = true;
  ok("another type's pin is not resolved against this one", !leaked);
}

// ---- 9. the unbounded viewport is a query, not a column reference ----
console.log('\nunbounded');
{
  const view = { xMin: -Infinity, yMin: -Infinity, xMax: Infinity, yMax: Infinity };
  let threw = null;
  let slice = null;
  try {
    slice = await source.slice({ view, limit: 500, fill: 'cluster_id' });
  } catch (cause) {
    threw = String(cause);
  }
  ok('±Infinity is a rectangle DuckDB can answer', threw === null, threw ?? '');
  if (slice) {
    ok('an open rectangle holds the whole corpus', slice.n === count, `${slice.n} of ${count}`);
    ok('an open rectangle still respects the limit', slice.marks <= 500, `${slice.marks}`);
  }
}

// ---- the number the ledger prints, measured here too ----
console.log('\nthe ledger');
note(
  `10% window: ${ten.chosen.length}/${boxes.length} tiles · ${ten.runs.length} runs · ` +
    `${(ten.bytes / 1024).toFixed(0)} kB = ${((ten.bytes / stamp.bytes) * 100).toFixed(2)}% of the corpus`,
);

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} failed`);
process.exit(failures === 0 ? 0 : 1);
