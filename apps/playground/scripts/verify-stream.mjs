/**
 * Check the streaming arithmetic against bytes, over a real HTTP server, in Node.
 *
 * `src/stream.ts` computes which tiles a rectangle touches, collapses them into byte
 * intervals, and asks for each with one `Range` request. Every one of those steps can be
 * self-consistently wrong: a run can name an interval that does not hold the tiles it claims,
 * a selection can miss vertices that are inside the window, and a server can ignore `Range`
 * and make the whole thing look like it worked. So none of it is checked against itself.
 *
 * What this asserts, against the corpus `scripts/bench-corpus.mjs` generates:
 *
 *   1. The footer's tiles tile the file — contiguous, ascending, covering every row.
 *   2. `runsOf` produces intervals that are disjoint and, concatenated, hold exactly the
 *      selected tiles' bytes. No byte is fetched twice; none is skipped.
 *   3. The tiles a rectangle selects CONTAIN every vertex inside that rectangle. Checked by
 *      asking DuckDB for the vertices directly and confirming their `dense_id >> 12` is a
 *      subset of the selection. This is the one that catches a wrong box.
 *   4. The server answers `206` and sends exactly the requested bytes.
 *   5. A question costs a small fraction of the corpus — the point of the exercise.
 *
 * Run: `node scripts/verify-stream.mjs [--vertices 200000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { createReadStream, existsSync, readFileSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { query } from '../../corpus/guards/duck.mjs';
import { FOOTER_SQL, extentOf, fetchRuns, runsOf, selectTiles, toTileBox, windowIn } from '../src/stream.ts';

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

let failures = 0;
const ok = (label, condition, detail = '') => {
  if (condition) console.log(`  ok   ${label}${detail ? ` — ${detail}` : ''}`);
  else {
    console.log(`  FAIL ${label}${detail ? ` — ${detail}` : ''}`);
    failures++;
  }
};

// ---- a static server that honours Range, because that is half of what is under test ----
const server = createServer((req, res) => {
  const file = join(root, decodeURIComponent(new URL(req.url, 'http://x').pathname));
  if (!file.startsWith(root) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  const size = statSync(file).size;
  const range = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? '');
  if (!range) {
    res.writeHead(200, { 'content-length': size }).end(readFileSync(file));
    return;
  }
  const [, a, b] = range;
  const start = Number(a);
  const end = Math.min(Number(b), size - 1);
  res.writeHead(206, {
    'content-range': `bytes ${start}-${end}/${size}`,
    'content-length': end - start + 1,
  });
  createReadStream(file, { start, end }).pipe(res);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}`;
const url = `${base}/vertex/Person/tiles.parquet`;

try {
  console.log(`corpus: ${count} vertices · ${stamp.tiles} tiles · ${stamp.layout}`);

  // ---- 1. the footer tiles the file ----
  const boxes = query(FOOTER_SQL(payload)).map(toTileBox);
  const fileBytes = statSync(payload).size;
  const corpusBytes = ['vertex/Person/tiles.parquet', 'vertex/Person/index/tiles.parquet',
    'edge/Person_knows_Person/by_source/tiles.parquet', 'edge/Person_knows_Person/by_target/tiles.parquet']
    .reduce((a, p) => a + statSync(join(root, p)).size, 0);

  console.log('\nfooter');
  ok('one row group per tile', boxes.length === stamp.tiles, `${boxes.length} of ${stamp.tiles}`);
  ok('rows sum to the vertex count', boxes.reduce((a, b) => a + b.rows, 0) === count);
  let gaps = 0;
  for (let i = 1; i < boxes.length; i++) {
    if (boxes[i].start !== boxes[i - 1].start + boxes[i - 1].bytes) gaps++;
  }
  ok('row groups are contiguous', gaps === 0, `${gaps} gaps in ${boxes.length - 1} boundaries`);
  const footerBytes = fileBytes - boxes.reduce((a, b) => a + b.bytes, 0);
  console.log(`  ..   footer is ${footerBytes} B of ${fileBytes} B — bought once, then every window is free of it`);

  // ---- 2/3/4/5. windows ----
  const extent = extentOf(boxes);
  console.log(`\nextent  x[${extent.xlo.toFixed(0)}, ${extent.xhi.toFixed(0)}]  y[${extent.ylo.toFixed(0)}, ${extent.yhi.toFixed(0)}]`);

  for (const fraction of [0.02, 0.1, 0.3]) {
    const rect = windowIn(extent, fraction);
    const chosen = selectTiles(boxes, rect);
    const runs = runsOf(chosen);
    console.log(`\nwindow ${(fraction * 100).toFixed(0)}% of each axis`);

    // 2. runs are disjoint, and cover exactly the chosen tiles' bytes
    const sorted = [...runs].sort((a, b) => a.start - b.start);
    let overlaps = 0;
    for (let i = 1; i < sorted.length; i++) {
      if (sorted[i].start < sorted[i - 1].start + sorted[i - 1].bytes) overlaps++;
    }
    ok('runs are disjoint', overlaps === 0, `${overlaps} overlapping`);
    ok(
      'runs cover exactly the selected tiles',
      runs.reduce((a, r) => a + r.bytes, 0) === chosen.reduce((a, t) => a + t.bytes, 0),
    );
    ok('runs hold only selected tiles', runs.every((r) => r.last - r.first + 1 ===
      chosen.filter((t) => t.tile >= r.first && t.tile <= r.last).length));

    // 3. the selection contains every vertex inside the rectangle
    const inside = query(`
      SELECT DISTINCT dense_id >> 12 AS tile FROM read_parquet('${payload.replace(/'/g, "''")}')
      WHERE x BETWEEN ${rect.xlo} AND ${rect.xhi} AND y BETWEEN ${rect.ylo} AND ${rect.yhi}
    `).map((r) => Number(r.tile));
    const selected = new Set(chosen.map((t) => t.tile));
    const missed = inside.filter((t) => !selected.has(t));
    ok('selection contains every matching vertex', missed.length === 0, `${missed.length} tiles missed`);
    if (inside.length > 0) {
      console.log(`  ..   over-read ${(chosen.length / inside.length).toFixed(2)}× at tile granularity (${chosen.length} fetched, ${inside.length} needed)`);
    }

    // 4/5. the bytes
    const cost = await fetchRuns(url, runs, boxes.length, chosen.length, fetch);
    ok('server honoured Range', cost.ranged);
    ok('got exactly the bytes asked for', cost.gotBytes === cost.askedBytes, `${cost.gotBytes} vs ${cost.askedBytes}`);
    ok('a question costs less than the corpus', cost.gotBytes < corpusBytes,
      `${(cost.gotBytes / 1024).toFixed(0)} kB of ${(corpusBytes / 1024 / 1024).toFixed(1)} MB`);
    console.log(
      `  ..   ${cost.tiles}/${cost.ofTiles} tiles · ${cost.requests} requests · ` +
      `${(cost.gotBytes / 1024).toFixed(0)} kB = ${((cost.gotBytes / corpusBytes) * 100).toFixed(2)}% of the corpus · ${cost.ms.toFixed(0)} ms`,
    );
  }
} finally {
  server.close();
}

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
