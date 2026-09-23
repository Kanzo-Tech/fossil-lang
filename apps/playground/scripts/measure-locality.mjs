/**
 * **What a neighbourhood costs, and what a graph-aware `dense_id` would change** — one corpus,
 * two hypothetical orders, arithmetic only.
 *
 * A fossil corpus ranks its vertices by the Morton code of their layout position and lets that
 * rank BE `dense_id`. A tile is a fixed run of that rank, so disk locality follows SPATIAL
 * locality and a camera rectangle costs few range requests. Graph locality follows nothing:
 * `cluster_id` is Louvain output used as a colour, the neighbours of a vertex are wherever the
 * plane put them, and incoming edges are served by a second copy of every edge (`by_target`).
 *
 * This asks what would change if `dense_id` were the rank in `(cluster_id, morton-within-cluster)`
 * instead. It answers with four tables: the neighbour cost under each order at one and two hops,
 * the camera cost under each order — which is the thing the candidate trades away — and how much
 * of a vertex's incoming half a `by_source`-only reader could recover.
 *
 * ## What this is NOT
 *
 * - **Not a benchmark, and not a latency.** Nothing here is timed and nothing here is fetched. A
 *   tile is a row group, a run is a maximal stretch of adjacent row groups — one HTTP `Range`
 *   request each — and bytes are the row-group sizes Parquet already wrote. A range request is not
 *   a millisecond: it hides RTT, concurrency, the reader's own decoding, and every cache between.
 * - **Not a comparison with a triplestore.** No GraphDB, no Ontotext, no server of any kind was
 *   measured or estimated here. This produces fossil's side of that comparison and stops.
 * - **Not a writer.** No corpus is generated, no layout pass runs, nothing is reordered on disk. A
 *   hypothetical order is a `row_number()`; the tile a rank falls in is `rank >> 12`.
 * - **Not a statement about multi-type corpora.** This corpus has ONE vertex type and ONE relation.
 *   It can say nothing about shapes, predicate grouping, or a per-relation order.
 *
 * ## What the fixture is, which bounds every number below
 *
 * `apps/corpus/guards/fixture.mjs` builds this corpus, and it is synthetic in three ways the
 * reader must know before believing any ratio here. The script re-derives all three at runtime and
 * prints them as Table 0, so they cannot silently stop being true:
 *
 *   1. **Degree is a constant.** Every vertex has out-degree 2 (a ring successor and one chord) and
 *      in-degree ~2. There is no degree distribution to sample across, so "a sample spanning
 *      degrees" is not available here — the sample spans CLUSTERS and rank space instead.
 *   2. **The communities are the generator's, not a measurement.** Vertices are assigned to
 *      `count/clusters` blocks by pre-layout index and the chord edge is drawn INSIDE the block, so
 *      almost every edge is intra-cluster by construction. Real Louvain output on a real graph will
 *      not be this clean, and the candidate order's win here is an upper bound, not a forecast.
 *   3. **The clusters are not spatially separated.** The discs are packed at radius
 *      `12*sqrt(k)` on a grid of spacing 100, so at a million vertices each disc is wider than the
 *      whole grid and all 128 overlap. That is why the camera table looks the way it does.
 *
 * ## Method
 *
 * Everything is one DuckDB database built from the corpus's own Parquet. Two orders:
 *
 *   - `m` — **Morton**, what is on disk: `rank(v) = dense_id(v)`.
 *   - `c` — **candidate**: `rank(v) = row_number() OVER (ORDER BY cluster_id, dense_id) - 1`, which
 *     is `(cluster, then Morton within the cluster)` because `dense_id` already IS Morton rank.
 *
 * Under either order a vertex tile is `rank >> 12` and an edge projection is re-sorted by the
 * ranks of its aligned endpoint, so an edge tile is `edge_position >> 12`. `tiles` counts distinct
 * row groups; `runs` counts maximal stretches of adjacent ones, which is the request count.
 *
 * ## Usage
 *
 *     node scripts/measure-locality.mjs [--corpus <dir>] [--sample-per-cluster 4] [--keep-db]
 *
 * Defaults to `public/bench/1000000`, which `scripts/bench-corpus.mjs` writes. Read-only: the
 * corpus directory is never opened for writing. Needs the `duckdb` binary on PATH and nothing else.
 */
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { lit } from '../../corpus/guards/duck.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
};

const corpus = resolve(flag('corpus', join(app, 'public/bench/1000000')));
const perCluster = Number(flag('sample-per-cluster', 4));
const keepDb = process.argv.includes('--keep-db');

if (!existsSync(corpus)) {
  console.error(`no corpus at ${corpus} — run scripts/bench-corpus.mjs first`);
  process.exit(2);
}

const VERTEX = join(corpus, 'vertex/Person/tiles.parquet');
const BY_SOURCE = join(corpus, 'edge/Person_knows_Person/by_source/tiles.parquet');
const BY_TARGET = join(corpus, 'edge/Person_knows_Person/by_target/tiles.parquet');
for (const f of [VERTEX, BY_SOURCE, BY_TARGET]) {
  if (!existsSync(f)) {
    console.error(`corpus is missing ${f}`);
    process.exit(2);
  }
}

// `guards/duck.mjs` spawns a fresh in-memory DuckDB per statement, which is right for a guard and
// wrong here: this measurement derives a dozen tables and every later query reads them. So the
// same contract — statements over stdin, never `-c`, so no corpus path meets a shell — against a
// database file that persists between calls. `lit` is imported rather than restated.
const work = mkdtempSync(join(tmpdir(), 'fossil-locality-'));
const db = join(work, 'locality.duckdb');

function duck(sql, json) {
  const args = json ? [db, '-json', '-noheader', '-batch'] : [db, '-batch'];
  const run = spawnSync('duckdb', args, {
    input: `${sql};`,
    encoding: 'utf8',
    maxBuffer: 512 * 1024 * 1024,
  });
  if (run.error && run.error.code === 'ENOENT') {
    throw new Error('the `duckdb` binary is not on PATH; see https://duckdb.org/docs/installation.');
  }
  // Any other `run.error` is a process that never started either, and it leaves `status` and
  // `stderr` null — so `run.stderr.trim()` below would report a TypeError from this file rather
  // than the reason. `apps/corpus/guards/duck.mjs` has the same two lines for the same reason.
  if (run.error) {
    throw new Error(`duckdb could not be run: ${run.error.message}\n--- sql ---\n${sql}`, {
      cause: run.error,
    });
  }
  if (run.status !== 0) {
    throw new Error(`duckdb exited ${run.status}\n${(run.stderr ?? '').trim()}\n--- sql ---\n${sql}`);
  }
  if (!json) return null;
  const out = run.stdout.trim();
  if (out === '') return [];
  return JSON.parse(out);
}

const build = (sql) => duck(sql, false);
const ask = (sql) => duck(sql, true);
const one = (sql) => {
  const rows = ask(sql);
  if (rows.length !== 1) throw new Error(`expected one row, got ${rows.length}\n${sql}`);
  return rows[0];
};

const TILE_SHIFT = 12; // chunk_size 4096 — asserted against the manifest below.

// ---------------------------------------------------------------------------------------------
// Build

process.stderr.write('building…\n');

build(`
CREATE OR REPLACE TABLE v AS
  SELECT dense_id::BIGINT AS id, cluster_id::BIGINT AS cl, x::DOUBLE AS x, y::DOUBLE AS y
  FROM read_parquet('${lit(VERTEX)}');

CREATE OR REPLACE TABLE e AS
  SELECT src_dense::BIGINT AS s, dst_dense::BIGINT AS d
  FROM read_parquet('${lit(BY_SOURCE)}');

-- The two orders. \`m\` is what is on disk; \`c\` is the candidate. Nothing else in this script
-- knows which is which — every metric below is computed twice from these two columns.
CREATE OR REPLACE TABLE rk AS
  SELECT id, cl, x, y,
         id AS m,
         (row_number() OVER (ORDER BY cl, id)) - 1 AS c
  FROM v;

-- Edge positions in each projection under each order. A projection is sorted by its aligned
-- endpoint's rank, then the other end's — which is what the writer does with \`dense_id\`.
CREATE OR REPLACE TABLE ep AS
  SELECT e.s, e.d,
         (row_number() OVER (ORDER BY rs.m, rd.m)) - 1 AS ps_m,
         (row_number() OVER (ORDER BY rd.m, rs.m)) - 1 AS pt_m,
         (row_number() OVER (ORDER BY rs.c, rd.c)) - 1 AS ps_c,
         (row_number() OVER (ORDER BY rd.c, rs.c)) - 1 AS pt_c
  FROM e JOIN rk rs ON rs.id = e.s JOIN rk rd ON rd.id = e.d;

-- Where a vertex's own rows sit in each projection. Contiguous by construction: the projection is
-- sorted by that endpoint, so one vertex is one range and therefore one request.
CREATE OR REPLACE TABLE srng AS
  SELECT s AS id, min(ps_m) AS lo_m, max(ps_m) AS hi_m, min(ps_c) AS lo_c, max(ps_c) AS hi_c
  FROM ep GROUP BY s;
CREATE OR REPLACE TABLE trng AS
  SELECT d AS id, min(pt_m) AS lo_m, max(pt_m) AS hi_m, min(pt_c) AS lo_c, max(pt_c) AS hi_c
  FROM ep GROUP BY d;

CREATE OR REPLACE TABLE adj AS
  SELECT DISTINCT a, b FROM (SELECT s AS a, d AS b FROM e UNION ALL SELECT d AS a, s AS b FROM e)
  WHERE a <> b;

-- The sample. Degree cannot be spanned (see the header, and Table 0 proves it), so it spans
-- clusters and rank space instead: \`perCluster\` vertices from every cluster, at fixed fractions
-- of each cluster's own Morton order. Deterministic — no RNG, so a re-run reproduces the numbers.
CREATE OR REPLACE TABLE smp AS
  WITH r AS (
    SELECT id, cl,
           row_number() OVER (PARTITION BY cl ORDER BY id) AS rn,
           count(*) OVER (PARTITION BY cl) AS n
    FROM v
  )
  SELECT id AS v FROM r
  WHERE rn IN (SELECT DISTINCT greatest(1, ((n * g) / ${perCluster})::BIGINT) FROM (SELECT unnest(range(0, ${perCluster})) AS g));

CREATE OR REPLACE TABLE h1 AS
  SELECT DISTINCT smp.v, adj.b AS u FROM smp JOIN adj ON adj.a = smp.v WHERE adj.b <> smp.v;

CREATE OR REPLACE TABLE h2 AS
  SELECT DISTINCT v, u FROM (
    SELECT v, u FROM h1
    UNION ALL
    SELECT h1.v, adj.b AS u FROM h1 JOIN adj ON adj.a = h1.u
  ) WHERE u <> v;
`);

// ---------------------------------------------------------------------------------------------
// Sanity: the manifest's own numbers, re-derived. If these drift the tables below are about a
// different corpus and every ratio in the report is stale.

const facts = one(`
  SELECT (SELECT count(*) FROM v) AS vertices,
         (SELECT count(*) FROM e) AS edges,
         (SELECT count(DISTINCT cl) FROM v) AS clusters,
         (SELECT max(id) + 1 FROM v) AS dense_span,
         (((SELECT count(*) FROM v) + ${1 << TILE_SHIFT} - 1) // ${1 << TILE_SHIFT})::BIGINT AS vertex_tiles,
         (((SELECT count(*) FROM e) + ${1 << TILE_SHIFT} - 1) // ${1 << TILE_SHIFT})::BIGINT AS edge_tiles,
         (SELECT count(*) FROM smp) AS sampled
`);

const rowGroups = (path) =>
  one(`SELECT count(*) AS groups, round(avg(b))::BIGINT AS mean_bytes, sum(b)::BIGINT AS total_bytes
       FROM (SELECT row_group_id, sum(total_compressed_size) AS b
             FROM parquet_metadata('${lit(path)}') GROUP BY row_group_id)`);

const rgVertex = rowGroups(VERTEX);
const rgSource = rowGroups(BY_SOURCE);
const rgTarget = rowGroups(BY_TARGET);

const shape = one(`
  SELECT round(avg(deg), 3) AS mean_deg, min(deg) AS min_deg, max(deg) AS max_deg,
         count(DISTINCT deg) AS distinct_degrees
  FROM (SELECT a AS id, count(*) AS deg FROM adj GROUP BY a)
`);

const clusterShape = one(`
  SELECT min(n) AS min_size, max(n) AS max_size, round(avg(n), 1) AS mean_size,
         round(avg(n) / ${1 << TILE_SHIFT}, 3) AS mean_tiles_per_cluster
  FROM (SELECT cl, count(*) AS n FROM v GROUP BY cl)
`);

const edgeShape = one(`
  SELECT count(*) AS total,
         sum(CASE WHEN a.cl = b.cl THEN 1 ELSE 0 END) AS intra_cluster,
         round(100.0 * sum(CASE WHEN a.cl = b.cl THEN 1 ELSE 0 END) / count(*), 3) AS pct_intra,
         round(avg(abs(e.d - e.s))) AS mean_dense_id_gap
  FROM e JOIN v a ON a.id = e.s JOIN v b ON b.id = e.d
`);

// Whether the communities are spatially separated at all. If a cluster's disc is as wide as the
// whole corpus, "cluster order" and "spatial order" are not two views of one thing.
const overlap = one(`
  WITH ext AS (SELECT max(x) - min(x) AS w, max(y) - min(y) AS h FROM v),
       disc AS (SELECT cl, max(x) - min(x) AS w, max(y) - min(y) AS h FROM v GROUP BY cl)
  SELECT round(avg(disc.w / ext.w), 3) AS mean_disc_width_over_extent,
         round(max(disc.w / ext.w), 3) AS max_disc_width_over_extent
  FROM disc, ext
`);

// ---------------------------------------------------------------------------------------------
// Metrics

/** Distinct tiles and maximal adjacent runs of `rank >> 12`, per query key, over a (v,u) table. */
const tilesAndRuns = (table, order) => `
  SELECT v,
         count(*) AS tiles,
         sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
  FROM (
    SELECT v, tile, lag(tile) OVER (PARTITION BY v ORDER BY tile) AS pv
    FROM (SELECT DISTINCT h.v, (rk.${order} >> ${TILE_SHIFT}) AS tile
          FROM ${table} h JOIN rk ON rk.id = h.u)
  ) GROUP BY v`;

/** Mean / median / p95 / max of one column of a per-vertex table. */
const spread = (inner, col) =>
  one(`SELECT round(avg(${col}), 2) AS mean,
              median(${col}) AS median,
              quantile_cont(${col}, 0.95) AS p95,
              max(${col}) AS max
       FROM (${inner})`);

/** Edge tiles a set of vertices' rows occupy in one projection, from their contiguous ranges. */
const edgeTiles = (table, rng, lo, hi) => `
  SELECT v,
         count(*) AS tiles,
         sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
  FROM (
    SELECT v, tile, lag(tile) OVER (PARTITION BY v ORDER BY tile) AS pv
    FROM (SELECT DISTINCT h.v,
                 unnest(range(r.${lo} >> ${TILE_SHIFT}, (r.${hi} >> ${TILE_SHIFT}) + 1)) AS tile
          FROM ${table} h JOIN ${rng} r ON r.id = h.u)
  ) GROUP BY v`;

process.stderr.write('measuring neighbourhoods…\n');

// The frontier a two-hop expansion must open edge rows for: v itself plus its one-hop neighbours.
build(`CREATE OR REPLACE TABLE frontier AS SELECT v, v AS u FROM smp UNION SELECT v, u FROM h1;`);
// And what a one-hop query opens: v's own rows only.
build(`CREATE OR REPLACE TABLE self1 AS SELECT v, v AS u FROM smp;`);

const orders = { Morton: 'm', Candidate: 'c' };
const neighbourRows = [];
for (const [label, ord] of Object.entries(orders)) {
  for (const [hop, table, frontierTable] of [
    ['1-hop', 'h1', 'self1'],
    ['2-hop', 'h2', 'frontier'],
  ]) {
    const vt = tilesAndRuns(table, ord);
    const es = edgeTiles(frontierTable, 'srng', `lo_${ord}`, `hi_${ord}`);
    const et = edgeTiles(frontierTable, 'trng', `lo_${ord}`, `hi_${ord}`);
    const size = spread(`SELECT v, count(*) AS n FROM ${table} GROUP BY v`, 'n');
    neighbourRows.push({
      order: label,
      hop,
      size,
      vertexTiles: spread(vt, 'tiles'),
      vertexRuns: spread(vt, 'runs'),
      sourceRuns: spread(es, 'runs'),
      targetRuns: spread(et, 'runs'),
      // Bytes a reader pulls, from the row-group sizes Parquet actually wrote. An approximation
      // for the candidate: reordering changes what compresses next to what, so a re-sorted tile
      // would not weigh exactly what the Morton one weighs. Mean tile size is the stand-in.
      meanBytes: spread(
        `SELECT a.v,
                a.tiles * ${Number(rgVertex.mean_bytes)}
              + b.tiles * ${Number(rgSource.mean_bytes)}
              + c.tiles * ${Number(rgTarget.mean_bytes)} AS bytes
         FROM (${vt}) a JOIN (${es}) b ON b.v = a.v JOIN (${et}) c ON c.v = a.v`,
        'bytes',
      ),
      // What the whole query costs a reader in requests: edge runs on both projections, then the
      // vertex runs to materialise the answer.
      totalRuns: spread(
        `SELECT a.v, a.runs + b.runs + c.runs AS runs
         FROM (${vt}) a JOIN (${es}) b ON b.v = a.v JOIN (${et}) c ON c.v = a.v`,
        'runs',
      ),
    });
  }
}

// ---------------------------------------------------------------------------------------------
// Camera — what the candidate trades away.

process.stderr.write('measuring camera windows…\n');

build(`
CREATE OR REPLACE TABLE win AS
  WITH ext AS (SELECT min(x) AS x0, max(x) AS x1, min(y) AS y0, max(y) AS y1 FROM v),
       f AS (SELECT unnest([0.01, 0.05, 0.10, 0.25, 0.50]) AS frac),
       g AS (SELECT unnest([0, 1, 2]) AS gx),
       h AS (SELECT unnest([0, 1, 2]) AS gy)
  SELECT row_number() OVER (ORDER BY frac, gx, gy) AS wid, frac, gx, gy,
         greatest(x0, x0 + (x1 - x0) * (gx + 0.5) / 3 - (x1 - x0) * frac / 2) AS minx,
         least(x1,  x0 + (x1 - x0) * (gx + 0.5) / 3 + (x1 - x0) * frac / 2) AS maxx,
         greatest(y0, y0 + (y1 - y0) * (gy + 0.5) / 3 - (y1 - y0) * frac / 2) AS miny,
         least(y1,  y0 + (y1 - y0) * (gy + 0.5) / 3 + (y1 - y0) * frac / 2) AS maxy
  FROM ext, f, g, h;

CREATE OR REPLACE TABLE inwin AS
  SELECT w.wid, w.frac, rk.id, rk.m, rk.c
  FROM win w JOIN rk ON rk.x BETWEEN w.minx AND w.maxx AND rk.y BETWEEN w.miny AND w.maxy;
`);

const cameraRows = ask(`
  WITH per AS (
    SELECT frac, wid, 'Morton' AS ord, (m >> ${TILE_SHIFT}) AS tile FROM inwin
    UNION ALL
    SELECT frac, wid, 'Candidate', (c >> ${TILE_SHIFT}) FROM inwin
  ),
  d AS (SELECT DISTINCT frac, wid, ord, tile FROM per),
  w AS (SELECT frac, wid, ord, tile, lag(tile) OVER (PARTITION BY wid, ord ORDER BY tile) AS pv FROM d),
  agg AS (
    SELECT frac, wid, ord, count(*) AS tiles,
           sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
    FROM w GROUP BY frac, wid, ord
  ),
  pop AS (SELECT frac, wid, count(*) AS marks FROM inwin GROUP BY frac, wid)
  SELECT agg.frac, agg.ord,
         round(avg(pop.marks)) AS mean_marks,
         round(avg(agg.tiles), 1) AS mean_tiles,
         max(agg.tiles) AS max_tiles,
         round(avg(agg.runs), 1) AS mean_runs,
         max(agg.runs) AS max_runs
  FROM agg JOIN pop ON pop.wid = agg.wid
  GROUP BY agg.frac, agg.ord ORDER BY agg.frac, agg.ord DESC
`);

// ---------------------------------------------------------------------------------------------
// Whether `by_target` could go: how much of the incoming half a `by_source`-only reader recovers.

process.stderr.write('measuring by_target retirement…\n');

const retire = (ord) =>
  one(`
    WITH open AS (
      -- the by_source tiles the outgoing half of neighbours(v) already opened
      SELECT smp.v, unnest(range(r.lo_${ord} >> ${TILE_SHIFT}, (r.hi_${ord} >> ${TILE_SHIFT}) + 1)) AS tile
      FROM smp JOIN srng r ON r.id = smp.v
    ),
    incoming AS (
      -- where v's INCOMING edges sit in by_source, which is wherever their source's rank put them
      SELECT smp.v, ep.ps_${ord} >> ${TILE_SHIFT} AS tile
      FROM smp JOIN ep ON ep.d = smp.v
    ),
    hit AS (
      SELECT i.v, i.tile, (o.tile IS NOT NULL) AS covered
      FROM incoming i LEFT JOIN open o ON o.v = i.v AND o.tile = i.tile
    ),
    per AS (
      SELECT v,
             count(*) AS in_edges,
             sum(CASE WHEN covered THEN 1 ELSE 0 END) AS covered_edges,
             count(DISTINCT CASE WHEN NOT covered THEN tile END) AS extra_tiles,
             max(tile) - min(tile) + 1 AS band_tiles
      FROM hit GROUP BY v
    ),
    -- What a by_source-ONLY reader would actually fetch: the union of the outgoing tiles and the
    -- incoming ones, as maximal adjacent runs. This is the read that would replace by_target.
    union_tiles AS (SELECT v, tile FROM open UNION SELECT v, tile FROM incoming),
    union_runs AS (
      SELECT v, count(*) AS tiles,
             sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
      FROM (SELECT v, tile, lag(tile) OVER (PARTITION BY v ORDER BY tile) AS pv FROM union_tiles)
      GROUP BY v
    )
    SELECT round(100.0 * sum(per.covered_edges) / sum(per.in_edges), 2) AS pct_edges_already_open,
           round(avg(per.extra_tiles), 2) AS mean_extra_tiles,
           median(per.extra_tiles) AS median_extra_tiles,
           quantile_cont(per.extra_tiles, 0.95) AS p95_extra_tiles,
           max(per.extra_tiles) AS max_extra_tiles,
           round(avg(per.band_tiles), 1) AS mean_band_tiles,
           max(per.band_tiles) AS max_band_tiles,
           round(avg(u.runs), 2) AS mean_bs_only_runs,
           quantile_cont(u.runs, 0.95) AS p95_bs_only_runs,
           max(u.runs) AS max_bs_only_runs,
           round(avg(u.tiles) * ${Number(rgSource.mean_bytes)} / 1024, 1) AS mean_bs_only_kb
    FROM per JOIN union_runs u ON u.v = per.v`);

const retireRows = { Morton: retire('m'), Candidate: retire('c') };

// ---------------------------------------------------------------------------------------------
// Report

const N = (x) => (x === null || x === undefined ? '—' : typeof x === 'number' ? x : Number(x));
const table = (head, rows) => {
  const body = rows.map((r) => `| ${r.join(' | ')} |`);
  return [`| ${head.join(' | ')} |`, `|${head.map(() => '---').join('|')}|`, ...body].join('\n');
};

const bytes = (n) => `${(Number(n) / 1e6).toFixed(1)} MB`;

console.log(`# Locality of a fossil corpus — measured

Corpus: \`${corpus}\`
Vertex tile: ${1 << TILE_SHIFT} rows. Every figure below is produced by this script and nothing else.

## Table 0 — the corpus, and what it cannot test

${table(
  ['fact', 'value'],
  [
    ['vertices', N(facts.vertices)],
    ['edges', N(facts.edges)],
    ['dense_id span', N(facts.dense_span)],
    ['vertex tiles (row groups)', `${N(facts.vertex_tiles)} (parquet says ${N(rgVertex.groups)})`],
    ['edge tiles per projection', `${N(facts.edge_tiles)} (parquet says ${N(rgSource.groups)})`],
    ['clusters', N(facts.clusters)],
    ['cluster size min/mean/max', `${N(clusterShape.min_size)} / ${N(clusterShape.mean_size)} / ${N(clusterShape.max_size)}`],
    ['cluster width in tiles (mean)', N(clusterShape.mean_tiles_per_cluster)],
    ['undirected degree min/mean/max', `${N(shape.min_deg)} / ${N(shape.mean_deg)} / ${N(shape.max_deg)}`],
    ['**distinct degrees in the whole corpus**', `**${N(shape.distinct_degrees)}**`],
    ['edges intra-cluster', `${N(edgeShape.intra_cluster)} of ${N(edgeShape.total)} (**${N(edgeShape.pct_intra)}%**)`],
    ['mean |dense_id(src) - dense_id(dst)|', N(edgeShape.mean_dense_id_gap)],
    ['cluster disc width / corpus extent (mean, max)', `${N(overlap.mean_disc_width_over_extent)}, ${N(overlap.max_disc_width_over_extent)}`],
    ['vertex payload', `${bytes(rgVertex.total_bytes)}, mean tile ${N(rgVertex.mean_bytes)} B`],
    ['by_source', `${bytes(rgSource.total_bytes)}, mean tile ${N(rgSource.mean_bytes)} B`],
    ['by_target', `${bytes(rgTarget.total_bytes)}, mean tile ${N(rgTarget.mean_bytes)} B`],
    ['by_target share of the four base projections', `${((Number(rgTarget.total_bytes) / (Number(rgVertex.total_bytes) + Number(rgSource.total_bytes) + Number(rgTarget.total_bytes))) * 100).toFixed(1)}%`],
    ['sampled vertices', `${N(facts.sampled)} (${perCluster} per cluster, deterministic)`],
  ],
)}

Two of these bound everything that follows. **Distinct degrees = ${N(shape.distinct_degrees)}**: this
corpus has no degree distribution, so no sample can span one and no figure here says what a hub
costs. **${N(edgeShape.pct_intra)}% of edges are intra-cluster**, because the generator draws the
chord inside the block it assigned — so the candidate order's neighbour win below is close to the
best any graph-aware order could do, not a forecast for a real graph.

## Table 1 & 2 — what neighbours(v) costs, per order

\`tiles\` are distinct row groups; \`runs\` are maximal adjacent stretches, i.e. HTTP Range requests.
\`vertex runs\` is the cost of materialising the answer; \`total runs\` adds the edge reads that
found it (by_source + by_target).

${table(
  ['order', 'hop', 'mean |N|', 'vertex tiles mean/med/p95/max', 'vertex runs mean/med/p95/max', 'by_source runs mean/max', 'by_target runs mean/max', 'TOTAL runs mean/med/p95/max', 'bytes mean/p95'],
  neighbourRows.map((r) => [
    r.order,
    r.hop,
    N(r.size.mean),
    `${N(r.vertexTiles.mean)} / ${N(r.vertexTiles.median)} / ${N(r.vertexTiles.p95)} / ${N(r.vertexTiles.max)}`,
    `${N(r.vertexRuns.mean)} / ${N(r.vertexRuns.median)} / ${N(r.vertexRuns.p95)} / ${N(r.vertexRuns.max)}`,
    `${N(r.sourceRuns.mean)} / ${N(r.sourceRuns.max)}`,
    `${N(r.targetRuns.mean)} / ${N(r.targetRuns.max)}`,
    `${N(r.totalRuns.mean)} / ${N(r.totalRuns.median)} / ${N(r.totalRuns.p95)} / ${N(r.totalRuns.max)}`,
    `${(Number(r.meanBytes.mean) / 1024).toFixed(0)} / ${(Number(r.meanBytes.p95) / 1024).toFixed(0)} KB`,
  ]),
)}

Byte columns are tiles times the mean row-group size Parquet actually wrote (vertex
${N(rgVertex.mean_bytes)} B, edge ${N(rgSource.mean_bytes)} B). They are an approximation under the
candidate order, which would re-sort what compresses next to what.

## Table 3 — what the candidate costs the camera

A window is a fraction of each side of the corpus extent, placed at each of nine positions on a
3x3 grid and clipped to the extent. Spatial locality is what the candidate trades away and this
is the price.

${table(
  ['window (side frac)', 'order', 'mean marks', 'tiles mean/max', 'runs mean/max'],
  cameraRows.map((r) => [
    `${(Number(r.frac) * 100).toFixed(0)}%`,
    r.ord,
    N(r.mean_marks),
    `${N(r.mean_tiles)} / ${N(r.max_tiles)}`,
    `${N(r.mean_runs)} / ${N(r.max_runs)}`,
  ]),
)}

## Table 4 — whether by_target could go

Under each order, v's incoming edges also exist in \`by_source\` as rows with \`dst = v\`, sitting
wherever their SOURCE's rank put them. \`% already open\` is the share of them landing in a
by_source tile the outgoing half of the same query had already fetched. \`band\` is how wide a
by_source scan would have to be to be sure of catching them all.

${table(
  ['order', '% of in-edges already open', 'extra tiles mean/med/p95/max', 'band tiles mean/max', 'by_source-only runs mean/p95/max', 'by_source-only bytes'],
  Object.entries(retireRows).map(([label, r]) => [
    label,
    `${N(r.pct_edges_already_open)}%`,
    `${N(r.mean_extra_tiles)} / ${N(r.median_extra_tiles)} / ${N(r.p95_extra_tiles)} / ${N(r.max_extra_tiles)}`,
    `${N(r.mean_band_tiles)} / ${N(r.max_band_tiles)}`,
    `${N(r.mean_bs_only_runs)} / ${N(r.p95_bs_only_runs)} / ${N(r.max_bs_only_runs)}`,
    `${N(r.mean_bs_only_kb)} KB`,
  ]),
)}

The last two columns are the whole edge read for \`neighbours(v)\` served from \`by_source\` alone,
with \`by_target\` deleted. Today that read is exactly 2 requests (one range in each projection) for
${bytes(rgSource.mean_bytes * 2)}.

**Those two columns are ORACLE numbers and the \`band\` column is the honest one.** They count the
tiles v's in-edges actually occupy, which a reader learns from \`by_target\` — the file being
retired. Without it a reader must fetch a band it can bound in advance, and that is what separates
the two rows: under the candidate the band is ${N(retireRows.Candidate.mean_band_tiles)} tiles mean
and ${N(retireRows.Candidate.max_band_tiles)} max, so ONE range request covers it; under Morton it
is ${N(retireRows.Morton.mean_band_tiles)} mean and ${N(retireRows.Morton.max_band_tiles)} max out
of ${N(facts.edge_tiles)}, which is a scan. Bounding even the candidate's band needs something the
manifest does not currently carry — a ${N(facts.clusters)}-entry cluster-to-rank-range table.
Naming that cost is not the same as paying it.

---

Not measured here: any triplestore, any latency, any byte actually transferred, any corpus with
more than one vertex type or more than one relation.
`);

if (keepDb) process.stderr.write(`database kept at ${db}\n`);
else rmSync(work, { recursive: true, force: true });
