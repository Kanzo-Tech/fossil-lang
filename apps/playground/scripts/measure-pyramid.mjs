/**
 * **What the pyramid buys the default view, measured** — one rectangle, two reads, one corpus.
 *
 * The ratio this prints was an EXTRAPOLATION for as long as it existed: measured over a
 * 300,000-vertex corpus written by the layout pass and scaled to a million, because nothing could
 * write a level set a JavaScript reader could open. Both halves of that are gone —
 * `apps/corpus/guards/fixture.mjs` writes the pyramid now and `@fossil-lang/corpus` reads it — so
 * the number is measurable end to end and this is the thing that measures it.
 *
 * It is an INSTRUMENT and not a test, which is why it asserts nothing about a byte count: a
 * compressor is entitled to change its mind. What it reports is the trade the app makes in
 * `src/tiles.ts` — the same rectangle at the same level, asked with links and without, against a
 * corpus whose manifest declares a pyramid:
 *
 * - **with links**, the payload is opened, because a level file cannot position the far end of an
 *   edge with one end drawn (`crates/fossil-layout/tests/levels.rs` measures what that costs: 0.81%
 *   of a coarse view's edges survive in the level at the app's own three-pixel floor);
 * - **without links**, the level answers, and `cost.matchedAt` says so by exceeding 0.
 *
 * The marks are identical in both, which is the property the whole design rests on: a written level
 * is a cache of `dense_id % 2^k == 0` and changes the byte count rather than the answer.
 *
 * Run: `node scripts/measure-pyramid.mjs [--vertices 1000000] [--budget 20000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { levelsOf, open } from '@fossil-lang/corpus';
import { query as duckQuery } from '../../corpus/guards/duck.mjs';

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

/** The renderer's own cap — `BOUNDED_DEFAULTS.limit` in `@kanzo-tech/graph`. */
const BUDGET = Number(flag('budget', 20000));

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

const corpus = await open(root, {
  query: async (sql) => duckQuery(sql),
  wasmUrl: CORPUS_WASM,
});
const type = corpus.types.vertices[0].type;
const extent = await corpus.extent();
if (extent === null) {
  console.error(`${type} carries no positions, so no rectangle names any of it`);
  process.exit(2);
}
// The whole extent, padded off the far edge because a box is half-open there.
const pad = 1e-3;
const box = {
  x: extent.minX - pad,
  y: extent.minY - pad,
  w: extent.maxX - extent.minX + 2 * pad,
  h: extent.maxY - extent.minY + 2 * pad,
};

const written = levelsOf(corpus.addressing, type).filter((l) => l.written);
// The door takes the canvas now and derives the level from it at one mark per pixel, so a
// budget of BUDGET marks is the square that holds them. `frame.level` reports what it chose.
const side = Math.sqrt(BUDGET);
const pixels = { w: side, h: side };
const level = (await corpus.frame({ ...box, type, pixels, links: false })).level;
console.log(`\n${type} · ${count.toLocaleString('en-US')} vertices · budget ${BUDGET.toLocaleString('en-US')} marks`);
console.log(
  `  written levels ${written.length === 0 ? 'none' : written.map((l) => l.level).join(', ')} · ` +
    `levelFor(whole extent) = ${level}${written.some((l) => l.level === level) ? '' : ' — NOT written'}`,
);

const kB = (bytes) => `${Math.round(bytes / 1024).toLocaleString('en-US')} kB`;
const answers = [];
for (const links of [true, false]) {
  const started = Date.now();
  const view = await corpus.frame({ ...box, type, level, links });
  answers.push({ links, view, ms: Date.now() - started });
  console.log(
    `  links ${String(links).padEnd(5)} · counted at ${String(view.matchedAt).padEnd(2)} · ` +
      `${String(view.cost.tiles).padStart(3)}/${view.cost.ofTiles} tile(s) in ${view.cost.requests} request(s) · ` +
      `${kB(view.cost.bytes).padStart(10)} · marks ${view.marks.toLocaleString('en-US')} · ` +
      `anchors ${(view.positions.length / 2 - view.marks).toLocaleString('en-US')} · ` +
      `links ${(view.links.length / 2).toLocaleString('en-US')} · ` +
      `matched ${view.matched.toLocaleString('en-US')} at level ${view.matchedAt}`,
  );
}

// The same picture off the PAYLOAD, for the comparison to be a comparison. A level one finer than
// the finest written one is not a level this corpus wrote, so it is answered by striding the tiles
// and opening the adjacency — the read the pyramid replaces, at the same rectangle.
const finest = written.length === 0 ? null : Math.min(...written.map((l) => l.level));
if (finest !== null && finest > 0) {
  const view = await corpus.frame({ ...box, type, level: finest - 1, links: true });
  console.log(
    `  payload  · counted at ${String(view.matchedAt).padEnd(2)} · ` +
      `${String(view.cost.tiles).padStart(3)}/${view.cost.ofTiles} tile(s) · ${kB(view.cost.bytes).padStart(10)} · ` +
      `marks ${view.marks.toLocaleString('en-US')} · ` +
      `anchors ${(view.positions.length / 2 - view.marks).toLocaleString('en-US')} · ` +
      `links ${(view.links.length / 2).toLocaleString('en-US')} — level ${finest - 1}, which nobody wrote`,
  );
  const cheap = answers[0];
  if (cheap.view.matchedAt > 0) {
    console.log(
      `\n  ${(view.cost.bytes / Math.max(cheap.view.cost.bytes, 1)).toFixed(1)}× fewer bytes for the same ` +
        `picture — ${cheap.view.cost.tiles} tile(s) rather than ${view.cost.tiles}, ` +
        `${cheap.view.links.length / 2} links rather than ${view.links.length / 2}`,
    );
  }
}

const [, points] = answers;
if (points.view.matchedAt === 0) {
  // Not a failure and worth saying: the camera's level and the writer's window are set by
  // different rules — a budget in marks against a coarsest level that fits one tile — and they
  // meet at some corpus sizes and miss by one at others. At 300,000 vertices the budget asks for
  // level 4 and the pyramid starts at 5.
  console.log(
    `\n  the pyramid is not read here: the budget asks for level ${level} and this corpus writes ` +
      `${written.map((l) => l.level).join(', ') || 'none'}`,
  );
}
