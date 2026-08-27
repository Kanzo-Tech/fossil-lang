/**
 * Generate the large corpus the streaming panel reads, into this app's own `public/`.
 *
 * ## Why a build step and not a fetch
 *
 * The playground's claim is that nothing leaves the machine. A demo of "the browser streams
 * the tiles a question touches" needs a corpus large enough for touching a subset to mean
 * something, and there are three ways to get one:
 *
 *   - **Fetch it from somewhere.** Breaks the claim outright, and there is no somewhere.
 *   - **Generate it in the tab.** `guards/fixture.mjs` drives the `duckdb` CLI over stdin. The
 *     browser has no CLI, and porting the generator to DuckDB-WASM would make the demo's data
 *     a second implementation of the fixture — the one thing `conformance/provenance.mjs`
 *     exists to prevent.
 *   - **Generate it at build time, serve it from this app's origin.** What this does.
 *
 * The third is the only one that keeps the corpus BYTE-IDENTICAL to what the guards check,
 * because it calls `write()` — the same function, not a copy of it. `apps/corpus/guards/` has
 * no npm dependencies by design, so importing across the workspace costs nothing but a path.
 *
 * ## What it costs
 *
 * Measured on this machine (DuckDB 1.5.3, Node 23, M-series):
 *
 * | vertices | edges | tiles | wall | on disk |
 * |---|---|---|---|---|
 * | 200,000 | 400,000 | 49 | 0.6 s | 11.3 MB |
 * | 1,000,000 | 1,999,993 | 245 | 3.2 s | 57.3 MB |
 *
 * A million is the default because it is the size `/docs/format/conventions/addressing`
 * measures against, so its numbers are comparable: 245 tiles, and a vertex payload of one
 * `tiles.parquet` carrying 245 row groups.
 *
 * The output is gitignored. It is a build artifact of a private app — 57 MB has no business
 * in a repository, and regenerating it is four seconds.
 *
 * ## Container
 *
 * `rowgroups`, which is what fossil's writer emits: ONE `tiles.parquet` whose row group `k`
 * IS tile `k`. That is the container the streaming panel needs, because it is the one where a
 * run of consecutive tiles collapses into a single HTTP `Range` request. Written as `files`,
 * the same graph would answer identically and cost 4× the requests — measured in the
 * addressing page, and the reason the writer moved.
 *
 * ## Usage
 *
 *     node scripts/bench-corpus.mjs                  # 1,000,000 into public/bench/1000000
 *     node scripts/bench-corpus.mjs --vertices 200000
 *     node scripts/bench-corpus.mjs --force          # regenerate even if present
 *
 * Idempotent by default: an existing corpus of the right size is left alone, so `pnpm dev`
 * does not pay four seconds on every start.
 */
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { write } from '../../corpus/guards/fixture.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
};

const count = Number(flag('vertices', 1_000_000));
const clusters = Number(flag('clusters', 128));
const force = process.argv.includes('--force');

if (!Number.isInteger(count) || count <= 0) {
  console.error(`--vertices must be a positive integer, got ${flag('vertices', '')}`);
  process.exit(2);
}

const dir = resolve(app, 'public/bench', String(count));
// The panel reads this to know what is on disk without probing for it, and the app treats a
// missing file as "no bench corpus built" rather than as an error — `pnpm dev` has to work in
// a checkout where this script has not run.
const stamp = resolve(app, 'public/bench/index.json');

const already = existsSync(stamp) ? JSON.parse(readFileSync(stamp, 'utf8')) : null;
if (!force && already?.count === count && existsSync(resolve(dir, 'graph.graph.yml'))) {
  console.log(`bench corpus already at public/bench/${count} (${already.tiles} tiles) — --force to rebuild`);
  process.exit(0);
}

const started = Date.now();
const report = write(dir, { count, clusters, layout: 'rowgroups' });
const ms = Date.now() - started;

/**
 * Every file the corpus is made of, with its real size.
 *
 * The streaming panel reports "this question cost N% of the corpus", and that denominator has
 * to be a measurement or the percentage is decoration. Recording it here is free — the files
 * are on this disk — and it means the panel never issues a request merely to size something it
 * is not going to read.
 */
const walk = (at, root) =>
  readdirSync(at, { withFileTypes: true }).flatMap((entry) => {
    const full = join(at, entry.name);
    return entry.isDirectory() ? walk(full, root) : [{ path: relative(root, full), bytes: statSync(full).size }];
  });
const files = walk(dir, dir).sort((a, b) => b.bytes - a.bytes);
const bytes = files.reduce((a, f) => a + f.bytes, 0);

mkdirSync(dirname(stamp), { recursive: true });
writeFileSync(stamp, `${JSON.stringify({ ...report, dir: `bench/${count}`, ms, bytes, files }, null, 2)}\n`);

console.log(
  `bench corpus: ${report.count} vertices · ${report.edges} edges · ${report.tiles} tiles · ` +
    `${(bytes / 1024 / 1024).toFixed(1)} MB · ${ms} ms → public/bench/${count}`,
);
