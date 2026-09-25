/**
 * **What one frame costs, per camera rectangle** — the baseline the renderer decision is made
 * against.
 *
 * There is an open question beside this app: whether `@cosmos.gl/graph` should be replaced by
 * Cosmograph, which is reported to be far more efficient. It was deliberately left open until
 * somebody measured something. This is that measurement — and it is careful about *which half* it
 * measures, because the two halves are settled by different evidence:
 *
 *   - **Everything up to the upload is fossil's**, and it is what this script sees: which tiles a
 *     camera rectangle opens, how many range requests those collapse into, the bytes behind them,
 *     what the answer holds, how much of it is actually drawn, the level it was counted at, and the
 *     wall clock to get there. None of it changes when the engine changes. A renderer swap cannot
 *     make a rectangle open fewer tiles.
 *   - **Everything after the upload is the renderer's**, and this script cannot see any of it —
 *     no fps, no GPU upload, no draw call, no overdraw. `verify-canvas.mjs` states the reason and
 *     it is the same one here: cosmos.gl draws from `requestAnimationFrame` and a browser fires
 *     none in a tab that is not visible, which a tab opened under automation is not.
 *
 * So the number this hands the decision is the **floor**: the work the architecture imposes on any
 * renderer. Measure an engine against it and whatever gap is left over is the engine's. Measure it
 * again after residency and visibility are separated
 * (`apps/docs/content/docs/design/camera.mdx`) and the same columns say how much of the gap was
 * architectural — Table 3 is already the arithmetic for that, today, with no cache in the app.
 *
 * ## The camera path is stated, not sampled
 *
 * Ten rectangles, written down below as `PATH`, in order: open the whole corpus, zoom to the
 * centre in three steps, pan twice, zoom deeper twice, pull back out, and return home. No RNG, no
 * sampling, no timestamp in any input. Every column but the clock is a function of the corpus and
 * the path, so two runs produce the same table and the `digest` line at the bottom is how that is
 * checked in one comparison rather than twelve.
 *
 * Each frame is asked `--repeat` times (3 by default). **If the cost columns disagree between
 * repeats the script exits 1 and says which frame moved**, because a bench that flaps is not
 * evidence.
 *
 * ## The caveat this bench inherits from its corpus
 *
 * `apps/corpus/guards/fixture.mjs`'s `positions()` packs each cluster at radius `12*sqrt(count /
 * clusters)` on a grid of CONSTANT spacing 100. At a million vertices over 128 clusters that radius
 * is about 1,061 while the grid contributes 1,100 — so every disc overlaps every other one and the
 * corpus is one blur. Table 0 re-derives the ratio live rather than restating it, and
 * `measure-locality.mjs` and `realkg-prepare.mjs` carry the same warning.
 *
 * **It does not make the byte and request numbers wrong** — they are what this corpus costs, and
 * the tiles really do hold those bytes. It bounds one claim and only one: nothing here says what
 * spatial SELECTIVITY buys, because at the whole extent every tile intersects every rectangle
 * worth naming. A corpus with separated communities would open fewer tiles for the same drawn set;
 * this one cannot show that, and `realkg-prepare.mjs` exists to get a corpus that can.
 *
 * ## What is bounded, said out loud
 *
 *   - **20,000 marks** — `BOUNDED_DEFAULTS.limit`, the renderer's own cap, which `src/tiles.ts`
 *     converts into the canvas the door derives a level from. A frame that reaches it is flagged
 *     `capped` in Table 2.
 *   - **The canvas is 920x400** and it feeds ONE thing: the three-pixel link-length floor, via
 *     `perPixel`. It does not choose the level — `src/tiles.ts` explains at length why the level
 *     comes from the renderer's mark budget instead, and this script does not second-guess it.
 *   - **`--repeat 3`** — repeats of each frame, for the clock only.
 *   - Nothing else is capped, sampled or truncated. The whole path runs; every frame is reported.
 *
 * Run: `node --experimental-strip-types scripts/measure-frame.mjs [--vertices 1000000]
 *       [--repeat 3] [--canvas 920x400] [--limit 20000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { levelsOf, open } from '@fossil-lang/corpus';
import { BOUNDED_DEFAULTS } from '@kanzo-tech/graph';

import { query as duckQuery, lit } from '../../corpus/guards/duck.mjs';

/**
 * The reader's wasm, as BYTES rather than a URL.
 *
 * `open` resolves a manifest through `fossil_graph::plan` compiled to wasm32, and Node is
 * the host that has to say where that lives. A `file://` URL is the obvious answer and it does not
 * work: wasm-bindgen's init calls `fetch`, and undici refuses the `file:` scheme with «not
 * implemented... yet...». A `Response` over the bytes is in the accepted union and needs no
 * network, which is also what a script reading a corpus off local disk should be doing.
 */
const CORPUS_WASM = new Response(
  await readFile(new URL('../node_modules/@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
  { headers: { 'content-type': 'application/wasm' } },
);

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

/** The renderer's own cap — `BOUNDED_DEFAULTS.limit` in `@kanzo-tech/graph`. */
const LIMIT = Number(flag('limit', BOUNDED_DEFAULTS.limit));
/** How many times each frame is asked. The clock is noisy; every other column must not be. */
const REPEAT = Math.max(1, Number(flag('repeat', 3)));
/**
 * The canvas, stated. It is the size `src/tiles.ts` measured on this corpus, and it feeds the
 * three-pixel link floor and nothing else.
 */
const CANVAS = (() => {
  const raw = String(flag('canvas', '920x400'));
  const m = /^(\d+)x(\d+)$/.exec(raw);
  if (!m) {
    console.error(`--canvas is WxH; got ${raw}`);
    process.exit(2);
  }
  return { w: Number(m[1]), h: Number(m[2]) };
})();

/**
 * **The camera path.** Ten rectangles, in order, each a fraction of every axis of the corpus extent
 * centred at a normalised point — which is exactly `windowIn`'s argument, so the geometry is the
 * app's own and not a second one.
 *
 * `fraction: null` is the viewport a renderer passes when the graph FITS: `±Infinity` on every
 * edge, which `boxFor` clamps against the footer's extent. It is here twice on purpose — once at
 * the start and once at the end — because the path's whole point is that a camera comes back, and
 * Table 3 has nothing to say about residency unless it does.
 */
const PATH = [
  { move: 'open', fraction: null, cx: 0.5, cy: 0.5 },
  { move: 'zoom 50%', fraction: 0.5, cx: 0.5, cy: 0.5 },
  { move: 'zoom 25%', fraction: 0.25, cx: 0.5, cy: 0.5 },
  { move: 'zoom 10%', fraction: 0.1, cx: 0.5, cy: 0.5 },
  { move: 'pan east', fraction: 0.1, cx: 0.65, cy: 0.5 },
  { move: 'pan south', fraction: 0.1, cx: 0.65, cy: 0.35 },
  { move: 'zoom 4%', fraction: 0.04, cx: 0.65, cy: 0.35 },
  { move: 'zoom 1.6%', fraction: 0.016, cx: 0.65, cy: 0.35 },
  { move: 'out to 25%', fraction: 0.25, cx: 0.65, cy: 0.35 },
  { move: 'home', fraction: null, cx: 0.5, cy: 0.5 },
];

// ---------------------------------------------------------------------------------------------
// The host's one capability, counted. Every query this script or the door issues goes through it.

let queries = 0;
const query = async (sql) => {
  queries += 1;
  return duckQuery(sql);
};

const num = (v) => (typeof v === 'bigint' ? Number(v) : Number(v));
const N = (v) => Number(v).toLocaleString('en-US');
const MB = (bytes) => `${(Number(bytes) / 1e6).toFixed(2)} MB`;
const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2;
};
const table = (head, rows) =>
  [`| ${head.join(' | ')} |`, `|${head.map(() => '---').join('|')}|`, ...rows.map((r) => `| ${r.join(' | ')} |`)].join(
    '\n',
  );

// ---------------------------------------------------------------------------------------------
// The floor under every clock in this script, measured rather than guessed.
//
// `guards/duck.mjs` spawns a FRESH `duckdb` process per statement — right for a guard, and the
// single largest term in any wall clock here. The browser pays none of it: DuckDB-WASM is booted
// once inside a Worker. So the per-query spawn cost is measured and Table 2 reports how much of
// each frame's milliseconds is floor.

process.stderr.write('measuring the engine floor…\n');
const spawnSamples = [];
for (let i = 0; i < 7; i += 1) {
  const at = performance.now();
  duckQuery('SELECT 1 AS one');
  spawnSamples.push(performance.now() - at);
}
const SPAWN_MS = median(spawnSamples);

// ---------------------------------------------------------------------------------------------
// The door, and the footer the panel buys once.

process.stderr.write('opening the corpus…\n');

const corpus = await open(root, { query, wasm: CORPUS_WASM });
const address = corpus.addressing.vertexType();
const payload = address.tileUrl(0);
const boxes = duckQuery(FOOTER_SQL(payload)).map(toTileBox);
const whole = extentOf(boxes);
const payloadBytes = boxes.reduce((a, b) => a + b.bytes, 0);

/** The last ledger the source reported — `SliceCost`, which is what the panel prints. */
let lastCost = null;
const source = corpusSource({
  corpus,
  boxes,
  onCost: (cost) => {
    lastCost = cost;
  },
});

// ---------------------------------------------------------------------------------------------
// Table 0's live facts. The disc-over-extent ratio is the caveat in the header, re-derived, so it
// cannot silently stop being true.

process.stderr.write('deriving the corpus facts…\n');
const shape = duckQuery(`
  WITH ext AS (SELECT max(x) - min(x) AS w FROM read_parquet('${lit(payload)}')),
       disc AS (SELECT cluster_id, max(x) - min(x) AS w, avg(x) AS cx
                FROM read_parquet('${lit(payload)}') GROUP BY cluster_id),
       grid AS (SELECT max(cx) - min(cx) AS span FROM disc)
  SELECT count(*) AS clusters,
         avg(disc.w) AS mean_disc,
         any_value(ext.w) AS extent,
         any_value(grid.span) AS centre_span,
         avg(disc.w) / any_value(ext.w) AS over_extent,
         avg(disc.w) / any_value(grid.span) AS over_centres
  FROM disc, ext, grid`)[0];
const overExtent = num(shape.over_extent);
const overCentres = num(shape.over_centres);
const written = levelsOf(corpus.addressing, address.type).filter((l) => l.written);

// ---------------------------------------------------------------------------------------------
// One warm-up frame, discarded from the table and reported on its own line.
//
// The door does its per-corpus setup — one `DESCRIBE` per type, the level manifest — on the first
// `frame()`. That is a real cost and it is paid once per session, so putting it on the first
// camera MOVE would be charging a pan for something a pan does not pay.

process.stderr.write('warm-up frame…\n');
const warmQueries = queries;
const warmAt = performance.now();
await source.slice({
  view: { xMin: -Infinity, yMin: -Infinity, xMax: Infinity, yMax: Infinity },
  limit: LIMIT,
  fill: 'cluster_id',
});
const warm = { ms: performance.now() - warmAt, queries: queries - warmQueries, cost: lastCost };

// ---------------------------------------------------------------------------------------------
// The path.

/**
 * The deterministic half of one frame — everything but the clock, **and everything but the query
 * count**.
 *
 * No `level` field, and the absence is deliberate: a `SliceCost` carries `matchedAt`, which is the
 * level the answer was COUNTED at, and nothing else says which level was drawn. Deriving one from
 * `matched / marks` would be an invented number sitting in a column of measured ones — the door's
 * own doc makes the same refusal about `cost.read`.
 *
 * **`queries` is out because it is a fact about the session and not about the frame.** The door
 * memoises what it weighs — `bytesOf` is keyed on the URL list and the column list — so the second
 * visit to a level costs three fewer `parquet_metadata` sweeps than the first. Measured here: the
 * 25% and 10% frames issue 5 queries the first time and 2 every time after. That is real, it is
 * reported in its own column as `first → steady`, and it is not a difference in what the frame
 * ANSWERED, which is what these repeats exist to hold still. The first visit's count is
 * deterministic across processes and goes into the digest.
 */
const keyOf = (f) =>
  JSON.stringify([f.matchedAt, f.tiles, f.ofTiles, f.requests, f.bytes, f.matched, f.marks, f.anchors, f.links, f.rows]);

const frames = [];
let flapped = 0;

for (const step of PATH) {
  process.stderr.write(`frame ${step.move}…\n`);
  const rect = step.fraction === null ? whole : windowIn(whole, step.fraction, step.cx, step.cy);
  const view =
    step.fraction === null
      ? { xMin: -Infinity, yMin: -Infinity, xMax: Infinity, yMax: Infinity }
      : { xMin: rect.xlo, yMin: rect.ylo, xMax: rect.xhi, yMax: rect.yhi };
  // Corpus units per pixel — the renderer's own conversion, so `src/tiles.ts` can turn its
  // three-PIXEL link floor into a length in the corpus's units. Nothing else consumes it.
  const perPixel = (rect.xhi - rect.xlo) / CANVAS.w;

  // The footer's own answer to the same rectangle, which is what the streaming panel shows and
  // what Table 3 differences. `verify-canvas` asserts the door never opens MORE than this.
  const chosen = selectTiles(boxes, rect);
  const runs = runsOf(chosen);
  const footerBytes = runs.reduce((a, r) => a + r.bytes, 0);

  const repeats = [];
  for (let r = 0; r < REPEAT; r += 1) {
    const before = queries;
    const slice = await source.slice({ view, limit: LIMIT, perPixel, fill: 'cluster_id' });
    repeats.push({
      matchedAt: lastCost.matchedAt,
      tiles: lastCost.tiles,
      ofTiles: lastCost.ofTiles,
      requests: lastCost.requests,
      bytes: lastCost.bytes,
      matched: lastCost.matched,
      marks: lastCost.marks,
      anchors: lastCost.anchors,
      links: lastCost.links,
      rows: slice.positions.length / 2,
      queries: queries - before,
      ms: lastCost.ms,
    });
  }

  const keys = new Set(repeats.map(keyOf));
  if (keys.size !== 1) flapped += 1;

  frames.push({
    ...step,
    rect,
    perPixel,
    ...repeats[0],
    stable: keys.size === 1,
    keys: [...keys],
    msFirst: repeats[0].ms,
    // The steady state is every repeat but the first, because the first pays the door's memoised
    // weighing. With `--repeat 1` there is no steady state to separate and the one sample is both.
    msSteady: repeats.length > 1 ? repeats.slice(1).map((r) => r.ms) : [repeats[0].ms],
    queriesFirst: repeats[0].queries,
    queriesSteady: repeats[repeats.length - 1].queries,
    footerTiles: chosen.map((t) => t.tile),
    footerRuns: runs.length,
    footerBytes,
  });
}

// ---------------------------------------------------------------------------------------------
// Residency — what a cache would not have had to fetch.
//
// Over the FOOTER's tile selection rather than the door's, because a `SliceCost` reports how many
// tiles were opened and not which ones. `verify-canvas` holds the door's count at or below the
// footer's, so this is an upper bound on the set and therefore a conservative statement of what a
// cache would already hold.

const union = new Set();
let fetches = 0;
for (let i = 0; i < frames.length; i += 1) {
  const mine = new Set(frames[i].footerTiles);
  const prev = i === 0 ? new Set() : new Set(frames[i - 1].footerTiles);
  let resident = 0;
  for (const t of mine) if (prev.has(t)) resident += 1;
  let held = 0;
  for (const t of mine) if (union.has(t)) held += 1;
  frames[i].resident = resident;
  frames[i].held = held;
  for (const t of mine) union.add(t);
  fetches += mine.size;
}

// ---------------------------------------------------------------------------------------------
// Report

const digest = createHash('sha256')
  .update(
    JSON.stringify(
      frames.map((f) => [f.move, f.fraction, f.cx, f.cy, keyOf(f), f.footerTiles.length, f.footerRuns, f.footerBytes, f.resident, f.held]),
    ),
  )
  .digest('hex')
  .slice(0, 16);

console.log(`# What a frame costs — measured

Corpus: \`${root}\`
${N(count)} vertices · ${N(stamp.edges)} edges · ${N(boxes.length)} payload tiles · ${MB(payloadBytes)} of vertex payload
Camera: ${PATH.length} rectangles, stated in Table 1, in order. No RNG, no sampling, no timestamp in any input.
Budget: ${N(LIMIT)} marks (\`BOUNDED_DEFAULTS.limit\`) · canvas ${CANVAS.w}x${CANVAS.h} (the link floor only) · ${REPEAT} repeats per frame
Engine: the \`duckdb\` binary, one process per statement, floor ${SPAWN_MS.toFixed(1)} ms/query measured over 7 \`SELECT 1\`s
Digest: \`${digest}\` — every column but the clock hashes into it, so two runs agree in one comparison.

## Table 0 — the corpus, and the caveat this bench inherits

${table(
  ['fact', 'value'],
  [
    ['vertices', N(count)],
    ['payload tiles', `${N(boxes.length)} row groups, ${MB(payloadBytes)}`],
    ['written levels', written.length === 0 ? 'none' : written.map((l) => `l${l.level}`).join(', ')],
    ['clusters', N(shape.clusters)],
    ['corpus extent in x', num(shape.extent).toFixed(0)],
    ['mean cluster disc width', num(shape.mean_disc).toFixed(0)],
    ['span of the cluster centres in x', num(shape.centre_span).toFixed(0)],
    ['**disc width / corpus extent**', `**${overExtent.toFixed(3)}**`],
    ['**disc width / span of the centres**', `**${overCentres.toFixed(3)}**`],
  ],
)}

${
  overCentres > 1
    ? `A disc **${overCentres.toFixed(2)}× wider than the entire grid its centres sit on** means all ${N(shape.clusters)} overlap: this corpus is one blur, and each disc alone covers ${(overExtent * 100).toFixed(0)}% of the picture.`
    : `The discs are narrower than the span of their centres (${overCentres.toFixed(2)}×), so this corpus has spatially separated communities and the caveat below does not bind.`
} \`positions()\` in \`apps/corpus/guards/fixture.mjs\` packs a cluster at radius
\`12*sqrt(count/clusters)\` on a grid of CONSTANT spacing 100, so the discs outgrow the grid as the
corpus grows and the degeneracy is a function of the size. \`measure-locality.mjs\` and
\`realkg-prepare.mjs\` record the same thing.

**It does not make the bytes or the requests below wrong** — those are what this corpus costs, and
the tiles really do hold those bytes. It bounds one claim and only one: nothing here says what
spatial SELECTIVITY buys, because at these rectangles a tile that intersects is the rule rather than
the exception. \`realkg-prepare.mjs\` exists to get a corpus that can support that claim.

## Table 1 — the camera path

\`fraction\` is the share of each axis; \`centre\` is normalised into the extent — together they are
\`windowIn(extent, fraction, cx, cy)\`, the app's own geometry. \`open\` and \`home\` pass the
\`±Infinity\` viewport a renderer sends when the graph fits, which \`boxFor\` clamps to the footer's
extent.

${table(
  ['#', 'move', 'fraction', 'centre', 'rectangle x', 'rectangle y', 'corpus units / pixel'],
  frames.map((f, i) => [
    i + 1,
    f.move,
    f.fraction === null ? '± ∞' : `${(f.fraction * 100).toFixed(1)}%`,
    `${f.cx}, ${f.cy}`,
    `[${f.rect.xlo.toFixed(0)}, ${f.rect.xhi.toFixed(0)}]`,
    `[${f.rect.ylo.toFixed(0)}, ${f.rect.yhi.toFixed(0)}]`,
    f.perPixel.toFixed(3),
  ]),
)}

## Table 2 — what each frame cost

\`tiles\` and \`requests\` are the DOOR's ledger — the tiles it addressed and the maximal runs of
adjacent ones those collapse into, one \`Range\` request each. \`of\` is how many tiles the artefact
that ANSWERED has, so it moves with the level rather than staying at the payload's own count.

**\`bytes\` is footer arithmetic** — \`sum(total_compressed_size)\` out of \`parquet_metadata\`, NOT
bytes weighed on a wire. DuckDB reads inside its own process, a Worker in the browser, where the
caller cannot weigh them; that is the same reason \`src/stream.ts\` fetches the payload itself when
it wants an observed number. Two halves go into it and the door weighs them differently: the vertex
half is the SELECTED tiles over the four columns a view draws with, the edge half is **the whole
relation file** over its endpoint columns. That is why the column does not fall with \`tiles\`, and
why a frame reading 3 tiles of 62 still reports more bytes than the entire vertex payload.

\`matched\` is what the rectangle HOLDS at the level it was counted at (\`at\`), \`marks\` is what is
drawn, \`anchors\` are far ends positioned to hold an edge down but never painted, and **\`points\`
is \`marks + anchors\` — the array actually handed to the renderer**, which is the one column an
engine swap operates on.

${table(
  ['#', 'move', 'at', 'tiles', 'of', 'requests', 'bytes', 'matched', 'marks', 'anchors', 'points', 'links', 'ms first', 'ms steady', 'range', 'queries', 'floor'],
  frames.map((f, i) => [
    i + 1,
    f.move + (f.marks >= LIMIT ? ' **capped**' : ''),
    f.matchedAt,
    N(f.tiles),
    N(f.ofTiles),
    N(f.requests),
    MB(f.bytes),
    N(f.matched),
    N(f.marks),
    N(f.anchors),
    N(f.rows),
    N(f.links),
    f.msFirst.toFixed(0),
    median(f.msSteady).toFixed(0),
    `${Math.min(...f.msSteady).toFixed(0)}–${Math.max(...f.msSteady).toFixed(0)}`,
    f.queriesFirst === f.queriesSteady ? `${f.queriesFirst}` : `${f.queriesFirst} → ${f.queriesSteady}`,
    `${((f.queriesSteady * SPAWN_MS * 100) / Math.max(1, median(f.msSteady))).toFixed(0)}%`,
  ]),
)}

**\`ms first\` and \`ms steady\` are two questions, and the gap between them is the door's memoisation
rather than noise.** \`bytesOf\` is keyed on the URL list and the column list, so a second visit to a
level skips the \`parquet_metadata\` sweeps; the \`queries\` column names every frame where that
happened as \`first → steady\`. **Nothing about the ANSWER moves** — every cost column was identical
across all ${REPEAT} repeats of every frame, which is what the digest hashes and what makes this a
bench rather than a sample.

\`floor\` is the share of the steady median that is this script's OWN engine boot —
\`${SPAWN_MS.toFixed(1)} ms\` times the queries the frame issued. A browser pays none of it: DuckDB-WASM
boots once per tab. Subtract it before comparing any millisecond here with a browser's.

Opening the corpus cost ${warm.queries} ${warm.queries === 1 ? 'query' : 'queries'} and
${warm.ms.toFixed(0)} ms once, outside the table: the door's \`DESCRIBE\` and level manifest are paid
per session, not per camera move.

${frames.some((f) => f.marks >= LIMIT) ? `**${frames.filter((f) => f.marks >= LIMIT).length} frame(s) reached the ${N(LIMIT)}-mark cap** and are flagged above — their \`marks\` is the budget rather than the picture.` : `No frame reached the ${N(LIMIT)}-mark cap, so every \`marks\` above is the level's own answer rather than a budget.`}

## Table 3 — residency: what a cache would not have re-fetched

Today **residency and visibility are the same set**: this app holds no tiles between frames, so
every camera move re-reads. \`already open\` counts the tiles the PREVIOUS frame selected; \`ever
seen\` counts the tiles any earlier frame on this path selected, which is what a cache with room for
the corpus would have held. Both are over the footer's selection rather than the door's, because a
\`SliceCost\` says how many tiles were opened and not which — \`verify-canvas\` holds the door's count
at or below the footer's, so these shares are a conservative floor on what a cache would save.

${table(
  ['#', 'move', 'tiles (footer)', 'runs', 'bytes (footer)', 'already open', 'ever seen', 'new'],
  frames.map((f, i) => [
    i + 1,
    f.move,
    N(f.footerTiles.length),
    N(f.footerRuns),
    MB(f.footerBytes),
    `${N(f.resident)} (${((f.resident / Math.max(1, f.footerTiles.length)) * 100).toFixed(0)}%)`,
    `${N(f.held)} (${((f.held / Math.max(1, f.footerTiles.length)) * 100).toFixed(0)}%)`,
    N(f.footerTiles.length - f.held),
  ]),
)}

Over the whole path: **${N(fetches)} tile fetches across ${N(union.size)} distinct tiles** of
${N(boxes.length)}. A reader that kept what it read would have issued ${N(union.size)} —
${(fetches / Math.max(1, union.size)).toFixed(1)}× fewer. That ratio is the architecture's share of
the cost and no renderer can change it.

## What this instrument cannot see

It runs in Node against a corpus on disk. It therefore reports **nothing** about:

- **fps, frame time, or jank.** cosmos.gl draws from \`requestAnimationFrame\` and a browser fires
  none in a tab that is not visible, which a tab opened under automation is not. There is no number
  here that can be compared with a frame budget.
- **GPU upload cost, VRAM, draw calls, overdraw, or shader time.** There is no GPU in this process.
  The closest this gets is the \`points\` and \`links\` columns — the size of the arrays a renderer is
  handed — which is the input to that cost and not the cost.
- **Bytes on a wire.** Every byte column is footer arithmetic over the tiles that were addressed.
  \`scripts/verify-stream.mjs\` is the one that fetches over real HTTP with \`Range\` and weighs the
  responses; this one does not fetch.
- **The browser's own latency.** The clock here includes one \`duckdb\` process spawn per query
  (${SPAWN_MS.toFixed(1)} ms, measured); DuckDB-WASM boots once per tab and answers from a Worker.
- **Whether Cosmograph is faster than cosmos.gl.** Nothing in this file touches either. What it
  establishes is the floor both would stand on.
- **What spatial selectivity buys**, for the reason Table 0 states.

digest: ${digest}
`);

if (flapped > 0) {
  console.error(`\n${flapped} frame(s) disagreed with themselves across ${REPEAT} repeats:`);
  for (const f of frames) if (!f.stable) console.error(`  ${f.move}: ${f.keys.join('  vs  ')}`);
  console.error('a bench that flaps is not evidence.');
  process.exit(1);
}
process.stderr.write(`\nstable across ${REPEAT} repeats of every frame · digest ${digest}\n`);
