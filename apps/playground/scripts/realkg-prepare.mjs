/**
 * **Fetch a REAL graph and put it in front of `fossil run`.**
 *
 * Everything fossil's read costs have been measured against so far is
 * `apps/corpus/guards/fixture.mjs`, and that fixture cannot answer the questions
 * that matter. Re-derived, not asserted — `measure-locality.mjs` prints all three
 * as its Table 0:
 *
 *   - **two distinct degrees in a million vertices**, so nothing measured there
 *     says what a HUB costs;
 *   - **99.994% of edges intra-cluster by construction**, because the generator
 *     draws the chord inside the block it assigned;
 *   - every cluster disc wider than the whole extent, so the communities are not
 *     spatially separated and the camera table is measuring one blur.
 *
 * This script gets a graph with a real degree distribution, real communities and
 * **published ground truth** onto disk in the shape `bench/dblp/dblp.fossil` maps.
 *
 * ## The dataset, pinned
 *
 * SNAP `com-DBLP`: the DBLP co-authorship network with ground-truth communities.
 * Chosen over the alternatives because it is the smallest graph that has all four
 * properties the fixture lacks — power-law degree (max 343 against a mean of 6.6),
 * real community structure, a *published* partition to check Louvain against, and
 * a size that fits through `fossil run` on a laptop.
 *
 * Both archives are pinned by URL, byte count and SHA-256 below; a mismatch is a
 * hard failure, because a benchmark whose input silently changed is not one.
 *
 * ## What it writes
 *
 *   `apps/playground/public/bench/realkg/`   the two `.gz` and their expansion
 *   `apps/playground/bench/dblp/data/links.csv`   src,dst — BOTH orientations
 *
 * Both are gitignored. The symmetrisation is not cosmetic: SNAP lists each
 * undirected edge once, and the fossil program mints its vertices from `src`
 * alone, so without both orientations the corpus would be missing every author
 * who never happens to sort first.
 *
 * ## Usage
 *
 *     node scripts/realkg-prepare.mjs [--force]
 *
 * Idempotent: an existing `links.csv` of the right row count is left alone.
 * Needs `curl` and the `duckdb` binary on PATH.
 */
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { lit } from '../../corpus/guards/duck.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

/**
 * The pin. A benchmark's input is part of its result, so the URL, the byte count
 * and the digest live beside the code that consumes them rather than in prose
 * somewhere that cannot fail.
 */
export const DATASET = {
  name: 'SNAP com-DBLP',
  cite: 'J. Yang and J. Leskovec, "Defining and Evaluating Network Communities based on Ground-truth", ICDM 2012',
  files: [
    {
      url: 'https://snap.stanford.edu/data/bigdata/communities/com-dblp.ungraph.txt.gz',
      gz: 'com-dblp.ungraph.txt.gz',
      txt: 'ungraph.txt',
      bytes: 4_138_463,
      sha256: '9eb0bd30312ddd04e2624f7c36c0983a2e99b116f0385be5a7fce6d6170f4cb3',
    },
    {
      url: 'https://snap.stanford.edu/data/bigdata/communities/com-dblp.top5000.cmty.txt.gz',
      gz: 'com-dblp.top5000.cmty.txt.gz',
      txt: 'cmty.txt',
      bytes: 320_139,
      sha256: '6b23675b3c0ef35c7ba2520e967d3dba3a7cec771ae16a2ef6a95426efb30b7a',
    },
  ],
  // SNAP's own header line, which the loader below re-derives and checks against.
  nodes: 317_080,
  edges: 1_049_866,
};

export const RAW_DIR = join(app, 'public/bench/realkg');
export const PROGRAM_DIR = join(app, 'bench/dblp');
export const LINKS_CSV = join(PROGRAM_DIR, 'data/links.csv');

const force = process.argv.includes('--force');

const sh = (cmd, args, opts = {}) => {
  const run = spawnSync(cmd, args, { encoding: 'utf8', stdio: 'pipe', ...opts });
  if (run.error) throw new Error(`${cmd}: ${run.error.message}`);
  if (run.status !== 0) {
    throw new Error(`${cmd} exited ${run.status}\n${(run.stderr || '').trim()}`);
  }
  return run.stdout;
};

/** DuckDB over stdin — never `-c`, so no path this script builds meets a shell. */
const duck = (sql) => sh('duckdb', ['-batch'], { input: `${sql};` });

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

function fetchAndVerify(f) {
  const gz = join(RAW_DIR, f.gz);
  if (!existsSync(gz) || force) {
    process.stderr.write(`fetching ${f.url}\n`);
    sh('curl', ['-sSL', '--fail', '-o', gz, f.url]);
  }
  const size = statSync(gz).size;
  const digest = sha256(gz);
  if (size !== f.bytes || digest !== f.sha256) {
    throw new Error(
      `${f.gz} is not the pinned artefact\n  expected ${f.bytes} B  ${f.sha256}\n  got      ${size} B  ${digest}`,
    );
  }
  const txt = join(RAW_DIR, f.txt);
  if (!existsSync(txt) || force) sh('bash', ['-c', `gunzip -c ${JSON.stringify(gz)} > ${JSON.stringify(txt)}`]);
  return txt;
}

export function prepare() {
  mkdirSync(RAW_DIR, { recursive: true });
  mkdirSync(dirname(LINKS_CSV), { recursive: true });
  const [ungraph] = DATASET.files.map(fetchAndVerify);

  if (existsSync(LINKS_CSV) && !force) {
    process.stderr.write(`links.csv present — ${(statSync(LINKS_CSV).size / 1e6).toFixed(1)} MB (use --force)\n`);
    return { ungraph, links: LINKS_CSV };
  }

  // Symmetrise. SNAP's file is tab-separated behind FOUR banner lines that begin
  // `#` — skipped by count rather than by `comment='#'`, which DuckDB then tries
  // to reconcile with a header it was told does not exist. Every undirected edge
  // appears ONCE, and the fossil program mints vertices from `src`, so both
  // orientations have to exist for every author to get one.
  duck(`
    COPY (
      WITH raw AS (
        SELECT u, v
        FROM read_csv('${lit(ungraph)}', delim='\t', header=false, skip=4,
                      columns={'u':'BIGINT','v':'BIGINT'})
      )
      SELECT u AS src, v AS dst FROM raw
      UNION ALL
      SELECT v AS src, u AS dst FROM raw
      ORDER BY src, dst
    ) TO '${lit(LINKS_CSV)}' (FORMAT CSV, HEADER)`);

  return { ungraph, links: LINKS_CSV };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { links } = prepare();
  const facts = JSON.parse(
    sh('duckdb', ['-json', '-noheader', '-batch'], {
      input: `SELECT count(*) AS rows, count(DISTINCT src) AS authors FROM read_csv('${lit(links)}');`,
    }),
  )[0];
  const rows = Number(facts.rows);
  const authors = Number(facts.authors);
  if (rows !== DATASET.edges * 2 || authors !== DATASET.nodes) {
    throw new Error(
      `symmetrised edge list disagrees with SNAP's own header: ${rows} rows / ${authors} authors, ` +
        `expected ${DATASET.edges * 2} / ${DATASET.nodes}`,
    );
  }
  console.log(
    `${DATASET.name}: ${authors.toLocaleString()} authors, ${DATASET.edges.toLocaleString()} undirected edges\n` +
      `${links} — ${rows.toLocaleString()} directed rows, ${(statSync(links).size / 1e6).toFixed(1)} MB`,
  );
}
