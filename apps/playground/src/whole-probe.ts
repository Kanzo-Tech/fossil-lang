/**
 * The two paths, measured against each other in a real browser, with no GPU in the way.
 *
 * **Not part of the app.** A second entry point Vite serves in dev and does not bundle, in the
 * shape `src/frame-probe.ts` already established. Open `/whole-probe.html` with the dev server up
 * and the bench corpus generated.
 *
 * It exists because the interesting half of the comparison cannot be measured through the canvas.
 * cosmos.gl needs a WebGL context and `requestAnimationFrame`, and a tab opened under automation
 * has neither — the canvas reports *this browser offers no WebGL context* and the query loop never
 * asks anything, so the ledger stays at zero and measures nothing. What does not need a GPU is
 * everything up to the upload: the same two `BoundedSource`s the canvas mounts, over the same
 * corpus, through the same one DuckDB-WASM connection, asked the same rectangles. That is what
 * this drives.
 *
 * `scripts/verify-canvas.mjs` makes the same argument for the same reason and answers it in Node,
 * which is right for a correctness check and wrong for a cost one: the numbers that matter here are
 * a browser's — DuckDB-WASM in a Worker, and a million row objects crossing into JS through
 * `QueryFn` — and the `duckdb` binary answering over JSON on stdin is neither of those.
 *
 * What it prints is the table the ledger beside the canvas prints, filled in.
 */
import { openCorpus } from '@fossil-lang/corpus';

import { openBench } from './bench.js';
import * as duck from './duckdb.js';
import { FOOTER_SQL, extentOf, toTileBox, windowIn, type Rect } from './stream.js';
import { corpusSource, type SliceCost } from './tiles.js';
import { wholeSource, type WholeCost } from './whole.js';

const out = document.getElementById('out')!;
const say = (line: string) => {
  out.textContent += `${line}\n`;
};

/** A canvas width to convert the pixel floor against — `.can-surface` is roughly this wide. */
const PIXELS = 900;

/** The camera moves, as fractions of each axis and a centre in normalised coordinates. */
const MOVES: { label: string; f: number; cx: number; cy: number }[] = [
  { label: 'zoom to 30%', f: 0.3, cx: 0.5, cy: 0.5 },
  { label: 'pan right', f: 0.3, cx: 0.65, cy: 0.5 },
  { label: 'zoom to 10%', f: 0.1, cx: 0.65, cy: 0.5 },
  { label: 'pan down', f: 0.1, cx: 0.65, cy: 0.65 },
  { label: 'zoom to 2%', f: 0.02, cx: 0.65, cy: 0.65 },
];

const viewOf = (rect: Rect) => ({ xMin: rect.xlo, yMin: rect.ylo, xMax: rect.xhi, yMax: rect.yhi });

/** `usedJSHeapSize`, when the browser publishes it. Chrome does; the spec does not. */
const heap = (): number | null => {
  const memory = (performance as { memory?: { usedJSHeapSize: number } }).memory;
  return memory ? memory.usedJSHeapSize : null;
};
const MB = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;

async function main(): Promise<void> {
  out.textContent = '';
  say('booting duckdb-wasm…');
  await duck.boot();

  const bench = await openBench();
  if (!bench) {
    say('no bench corpus — run `node scripts/bench-corpus.mjs --vertices 1000000` first');
    return;
  }
  const address = bench.addressing.vertexType();

  say('reading the footer…');
  const boxes = (await duck.query(FOOTER_SQL(address.tileUrl(0)))).map(toTileBox);
  const extent = extentOf(boxes)!;
  say(
    `corpus: ${bench.stamp.count.toLocaleString()} vertices · ${bench.stamp.edges.toLocaleString()} edges · ` +
      `${boxes.length} tiles · ${MB(bench.stamp.bytes)} on disk`,
  );
  say('');

  const corpus = openCorpus(bench.base, { query: duck.query });

  // ---- the windowed path ----
  let streamCost: SliceCost | null = null;
  const streaming = corpusSource({
    corpus,
    boxes,
    slots: 8,
    onCost: (cost) => {
      streamCost = cost;
    },
  });

  say('streaming — the tiles the rectangle touches, on every move');
  let streamBytes = 0;
  let streamFirst = 0;
  const streamMoves: number[] = [];
  for (const [i, move] of [{ label: 'first paint (whole extent)', f: 1, cx: 0.5, cy: 0.5 }, ...MOVES].entries()) {
    const rect = move.f === 1 ? extent : windowIn(extent, move.f, move.cx, move.cy);
    const view = viewOf(rect);
    const perPixel = (view.xMax - view.xMin) / PIXELS;
    const started = performance.now();
    const slice = await streaming.slice({ view, perPixel, limit: 20000, fill: 'cluster_id' });
    const ms = performance.now() - started;
    const cost = streamCost!;
    streamBytes += cost.bytes;
    if (i === 0) streamFirst = ms;
    else streamMoves.push(ms);
    say(
      `  ${move.label.padEnd(28)} ${ms.toFixed(0).padStart(6)} ms · ${cost.tiles}/${cost.ofTiles} tiles · ` +
        `${(cost.bytes / 1024).toFixed(0)} kB · ${slice.marks.toLocaleString()} marks + ` +
        `${(slice.positions.length / 2 - slice.marks).toLocaleString()} anchors · ` +
        `${(slice.links.length / 2).toLocaleString()} links`,
    );
  }
  say('');

  // ---- the baseline ----
  let wholeCost: WholeCost | null = null;
  const baseline = wholeSource({
    corpus,
    boxes,
    query: duck.query,
    slots: 8,
    onCost: (cost) => {
      wholeCost = cost;
    },
  });

  say('load everything — the whole payload once, then the renderer does the culling');
  const before = heap();
  const wholeMoves: number[] = [];
  let wholeFirst = 0;
  for (const [i, move] of [{ label: 'first paint (the load)', f: 1, cx: 0.5, cy: 0.5 }, ...MOVES].entries()) {
    const rect = move.f === 1 ? extent : windowIn(extent, move.f, move.cx, move.cy);
    const view = viewOf(rect);
    const perPixel = (view.xMax - view.xMin) / PIXELS;
    const started = performance.now();
    const slice = await baseline.slice({ view, perPixel, limit: 20000, fill: 'cluster_id' });
    const ms = performance.now() - started;
    const cost = wholeCost!;
    if (i === 0) wholeFirst = ms;
    else wholeMoves.push(ms);
    say(
      `  ${move.label.padEnd(28)} ${ms.toFixed(1).padStart(6)} ms · ${cost.queries} queries · ` +
        `${(cost.bytes / 1024).toFixed(0)} kB · ${slice.marks.toLocaleString()} marks · ` +
        `${(slice.links.length / 2).toLocaleString()} links${cost.failure ? ` · FAILURE: ${cost.failure}` : ''}`,
    );
  }
  const after = heap();
  say('');

  const w = wholeCost!;
  const mean = (values: number[]) => values.reduce((a, b) => a + b, 0) / Math.max(1, values.length);
  say('the four numbers');
  say(`  bytes read        streaming ${MB(streamBytes)} over 6 answers   ·  whole ${MB(w.bytes)} once`);
  say(`  rows held         streaming one window at a time            ·  whole ${w.rows.toLocaleString()} + ${w.links.toLocaleString()} links = ${MB(w.held)}`);
  say(`  first paint       streaming ${streamFirst.toFixed(0)} ms                        ·  whole ${wholeFirst.toFixed(0)} ms`);
  say(
    `  per camera move   streaming ${mean(streamMoves).toFixed(0)} ms mean of ${streamMoves.length}              ` +
      `·  whole ${mean(wholeMoves).toFixed(3)} ms mean of ${wholeMoves.length}`,
  );
  if (before !== null && after !== null) {
    say('');
    say(`  JS heap ${MB(before)} → ${MB(after)} across the load (Chrome's usedJSHeapSize; not a spec)`);
  }
  if (w.failure) say(`\n  the baseline reports: ${w.failure}`);
}

main().catch((cause: unknown) => {
  say(`\nfailed: ${String(cause)}`);
});
