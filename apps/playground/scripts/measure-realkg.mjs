/**
 * **Does `dense_id` order already follow the graph?** — measured on a real graph,
 * with the synthetic fixture beside it.
 *
 * A fossil corpus orders its vertices by the Morton code of their layout position
 * and lets that rank BE `dense_id`. The layout pass runs **Louvain, then Morton**
 * (`fossil-layout/src/layout.rs`), so there is a chain worth checking end to end:
 *
 *     Louvain finds communities -> placement separates them in the plane
 *       -> Morton follows the plane -> does rank order therefore follow the GRAPH?
 *
 * If it does, no second ordering is needed and a large open design question closes
 * for nothing. `measure-locality.mjs` asked the adjacent question — what a
 * `(cluster_id, Morton-within-cluster)` order would buy — and got 4.6x on two-hop
 * neighbours for 10-13x on the camera. It could not ask THIS one, because on the
 * synthetic fixture the communities occupy the same patch of plane: all 128 discs
 * are wider than the whole extent, so "spatially ordered" and "graph ordered"
 * were not two distinguishable things there.
 *
 * ## The five questions, in order
 *
 *   1. **How much graph locality does Morton order already have?** The
 *      distribution of `|dense_id(src) - dense_id(dst)|`, the share of edges whose
 *      ends land in the SAME tile, and the share landing within one run of
 *      adjacent tiles. This is the headline and it is reported for both corpora.
 *   2. **Are Louvain's communities real?** The corpus's `cluster_id` against SNAP's
 *      *published* ground-truth communities — two independent partitions of one
 *      graph. Reported as per-community purity and as the share of ground-truth
 *      internal edges that Louvain also keeps internal. Only the real corpus has
 *      a ground truth; the synthetic fixture's "communities" are its generator's
 *      own block assignment and comparing them to themselves says nothing.
 *   3. **Are the communities separated in the plane?** The failure mode named
 *      above, measured rather than assumed: disc width against corpus extent, and
 *      the share of a cluster's members inside its own convex-ish neighbourhood.
 *   4. **What does `neighbours(v)` cost, ACROSS THE DEGREE DISTRIBUTION?** In
 *      tiles, runs and bytes, at one and two hops, banded by degree, with the
 *      highest-degree vertices reported separately. The old fixture had two
 *      distinct degrees in a million vertices; this one has a power law, so the
 *      hub row is the row that did not exist before.
 *
 *   5. **Is there already an index nobody is reading?** `cluster_id` is a payload
 *      column, so Parquet wrote its min/max into the footer of every row group —
 *      that is, of every TILE. `src/stream.ts::FOOTER_SQL` selects `x` and `y` and
 *      stops. If Morton keeps a tile's cluster range narrow, "which tiles hold
 *      community c" is answerable from bytes the reader already fetched, and the
 *      case for a second on-disk order weakens without anything being built.
 *
 * Then the camera, over the same windows `measure-locality.mjs` uses, so the two
 * scripts' camera tables are readable against each other.
 *
 * ## What this is NOT
 *
 * - **Not a latency, and not a benchmark of a server.** Nothing here is timed and
 *   nothing is fetched over a network. A tile is a Parquet row group; a run is a
 *   maximal stretch of adjacent row groups, i.e. ONE HTTP `Range` request; bytes
 *   are the row-group sizes Parquet already wrote. A range request is not a
 *   millisecond — it hides RTT, concurrency, decode and every cache in between.
 * - **Not a comparison with a triplestore.** No GraphDB, no Ontotext, no server of
 *   any kind is measured, estimated or cited. This produces fossil's side and stops.
 * - **Not a writer, and not a proposal.** No corpus is generated here and nothing
 *   is reordered. `bench/dblp/dblp.fossil` + `fossil run` produce the corpus; this
 *   reads it.
 * - **Not a statement about multi-type corpora.** Both corpora have ONE vertex type
 *   and ONE relation, so nothing here speaks to predicate grouping or per-relation
 *   order.
 * - **Not a claim about Louvain in general.** One graph, one seed, one placement.
 *
 * ## Usage
 *
 *     node scripts/measure-realkg.mjs                        # both corpora
 *     node scripts/measure-realkg.mjs --real <dir> --synthetic <dir>
 *     node scripts/measure-realkg.mjs --no-synthetic
 *     node scripts/measure-realkg.mjs --per-band 120 --hubs 50
 *
 * Every number below comes out of this one command. Needs the `duckdb` binary and
 * a corpus written by `fossil run`; `scripts/realkg-prepare.mjs` fetches the input.
 */
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { lit } from '../../corpus/guards/duck.mjs';
import { DATASET, RAW_DIR } from './realkg-prepare.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
};

const realDir = resolve(flag('real', join(app, 'public/bench/realkg/corpus')));
const synthDir = resolve(flag('synthetic', join(app, 'public/bench/1000000')));
const withSynthetic = !process.argv.includes('--no-synthetic');
const perBand = Number(flag('per-band', 120));
const hubCount = Number(flag('hubs', 50));
const keepDb = process.argv.includes('--keep-db');
const truthTxt = join(RAW_DIR, DATASET.files[1].txt);

const TILE_SHIFT = 12; // chunk_size 4096, the writer's default.
const TILE = 1 << TILE_SHIFT;

// ---------------------------------------------------------------------------------------------
// Corpus discovery. The relation directory is NOT hard-coded: it is named after the
// edge key the program wrote (`Person_coauthor_Person` here, `Person_knows_Person` in
// the fixture), and a script that spells one of them works on exactly one corpus.

function locate(dir) {
  const vertexRoot = join(dir, 'vertex');
  if (!existsSync(vertexRoot)) return null;
  const types = readdirSync(vertexRoot, { withFileTypes: true }).filter((d) => d.isDirectory());
  if (types.length !== 1) return null;
  const vertex = join(vertexRoot, types[0].name, 'tiles.parquet');
  const edgeRoot = join(dir, 'edge');
  const rels = existsSync(edgeRoot)
    ? readdirSync(edgeRoot, { withFileTypes: true }).filter((d) => d.isDirectory())
    : [];
  // The SELF relation — the only one the layout pass reads for adjacency.
  const self = rels.find((r) => {
    const [src, , dst] = [r.name.split('_')[0], null, r.name.split('_').at(-1)];
    return src === types[0].name && dst === types[0].name;
  });
  if (!self) return null;
  const bySource = join(edgeRoot, self.name, 'by_source/tiles.parquet');
  const byTarget = join(edgeRoot, self.name, 'by_target/tiles.parquet');
  if (![vertex, bySource, byTarget].every(existsSync)) return null;
  return { dir, type: types[0].name, relation: self.name, vertex, bySource, byTarget };
}

const real = locate(realDir);
if (!real) {
  console.error(
    `no readable corpus at ${realDir}\n` +
      `  node scripts/realkg-prepare.mjs\n` +
      `  cargo run --release --bin fossil -- run bench/dblp/dblp.fossil --dest file://${realDir}`,
  );
  process.exit(2);
}
const synth = withSynthetic ? locate(synthDir) : null;
if (withSynthetic && !synth) {
  console.error(`no readable corpus at ${synthDir} — run scripts/bench-corpus.mjs, or pass --no-synthetic`);
  process.exit(2);
}

// ---------------------------------------------------------------------------------------------
// One persistent DuckDB, statements over stdin — never `-c`, so no corpus path this
// script builds ever meets a shell. `lit` is imported from the guards rather than
// restated, for the same reason.

const work = mkdtempSync(join(tmpdir(), 'fossil-realkg-'));
const db = join(work, 'realkg.duckdb');

function duck(sql, json) {
  const run = spawnSync('duckdb', json ? [db, '-json', '-noheader', '-batch'] : [db, '-batch'], {
    input: `${sql};`,
    encoding: 'utf8',
    maxBuffer: 1024 * 1024 * 1024,
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
  if (run.status !== 0)
    throw new Error(`duckdb exited ${run.status}\n${(run.stderr ?? '').trim()}\n--- sql ---\n${sql}`);
  if (!json) return null;
  const out = run.stdout.trim();
  return out === '' ? [] : JSON.parse(out);
}
const build = (sql) => duck(sql, false);
const ask = (sql) => duck(sql, true);
const one = (sql) => {
  const rows = ask(sql);
  if (rows.length !== 1) throw new Error(`expected one row, got ${rows.length}\n${sql}`);
  return rows[0];
};

/** Load one corpus under a table prefix. Both corpora carry the same column names. */
function load(p, tag) {
  build(`
    CREATE OR REPLACE TABLE v_${tag} AS
      SELECT dense_id::BIGINT AS id, cluster_id::BIGINT AS cl, x::DOUBLE AS x, y::DOUBLE AS y
      FROM read_parquet('${lit(p.vertex)}');
    CREATE OR REPLACE TABLE e_${tag} AS
      SELECT src_dense::BIGINT AS s, dst_dense::BIGINT AS d
      FROM read_parquet('${lit(p.bySource)}');
    CREATE OR REPLACE TABLE adj_${tag} AS
      SELECT DISTINCT a, b FROM (
        SELECT s AS a, d AS b FROM e_${tag} UNION ALL SELECT d AS a, s AS b FROM e_${tag}
      ) WHERE a <> b;
    CREATE OR REPLACE TABLE deg_${tag} AS
      SELECT a AS id, count(*) AS deg FROM adj_${tag} GROUP BY a;
  `);
}

process.stderr.write('loading corpora…\n');
load(real, 'r');
if (synth) load(synth, 's');

const rowGroups = (path) =>
  one(`SELECT count(*) AS groups, round(avg(b))::BIGINT AS mean_bytes, sum(b)::BIGINT AS total_bytes
       FROM (SELECT row_group_id, sum(total_compressed_size) AS b
             FROM parquet_metadata('${lit(path)}') GROUP BY row_group_id)`);

const rg = {
  r: { v: rowGroups(real.vertex), s: rowGroups(real.bySource), t: rowGroups(real.byTarget) },
  ...(synth ? { s: { v: rowGroups(synth.vertex), s: rowGroups(synth.bySource), t: rowGroups(synth.byTarget) } } : {}),
};

// ---------------------------------------------------------------------------------------------
// Table 0 — the two corpora, side by side.

const shapeOf = (tag) =>
  one(`
    SELECT (SELECT count(*) FROM v_${tag}) AS vertices,
           (SELECT count(*) FROM e_${tag}) AS edges,
           (SELECT count(DISTINCT cl) FROM v_${tag}) AS clusters,
           (SELECT round(avg(deg), 2) FROM deg_${tag}) AS mean_deg,
           (SELECT median(deg) FROM deg_${tag}) AS median_deg,
           (SELECT quantile_cont(deg, 0.99) FROM deg_${tag}) AS p99_deg,
           (SELECT max(deg) FROM deg_${tag}) AS max_deg,
           (SELECT count(DISTINCT deg) FROM deg_${tag}) AS distinct_degrees,
           (SELECT round(100.0 * sum(CASE WHEN a.cl = b.cl THEN 1 ELSE 0 END) / count(*), 3)
              FROM e_${tag} e JOIN v_${tag} a ON a.id = e.s JOIN v_${tag} b ON b.id = e.d) AS pct_intra,
           (SELECT round(avg(n), 1) FROM (SELECT cl, count(*) AS n FROM v_${tag} GROUP BY cl)) AS mean_cluster,
           (SELECT max(n) FROM (SELECT cl, count(*) AS n FROM v_${tag} GROUP BY cl)) AS max_cluster
  `);

const shapes = { real: shapeOf('r'), ...(synth ? { synthetic: shapeOf('s') } : {}) };

// ---------------------------------------------------------------------------------------------
// Q1 — THE HEADLINE. How much graph locality does Morton order already have?
//
// `dense_id` IS the Morton rank, so `|dense_id(src) - dense_id(dst)|` is the distance
// the on-disk order puts between the two ends of an edge, and `id >> 12` is the tile.
// `same tile` is the strongest form; `within one run` is what actually matters to a
// reader, because two adjacent tiles collapse into one Range request.

const localityOf = (tag) =>
  one(`
    WITH g AS (
      SELECT abs(e.d - e.s) AS gap,
             (e.s >> ${TILE_SHIFT}) AS ts, (e.d >> ${TILE_SHIFT}) AS td
      FROM e_${tag} e
    )
    SELECT count(*) AS edges,
           round(avg(gap)) AS mean_gap,
           median(gap) AS median_gap,
           quantile_cont(gap, 0.95) AS p95_gap,
           max(gap) AS max_gap,
           round(100.0 * sum(CASE WHEN ts = td THEN 1 ELSE 0 END) / count(*), 2) AS pct_same_tile,
           round(100.0 * sum(CASE WHEN abs(ts - td) <= 1 THEN 1 ELSE 0 END) / count(*), 2) AS pct_adjacent_tile,
           round(100.0 * sum(CASE WHEN abs(ts - td) <= 4 THEN 1 ELSE 0 END) / count(*), 2) AS pct_within_4,
           round(100.0 * sum(CASE WHEN gap < ${TILE} THEN 1 ELSE 0 END) / count(*), 2) AS pct_gap_under_tile,
           round(avg(abs(ts - td)), 1) AS mean_tile_gap,
           median(abs(ts - td)) AS median_tile_gap
    FROM g`);

// The null model the percentages have to beat: the same edge count between the same
// number of vertices under a RANDOM order. Derived in closed form rather than
// shuffled — for n vertices in t tiles of size k, two independent uniform endpoints
// land in the same tile with probability ~1/t.
const nullModel = (vertices) => {
  const tiles = Math.ceil(vertices / TILE);
  return {
    tiles,
    sameTile: (100 / tiles).toFixed(3),
    adjacent: ((100 * (3 * tiles - 2)) / tiles ** 2).toFixed(3),
  };
};

const locality = { real: localityOf('r'), ...(synth ? { synthetic: localityOf('s') } : {}) };

// The gap histogram, in powers of two. A shape, not a summary: a distribution with a
// spike at zero and a long flat tail means something different from a smooth one.
const gapHistogram = (tag) =>
  ask(`
    WITH g AS (SELECT abs(d - s) AS gap FROM e_${tag}),
         b AS (SELECT CASE WHEN gap = 0 THEN -1 ELSE floor(log2(gap))::INT END AS lg FROM g)
    SELECT lg,
           count(*) AS n,
           round(100.0 * count(*) / (SELECT count(*) FROM b), 2) AS pct
    FROM b GROUP BY lg ORDER BY lg`);

const gaps = { real: gapHistogram('r'), ...(synth ? { synthetic: gapHistogram('s') } : {}) };

// ---------------------------------------------------------------------------------------------
// Q2 — Louvain against SNAP's published ground truth.

process.stderr.write('comparing Louvain against ground truth…\n');

let truth = null;
if (existsSync(truthTxt)) {
  build(`
    -- SNAP's file is one community per line, node ids separated by tabs. Read the
    -- whole line as text and unnest it: the lines have wildly different lengths and
    -- there is no column count to declare.
    CREATE OR REPLACE TABLE gt_raw AS
      SELECT row_number() OVER () AS gid, line
      FROM read_csv('${lit(truthTxt)}', delim='\\x01', header=false, columns={'line':'VARCHAR'});
    CREATE OR REPLACE TABLE gt AS
      SELECT gid, trim(tok)::BIGINT AS aid
      FROM (SELECT gid, unnest(str_split(line, chr(9))) AS tok FROM gt_raw)
      WHERE trim(tok) <> '';

    -- SNAP ids -> dense_id, through the \`aid\` property the program wrote. The
    -- subject IRI would do as well; the column is cheaper and says the same thing.
    CREATE OR REPLACE TABLE aidmap AS
      SELECT aid::BIGINT AS aid, dense_id::BIGINT AS id
      FROM read_parquet('${lit(real.vertex)}');

    CREATE OR REPLACE TABLE gtv AS
      SELECT gt.gid, m.id, v.cl
      FROM gt JOIN aidmap m ON m.aid = gt.aid JOIN v_r v ON v.id = m.id;
  `);

  truth = {
    coverage: one(`
      SELECT (SELECT count(DISTINCT gid) FROM gt) AS communities,
             (SELECT count(DISTINCT aid) FROM gt) AS nodes_in_truth,
             (SELECT count(*) FROM gt) AS memberships,
             (SELECT count(DISTINCT id) FROM gtv) AS matched_vertices,
             (SELECT round(avg(n), 1) FROM (SELECT gid, count(*) AS n FROM gt GROUP BY gid)) AS mean_size,
             (SELECT max(n) FROM (SELECT gid, count(*) AS n FROM gt GROUP BY gid)) AS max_size,
             (SELECT round(avg(k), 2) FROM (SELECT aid, count(*) AS k FROM gt GROUP BY aid)) AS mean_memberships`),
    // Purity: for each ground-truth community, the share of its members that sit in
    // its single most common Louvain cluster. 100% means Louvain contains it whole.
    purity: one(`
      WITH per AS (
        SELECT gid, cl, count(*) AS n FROM gtv GROUP BY gid, cl
      ),
      top AS (
        SELECT gid, max(n) AS best, sum(n) AS total, count(*) AS spread FROM per GROUP BY gid
      )
      SELECT round(avg(100.0 * best / total), 2) AS mean_purity,
             round(median(100.0 * best / total), 2) AS median_purity,
             round(quantile_cont(100.0 * best / total, 0.05), 2) AS p05_purity,
             round(100.0 * sum(CASE WHEN best = total THEN 1 ELSE 0 END) / count(*), 2) AS pct_wholly_contained,
             round(avg(spread), 2) AS mean_louvain_clusters_spanned,
             max(spread) AS max_louvain_clusters_spanned
      FROM top`),
    // The edge-level version, which is the one that matters for locality: of the
    // edges INSIDE a ground-truth community, how many did Louvain also keep inside
    // one cluster — and, the question this script exists for, how many landed in
    // one tile.
    edges: one(`
      WITH gte AS (
        SELECT DISTINCT e.s, e.d
        FROM e_r e JOIN gtv a ON a.id = e.s JOIN gtv b ON b.id = e.d AND b.gid = a.gid
      )
      SELECT count(*) AS truth_internal_edges,
             round(100.0 * sum(CASE WHEN va.cl = vb.cl THEN 1 ELSE 0 END) / count(*), 2) AS pct_louvain_agrees,
             round(100.0 * sum(CASE WHEN (e.s >> ${TILE_SHIFT}) = (e.d >> ${TILE_SHIFT}) THEN 1 ELSE 0 END)
                   / count(*), 2) AS pct_same_tile,
             round(avg(abs(e.d - e.s))) AS mean_gap
      FROM gte e JOIN v_r va ON va.id = e.s JOIN v_r vb ON vb.id = e.d`),
  };
}

// ---------------------------------------------------------------------------------------------
// Q3 — are the communities separated in the plane?

process.stderr.write('measuring spatial separation…\n');

const separation = (tag) =>
  one(`
    WITH ext AS (SELECT max(x) - min(x) AS w, max(y) - min(y) AS h FROM v_${tag}),
         disc AS (
           SELECT cl, count(*) AS n,
                  max(x) - min(x) AS w, max(y) - min(y) AS h,
                  avg(x) AS cx, avg(y) AS cy
           FROM v_${tag} GROUP BY cl
         ),
         spread AS (
           SELECT d.cl, d.n, d.w, d.h, d.cx, d.cy,
                  (SELECT avg(sqrt((v.x - d.cx) * (v.x - d.cx) + (v.y - d.cy) * (v.y - d.cy)))
                   FROM v_${tag} v WHERE v.cl = d.cl) AS radius
           FROM disc d
         ),
         near AS (
           SELECT a.cl, min(sqrt((a.cx - b.cx) * (a.cx - b.cx) + (a.cy - b.cy) * (a.cy - b.cy))) AS d_near,
                  a.radius
           FROM spread a JOIN spread b ON b.cl <> a.cl
           GROUP BY a.cl, a.radius
         )
    SELECT round(avg(s.w / ext.w), 4) AS mean_disc_width_over_extent,
           round(max(s.w / ext.w), 4) AS max_disc_width_over_extent,
           round(avg(n.d_near / nullif(n.radius, 0)), 3) AS mean_centroid_gap_over_radius,
           round(median(n.d_near / nullif(n.radius, 0)), 3) AS median_centroid_gap_over_radius,
           round(100.0 * sum(CASE WHEN n.d_near > n.radius THEN 1 ELSE 0 END) / count(*), 1) AS pct_clusters_disjoint
    FROM spread s JOIN near n ON n.cl = s.cl, ext`);

const sep = { real: separation('r'), ...(synth ? { synthetic: separation('s') } : {}) };

// ---------------------------------------------------------------------------------------------
// Q4 — what neighbours(v) costs, across the degree distribution.
//
// The sample the old script could not draw. Degree bands in powers of two, `perBand`
// vertices from each band at fixed positions in that band's own id order — no RNG, so
// a re-run reproduces every number — plus the `hubCount` highest-degree vertices as
// their own band, which is the row the synthetic fixture had no way to produce.

process.stderr.write('sampling across the degree distribution…\n');

build(`
  CREATE OR REPLACE TABLE band AS
    SELECT id, deg,
           CASE WHEN deg = 0 THEN 0 ELSE floor(log2(deg))::INT END AS lg
    FROM deg_r;

  CREATE OR REPLACE TABLE hubs AS
    SELECT id, deg, 'hub' AS bandname FROM band ORDER BY deg DESC, id LIMIT ${hubCount};

  CREATE OR REPLACE TABLE smp AS
    WITH r AS (
      SELECT id, deg, lg,
             row_number() OVER (PARTITION BY lg ORDER BY id) AS rn,
             count(*) OVER (PARTITION BY lg) AS n
      FROM band
    ),
    picked AS (
      SELECT id, deg, lg FROM r
      WHERE rn IN (
        SELECT DISTINCT greatest(1, ((n * g) / ${perBand})::BIGINT)
        FROM (SELECT unnest(range(0, ${perBand})) AS g)
      )
    )
    SELECT id AS v, deg, ('2^' || lg) AS bandname FROM picked
    UNION
    SELECT id AS v, deg, bandname FROM hubs;

  CREATE OR REPLACE TABLE h1 AS
    SELECT DISTINCT smp.v, adj.b AS u FROM smp JOIN adj_r adj ON adj.a = smp.v WHERE adj.b <> smp.v;

  CREATE OR REPLACE TABLE h2 AS
    SELECT DISTINCT v, u FROM (
      SELECT v, u FROM h1
      UNION ALL
      SELECT h1.v, adj.b AS u FROM h1 JOIN adj_r adj ON adj.a = h1.u
    ) WHERE u <> v;

  -- Where each vertex's rows sit in each projection. Contiguous by construction: a
  -- projection is sorted by its aligned endpoint, so one vertex is one range.
  CREATE OR REPLACE TABLE ep AS
    SELECT s, d,
           (row_number() OVER (ORDER BY s, d)) - 1 AS ps,
           (row_number() OVER (ORDER BY d, s)) - 1 AS pt
    FROM e_r;
  CREATE OR REPLACE TABLE srng AS SELECT s AS id, min(ps) AS lo, max(ps) AS hi FROM ep GROUP BY s;
  CREATE OR REPLACE TABLE trng AS SELECT d AS id, min(pt) AS lo, max(pt) AS hi FROM ep GROUP BY d;

  CREATE OR REPLACE TABLE self1 AS SELECT v, v AS u FROM smp;
  CREATE OR REPLACE TABLE frontier AS SELECT v, v AS u FROM smp UNION SELECT v, u FROM h1;
`);

/** Distinct vertex tiles + adjacent runs of `dense_id >> 12`, per query key. */
const vertexCost = (table) => `
  SELECT v, count(*) AS tiles,
         sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
  FROM (SELECT v, tile, lag(tile) OVER (PARTITION BY v ORDER BY tile) AS pv
        FROM (SELECT DISTINCT h.v, (h.u >> ${TILE_SHIFT}) AS tile FROM ${table} h))
  GROUP BY v`;

/** Edge tiles one projection must open for a frontier, from each vertex's range. */
const edgeCost = (table, rng) => `
  SELECT v, count(*) AS tiles,
         sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
  FROM (SELECT v, tile, lag(tile) OVER (PARTITION BY v ORDER BY tile) AS pv
        FROM (SELECT DISTINCT h.v,
                     unnest(range(r.lo >> ${TILE_SHIFT}, (r.hi >> ${TILE_SHIFT}) + 1)) AS tile
              FROM ${table} h JOIN ${rng} r ON r.id = h.u))
  GROUP BY v`;

const bandOrder = `CASE WHEN bandname = 'hub' THEN 999 ELSE CAST(substr(bandname, 3) AS INT) END`;

const hopCost = (hopTable, frontierTable) =>
  ask(`
    WITH vt AS (${vertexCost(hopTable)}),
         es AS (${edgeCost(frontierTable, 'srng')}),
         et AS (${edgeCost(frontierTable, 'trng')}),
         sz AS (SELECT v, count(*) AS n FROM ${hopTable} GROUP BY v),
         j AS (
           SELECT smp.bandname, smp.deg, smp.v,
                  sz.n AS reached,
                  vt.tiles AS vtiles, vt.runs AS vruns,
                  es.runs AS sruns, et.runs AS truns,
                  vt.runs + es.runs + et.runs AS total_runs,
                  vt.tiles * ${Number(rg.r.v.mean_bytes)}
                    + es.tiles * ${Number(rg.r.s.mean_bytes)}
                    + et.tiles * ${Number(rg.r.t.mean_bytes)} AS bytes
           FROM smp JOIN sz ON sz.v = smp.v JOIN vt ON vt.v = smp.v
                    JOIN es ON es.v = smp.v JOIN et ON et.v = smp.v
         )
    SELECT bandname,
           count(*) AS sampled,
           min(deg) AS deg_lo, max(deg) AS deg_hi,
           round(avg(reached)) AS mean_reached,
           round(avg(vtiles), 1) AS vtiles_mean, max(vtiles) AS vtiles_max,
           round(avg(vruns), 1) AS vruns_mean, median(vruns) AS vruns_med,
           round(quantile_cont(vruns, 0.95), 1) AS vruns_p95, max(vruns) AS vruns_max,
           round(avg(total_runs), 1) AS runs_mean, median(total_runs) AS runs_med,
           round(quantile_cont(total_runs, 0.95), 1) AS runs_p95, max(total_runs) AS runs_max,
           round(avg(bytes) / 1024, 1) AS kb_mean,
           round(quantile_cont(bytes, 0.95) / 1024, 1) AS kb_p95,
           round(max(bytes) / 1024, 1) AS kb_max
    FROM j GROUP BY bandname ORDER BY ${bandOrder}`);

const hop1 = hopCost('h1', 'self1');
const hop2 = hopCost('h2', 'frontier');

// ---------------------------------------------------------------------------------------------
// The camera — the same nine-position window family `measure-locality.mjs` uses, so the
// two reports' camera tables are readable against one another.

process.stderr.write('measuring camera windows…\n');

const cameraOf = (tag) => {
  build(`
    CREATE OR REPLACE TABLE win_${tag} AS
      WITH ext AS (SELECT min(x) AS x0, max(x) AS x1, min(y) AS y0, max(y) AS y1 FROM v_${tag}),
           f AS (SELECT unnest([0.01, 0.05, 0.10, 0.25, 0.50]) AS frac),
           g AS (SELECT unnest([0, 1, 2]) AS gx),
           h AS (SELECT unnest([0, 1, 2]) AS gy)
      SELECT row_number() OVER (ORDER BY frac, gx, gy) AS wid, frac,
             greatest(x0, x0 + (x1 - x0) * (gx + 0.5) / 3 - (x1 - x0) * frac / 2) AS minx,
             least(x1,  x0 + (x1 - x0) * (gx + 0.5) / 3 + (x1 - x0) * frac / 2) AS maxx,
             greatest(y0, y0 + (y1 - y0) * (gy + 0.5) / 3 - (y1 - y0) * frac / 2) AS miny,
             least(y1,  y0 + (y1 - y0) * (gy + 0.5) / 3 + (y1 - y0) * frac / 2) AS maxy
      FROM ext, f, g, h;
    CREATE OR REPLACE TABLE inwin_${tag} AS
      SELECT w.wid, w.frac, v.id
      FROM win_${tag} w JOIN v_${tag} v
        ON v.x BETWEEN w.minx AND w.maxx AND v.y BETWEEN w.miny AND w.maxy;
  `);
  return ask(`
    WITH d AS (SELECT DISTINCT frac, wid, (id >> ${TILE_SHIFT}) AS tile FROM inwin_${tag}),
         w AS (SELECT frac, wid, tile, lag(tile) OVER (PARTITION BY wid ORDER BY tile) AS pv FROM d),
         agg AS (SELECT frac, wid, count(*) AS tiles,
                        sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
                 FROM w GROUP BY frac, wid),
         pop AS (SELECT frac, wid, count(*) AS marks FROM inwin_${tag} GROUP BY frac, wid)
    SELECT agg.frac, round(avg(pop.marks)) AS mean_marks,
           round(avg(agg.tiles), 1) AS mean_tiles, max(agg.tiles) AS max_tiles,
           round(avg(agg.runs), 1) AS mean_runs, max(agg.runs) AS max_runs,
           round(100.0 * avg(pop.marks) / nullif(avg(agg.tiles) * ${TILE}, 0), 1) AS pct_useful
    FROM agg JOIN pop ON pop.wid = agg.wid
    GROUP BY agg.frac ORDER BY agg.frac`);
};

const camera = { real: cameraOf('r'), ...(synth ? { synthetic: cameraOf('s') } : {}) };

// ---------------------------------------------------------------------------------------------
// Q5 — the index nobody is reading.
//
// `cluster_id` is a payload COLUMN, so Parquet writes its min/max into the footer for
// every row group — that is, for every TILE. `src/stream.ts::FOOTER_SQL` selects `x`
// and `y` and stops, so a community-to-tile index already exists in every corpus,
// already paid for, unread. If Morton order keeps a tile's cluster range narrow, the
// footer alone answers "which tiles hold community c" before a single data byte moves.
//
// Three ways to answer that question are costed here:
//
//   (a) **scan** — read the payload, filter. Every tile, every byte.
//   (b) **footer-pruned** — keep the tiles whose `[cluster_lo, cluster_hi]` straddles c,
//       then read those. Sound but not exact: a range is not a set, so a tile whose
//       range spans c without containing a member is a false positive, and `precision`
//       below is what that costs.
//   (c) **candidate order** — `(cluster_id, Morton-within-cluster)`, where a community
//       is contiguous by construction and therefore always exactly one run.
//
// (b) is the interesting column because it needs no new artefact, no second order and
// no writer change — only a query.

process.stderr.write('measuring the footer cluster_id index…\n');

/** Per-tile `cluster_id` min/max straight out of the Parquet footer. */
const footerIndex = (tag, path) => {
  build(`
    CREATE OR REPLACE TABLE fi_${tag} AS
      SELECT row_group_id AS tile,
             any_value(row_group_num_rows) AS rows,
             sum(total_compressed_size) AS bytes,
             min(CASE WHEN path_in_schema = 'cluster_id' THEN CAST(stats_min_value AS BIGINT) END) AS clo,
             max(CASE WHEN path_in_schema = 'cluster_id' THEN CAST(stats_max_value AS BIGINT) END) AS chi
      FROM parquet_metadata('${lit(path)}')
      GROUP BY row_group_id;
    -- What the payload actually holds per tile, so the footer's range can be held
    -- against the truth rather than trusted.
    CREATE OR REPLACE TABLE ft_${tag} AS
      SELECT (id >> ${TILE_SHIFT}) AS tile, count(DISTINCT cl) AS distinct_clusters, count(*) AS n
      FROM v_${tag} GROUP BY 1;
  `);
  return one(`
    SELECT count(*) AS tiles,
           round(avg(chi - clo + 1), 1) AS mean_range_width,
           median(chi - clo + 1) AS median_range_width,
           round(quantile_cont(chi - clo + 1, 0.95), 1) AS p95_range_width,
           max(chi - clo + 1) AS max_range_width,
           round(avg(t.distinct_clusters), 1) AS mean_distinct_clusters,
           max(t.distinct_clusters) AS max_distinct_clusters,
           round(avg(1.0 * t.distinct_clusters / nullif(chi - clo + 1, 0)), 3) AS mean_density,
           (SELECT count(DISTINCT cl) FROM v_${tag}) AS total_clusters,
           round(100.0 * avg(chi - clo + 1) / (SELECT count(DISTINCT cl) FROM v_${tag}), 2) AS mean_range_pct_of_all
    FROM fi_${tag} f JOIN ft_${tag} t ON t.tile = f.tile`);
};

/** The three ways to answer "give me community c", per community, then averaged. */
const communityFetch = (tag) =>
  one(`
    WITH
    -- A HYPOTHETICAL, and the only one in this script: what the same footer index
    -- would prune to if cluster_id were numbered in Morton order of the cluster
    -- rather than in whatever order Louvain happened to emit labels. Louvain's
    -- labels are arbitrary, so a tile holding a handful of spatially adjacent
    -- communities can still carry a footer range spanning most of the id space --
    -- a failure of the LABELLING, not of the layout, and the two have very
    -- different fixes. crank is that renumbering; nothing on disk changes.
    ren AS (
      SELECT cl, (dense_rank() OVER (ORDER BY first_id)) - 1 AS crank
      FROM (SELECT cl, min(id) AS first_id FROM v_${tag} GROUP BY cl)
    ),
    rtile AS (
      SELECT (v.id >> ${TILE_SHIFT}) AS tile, min(r.crank) AS rlo, max(r.crank) AS rhi
      FROM v_${tag} v JOIN ren r ON r.cl = v.cl GROUP BY 1
    ),
    renpruned AS (
      SELECT r.cl, count(*) AS tiles,
             sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
      FROM (
        SELECT r.cl, t.tile, lag(t.tile) OVER (PARTITION BY r.cl ORDER BY t.tile) AS pv
        FROM ren r JOIN rtile t ON r.crank BETWEEN t.rlo AND t.rhi
      ) r GROUP BY r.cl
    ),
    truth AS (
      SELECT cl, count(*) AS members, min(id) AS lo, max(id) AS hi,
             count(DISTINCT (id >> ${TILE_SHIFT})) AS true_tiles
      FROM v_${tag} GROUP BY cl
    ),
    -- (b) every tile whose FOOTER range straddles the community id.
    pruned AS (
      SELECT t.cl, f.tile, f.bytes, f.rows
      FROM truth t JOIN fi_${tag} f ON t.cl BETWEEN f.clo AND f.chi
    ),
    pr AS (
      SELECT cl, count(*) AS tiles, sum(bytes) AS bytes, sum(rows) AS rows,
             sum(CASE WHEN pv IS NULL OR tile <> pv + 1 THEN 1 ELSE 0 END) AS runs
      FROM (SELECT cl, tile, bytes, rows,
                   lag(tile) OVER (PARTITION BY cl ORDER BY tile) AS pv FROM pruned)
      GROUP BY cl
    ),
    -- (c) the candidate order: a community is one contiguous rank range, always one run.
    cand AS (
      SELECT cl, count(*) AS members,
             (max(rk) >> ${TILE_SHIFT}) - (min(rk) >> ${TILE_SHIFT}) + 1 AS tiles
      FROM (SELECT cl, (row_number() OVER (ORDER BY cl, id)) - 1 AS rk FROM v_${tag})
      GROUP BY cl
    ),
    total AS (SELECT count(*) AS tiles, sum(bytes) AS bytes FROM fi_${tag})
    SELECT (SELECT tiles FROM total) AS scan_tiles,
           (SELECT round(bytes / 1024.0 / 1024.0, 1) FROM total) AS scan_mb,
           round(avg(pr.tiles), 1) AS pruned_tiles_mean,
           median(pr.tiles) AS pruned_tiles_med,
           round(quantile_cont(pr.tiles, 0.95), 1) AS pruned_tiles_p95,
           max(pr.tiles) AS pruned_tiles_max,
           round(avg(pr.runs), 1) AS pruned_runs_mean,
           median(pr.runs) AS pruned_runs_med,
           max(pr.runs) AS pruned_runs_max,
           round(avg(pr.bytes) / 1024, 1) AS pruned_kb_mean,
           round(100.0 * sum(t.members) / sum(pr.rows), 2) AS pruned_precision,
           round(100.0 * (SELECT tiles FROM total) / avg(pr.tiles), 0) AS pruned_speedup_pct,
           round(avg(c.tiles), 1) AS cand_tiles_mean,
           max(c.tiles) AS cand_tiles_max,
           round(avg(rp.tiles), 1) AS ren_tiles_mean,
           max(rp.tiles) AS ren_tiles_max,
           round(avg(rp.runs), 1) AS ren_runs_mean,
           round(avg(t.true_tiles), 1) AS true_tiles_mean,
           max(t.true_tiles) AS true_tiles_max,
           round(avg(t.members), 1) AS members_mean,
           max(t.members) AS members_max
    FROM truth t JOIN pr ON pr.cl = t.cl JOIN cand c ON c.cl = t.cl
                  JOIN renpruned rp ON rp.cl = t.cl`);

const footer = { real: footerIndex('r', real.vertex), ...(synth ? { synthetic: footerIndex('s', synth.vertex) } : {}) };
const fetchCost = { real: communityFetch('r'), ...(synth ? { synthetic: communityFetch('s') } : {}) };

// ---------------------------------------------------------------------------------------------
// Report

const N = (x) => (x === null || x === undefined ? '—' : typeof x === 'number' ? x : Number(x));
const table = (head, rows) =>
  [`| ${head.join(' | ')} |`, `|${head.map(() => '---').join('|')}|`, ...rows.map((r) => `| ${r.join(' | ')} |`)].join(
    '\n',
  );
const MB = (n) => `${(Number(n) / 1e6).toFixed(1)} MB`;
const pair = (get) => (synth ? [get(shapes.real, 'real'), get(shapes.synthetic, 'synthetic')] : [get(shapes.real, 'real')]);

const nullReal = nullModel(Number(shapes.real.vertices));
const nullSynth = synth ? nullModel(Number(shapes.synthetic.vertices)) : null;

const heads = synth ? ['fact', `real — ${DATASET.name}`, 'synthetic — guards/fixture.mjs'] : ['fact', 'real'];
const row2 = (label, a, b) => (synth ? [label, a, b] : [label, a]);

console.log(`# Does \`dense_id\` order already follow the graph? — measured on a real graph

Real corpus: \`${real.dir}\` (\`${real.type}\`, relation \`${real.relation}\`)
${synth ? `Synthetic corpus: \`${synth.dir}\` (\`${synth.type}\`, relation \`${synth.relation}\`)` : 'Synthetic corpus: not compared (\`--no-synthetic\`).'}
Tile: ${TILE} rows. Every figure below is produced by \`scripts/measure-realkg.mjs\` and nothing else.

Input: **${DATASET.name}** — ${DATASET.cite}. Pinned by URL, byte count and SHA-256 in
\`scripts/realkg-prepare.mjs\`; the corpus is what \`fossil run bench/dblp/dblp.fossil\` wrote, so
the \`cluster_id\` and \`(x, y)\` below are fossil's own Louvain and its own placement.

## Table 0 — the two graphs

${table(
  heads,
  [
    row2('vertices', N(shapes.real.vertices), synth && N(shapes.synthetic.vertices)),
    row2('edges (directed rows in by_source)', N(shapes.real.edges), synth && N(shapes.synthetic.edges)),
    row2('undirected degree mean / median', `${N(shapes.real.mean_deg)} / ${N(shapes.real.median_deg)}`, synth && `${N(shapes.synthetic.mean_deg)} / ${N(shapes.synthetic.median_deg)}`),
    row2('degree p99 / max', `${N(shapes.real.p99_deg)} / ${N(shapes.real.max_deg)}`, synth && `${N(shapes.synthetic.p99_deg)} / ${N(shapes.synthetic.max_deg)}`),
    row2('**distinct degrees**', `**${N(shapes.real.distinct_degrees)}**`, synth && `**${N(shapes.synthetic.distinct_degrees)}**`),
    row2('Louvain clusters', N(shapes.real.clusters), synth && N(shapes.synthetic.clusters)),
    row2('cluster size mean / max', `${N(shapes.real.mean_cluster)} / ${N(shapes.real.max_cluster)}`, synth && `${N(shapes.synthetic.mean_cluster)} / ${N(shapes.synthetic.max_cluster)}`),
    row2('edges intra-cluster', `${N(shapes.real.pct_intra)}%`, synth && `${N(shapes.synthetic.pct_intra)}%`),
    row2('vertex payload', `${MB(rg.r.v.total_bytes)} (${N(rg.r.v.groups)} row groups, mean ${N(rg.r.v.mean_bytes)} B)`, synth && `${MB(rg.s.v.total_bytes)} (${N(rg.s.v.groups)} row groups, mean ${N(rg.s.v.mean_bytes)} B)`),
    row2('by_source / by_target', `${MB(rg.r.s.total_bytes)} / ${MB(rg.r.t.total_bytes)}`, synth && `${MB(rg.s.s.total_bytes)} / ${MB(rg.s.t.total_bytes)}`),
  ].map((r) => r.filter((c) => c !== false)),
)}

The **distinct degrees** row is why this document exists. The synthetic corpus has no degree
distribution at all, so nothing measured on it says what a hub costs, and a hub is exactly where
an index earns its keep.

## Table 1 — THE HEADLINE: graph locality already present in Morton order

\`dense_id\` IS the Morton rank of the layout position, so \`|dense_id(src) - dense_id(dst)|\` is
what the on-disk order costs an edge, and \`>> ${TILE_SHIFT}\` is its tile. \`random\` is the null
model: the same endpoints under an order that knows nothing about the graph.

${table(
  ['corpus', 'mean gap', 'median gap', 'p95 gap', 'same tile', 'random baseline', 'within 1 tile', 'within 4 tiles', 'mean tile gap'],
  [
    [
      'real',
      N(locality.real.mean_gap),
      N(locality.real.median_gap),
      N(locality.real.p95_gap),
      `**${N(locality.real.pct_same_tile)}%**`,
      `${nullReal.sameTile}%`,
      `${N(locality.real.pct_adjacent_tile)}%`,
      `${N(locality.real.pct_within_4)}%`,
      N(locality.real.mean_tile_gap),
    ],
    ...(synth
      ? [
          [
            'synthetic',
            N(locality.synthetic.mean_gap),
            N(locality.synthetic.median_gap),
            N(locality.synthetic.p95_gap),
            `**${N(locality.synthetic.pct_same_tile)}%**`,
            `${nullSynth.sameTile}%`,
            `${N(locality.synthetic.pct_adjacent_tile)}%`,
            `${N(locality.synthetic.pct_within_4)}%`,
            N(locality.synthetic.mean_tile_gap),
          ],
        ]
      : []),
  ],
)}

### Table 1b — the gap distribution, by power of two

${table(
  ['\\|Δdense_id\\|', ...(synth ? ['real %', 'synthetic %'] : ['real %'])],
  (() => {
    const keys = [...new Set([...gaps.real, ...(synth ? gaps.synthetic : [])].map((r) => Number(r.lg)))].sort(
      (a, b) => a - b,
    );
    const at = (rows, lg) => rows.find((r) => Number(r.lg) === lg);
    return keys.map((lg) => [
      lg < 0 ? '0' : `2^${lg}..2^${lg + 1}`,
      at(gaps.real, lg) ? `${N(at(gaps.real, lg).pct)}%` : '0%',
      ...(synth ? [at(gaps.synthetic, lg) ? `${N(at(gaps.synthetic, lg).pct)}%` : '0%'] : []),
    ]);
  })(),
)}

## Table 2 — Louvain against SNAP's published ground truth

${
  truth
    ? `Two independent partitions of one graph: fossil's \`cluster_id\`, and the ${N(truth.coverage.communities)}
communities SNAP publishes for this network. The ground truth **overlaps** — a node belongs to
${N(truth.coverage.mean_memberships)} communities on average — so this is not a partition-vs-partition
score (no NMI, no ARI: neither is defined against an overlapping cover). Purity and edge agreement
are, and they are what is reported.

${table(
  ['fact', 'value'],
  [
    ['ground-truth communities', N(truth.coverage.communities)],
    ['nodes covered by the ground truth', `${N(truth.coverage.nodes_in_truth)} of ${N(shapes.real.vertices)}`],
    ['matched into the corpus by `aid`', N(truth.coverage.matched_vertices)],
    ['community size mean / max', `${N(truth.coverage.mean_size)} / ${N(truth.coverage.max_size)}`],
    ['memberships per node (mean)', N(truth.coverage.mean_memberships)],
    ['**purity — mean / median**', `**${N(truth.purity.mean_purity)}% / ${N(truth.purity.median_purity)}%**`],
    ['purity p05', `${N(truth.purity.p05_purity)}%`],
    ['communities wholly inside one Louvain cluster', `${N(truth.purity.pct_wholly_contained)}%`],
    ['Louvain clusters a community spans (mean / max)', `${N(truth.purity.mean_louvain_clusters_spanned)} / ${N(truth.purity.max_louvain_clusters_spanned)}`],
    ['ground-truth-internal edges', N(truth.edges.truth_internal_edges)],
    ['…of which Louvain also keeps internal', `**${N(truth.edges.pct_louvain_agrees)}%**`],
    ['…of which land in the SAME TILE', `${N(truth.edges.pct_same_tile)}%`],
    ['…mean |Δdense_id| across them', N(truth.edges.mean_gap)],
  ],
)}`
    : `Not measured: \`${truthTxt}\` is absent. Run \`node scripts/realkg-prepare.mjs\` first.`
}

## Table 3 — are the communities separated in the plane?

The failure mode the synthetic fixture has, measured on both. \`disc width / extent\` at 1.0 means a
cluster is as wide as the whole corpus; \`centroid gap / radius\` below 1 means a cluster's nearest
neighbour's centre sits inside its own mean radius, i.e. they interpenetrate.

${table(
  ['corpus', 'disc width / extent (mean, max)', 'centroid gap / radius (mean, median)', 'clusters whose nearest centroid is outside their radius'],
  [
    [
      'real',
      `${N(sep.real.mean_disc_width_over_extent)}, ${N(sep.real.max_disc_width_over_extent)}`,
      `${N(sep.real.mean_centroid_gap_over_radius)}, ${N(sep.real.median_centroid_gap_over_radius)}`,
      `${N(sep.real.pct_clusters_disjoint)}%`,
    ],
    ...(synth
      ? [
          [
            'synthetic',
            `${N(sep.synthetic.mean_disc_width_over_extent)}, ${N(sep.synthetic.max_disc_width_over_extent)}`,
            `${N(sep.synthetic.mean_centroid_gap_over_radius)}, ${N(sep.synthetic.median_centroid_gap_over_radius)}`,
            `${N(sep.synthetic.pct_clusters_disjoint)}%`,
          ],
        ]
      : []),
  ],
)}

## Table 4 — what \`neighbours(v)\` costs, by degree

\`tiles\` are Parquet row groups; \`runs\` are maximal adjacent stretches, i.e. one HTTP \`Range\`
request each. \`TOTAL runs\` is the whole query: by_source + by_target to find the neighbours, then
the vertex runs to materialise them. Bytes are tiles times the mean row-group size Parquet wrote
(vertex ${N(rg.r.v.mean_bytes)} B, by_source ${N(rg.r.s.mean_bytes)} B, by_target ${N(rg.r.t.mean_bytes)} B).
The \`hub\` row is the ${hubCount} highest-degree vertices, reported separately — it is the row the
synthetic fixture could not produce.

### 1-hop

${table(
  ['band', 'n', 'degree lo–hi', 'mean \\|N\\|', 'vertex tiles mean/max', 'vertex runs mean/med/p95/max', 'TOTAL runs mean/med/p95/max', 'KB mean/p95/max'],
  hop1.map((r) => [
    r.bandname === 'hub' ? '**hub**' : r.bandname,
    N(r.sampled),
    `${N(r.deg_lo)}–${N(r.deg_hi)}`,
    N(r.mean_reached),
    `${N(r.vtiles_mean)} / ${N(r.vtiles_max)}`,
    `${N(r.vruns_mean)} / ${N(r.vruns_med)} / ${N(r.vruns_p95)} / ${N(r.vruns_max)}`,
    `${N(r.runs_mean)} / ${N(r.runs_med)} / ${N(r.runs_p95)} / ${N(r.runs_max)}`,
    `${N(r.kb_mean)} / ${N(r.kb_p95)} / ${N(r.kb_max)}`,
  ]),
)}

### 2-hop

${table(
  ['band', 'n', 'degree lo–hi', 'mean \\|N\\|', 'vertex tiles mean/max', 'vertex runs mean/med/p95/max', 'TOTAL runs mean/med/p95/max', 'KB mean/p95/max'],
  hop2.map((r) => [
    r.bandname === 'hub' ? '**hub**' : r.bandname,
    N(r.sampled),
    `${N(r.deg_lo)}–${N(r.deg_hi)}`,
    N(r.mean_reached),
    `${N(r.vtiles_mean)} / ${N(r.vtiles_max)}`,
    `${N(r.vruns_mean)} / ${N(r.vruns_med)} / ${N(r.vruns_p95)} / ${N(r.vruns_max)}`,
    `${N(r.runs_mean)} / ${N(r.runs_med)} / ${N(r.runs_p95)} / ${N(r.runs_max)}`,
    `${N(r.kb_mean)} / ${N(r.kb_p95)} / ${N(r.kb_max)}`,
  ]),
)}

## Table 5 — the camera

A window is a fraction of each side of the corpus extent, at each of nine positions on a 3x3 grid,
clipped to the extent. \`% useful\` is the marks returned over the rows the tiles contain — the
share of what a reader fetches that it actually wanted.

${table(
  ['corpus', 'window', 'mean marks', 'tiles mean/max', 'runs mean/max', '% useful'],
  [
    ...camera.real.map((r) => [
      'real',
      `${(Number(r.frac) * 100).toFixed(0)}%`,
      N(r.mean_marks),
      `${N(r.mean_tiles)} / ${N(r.max_tiles)}`,
      `${N(r.mean_runs)} / ${N(r.max_runs)}`,
      `${N(r.pct_useful)}%`,
    ]),
    ...(synth
      ? camera.synthetic.map((r) => [
          'synthetic',
          `${(Number(r.frac) * 100).toFixed(0)}%`,
          N(r.mean_marks),
          `${N(r.mean_tiles)} / ${N(r.max_tiles)}`,
          `${N(r.mean_runs)} / ${N(r.max_runs)}`,
          `${N(r.pct_useful)}%`,
        ])
      : []),
  ],
)}

## Table 6 — the index already in the footer

\`cluster_id\` is a payload column, so Parquet wrote its min/max into the footer for every row
group — that is, **for every tile**. \`src/stream.ts::FOOTER_SQL\` selects \`x\` and \`y\` and stops,
so this index exists in every corpus, is bought with the footer a reader already fetches, and is
read by nobody. How useful it is depends entirely on whether Morton order keeps a tile's cluster
range narrow.

${table(
  ['corpus', 'tiles', 'clusters', 'footer range width mean/med/p95/max', 'as % of all clusters', 'clusters actually in a tile mean/max', 'density (real / range)'],
  [
    [
      'real',
      N(footer.real.tiles),
      N(footer.real.total_clusters),
      `${N(footer.real.mean_range_width)} / ${N(footer.real.median_range_width)} / ${N(footer.real.p95_range_width)} / ${N(footer.real.max_range_width)}`,
      `${N(footer.real.mean_range_pct_of_all)}%`,
      `${N(footer.real.mean_distinct_clusters)} / ${N(footer.real.max_distinct_clusters)}`,
      N(footer.real.mean_density),
    ],
    ...(synth
      ? [
          [
            'synthetic',
            N(footer.synthetic.tiles),
            N(footer.synthetic.total_clusters),
            `${N(footer.synthetic.mean_range_width)} / ${N(footer.synthetic.median_range_width)} / ${N(footer.synthetic.p95_range_width)} / ${N(footer.synthetic.max_range_width)}`,
            `${N(footer.synthetic.mean_range_pct_of_all)}%`,
            `${N(footer.synthetic.mean_distinct_clusters)} / ${N(footer.synthetic.max_distinct_clusters)}`,
            N(footer.synthetic.mean_density),
          ],
        ]
      : []),
  ],
)}

A footer range is a RANGE, not a set: a tile whose \`[lo, hi]\` straddles community \`c\` may hold no
member of it. \`density\` is how much of the range is real — 1.0 would mean every id in the range is
present in the tile.

### Table 6b — "give me community c", three ways

(a) scan the payload; (b) keep only the tiles whose footer range straddles \`c\`, then read them;
(c) the candidate \`(cluster_id, Morton)\` order, where a community is contiguous and therefore
always exactly one run. \`precision\` is members returned over rows read — what the range's
imprecision costs. \`oracle\` is the tiles that actually hold a member, which is the floor (b) is
trying to reach.

\`(b')\` is the one hypothetical in this script: the SAME footer index after \`cluster_id\` is
renumbered in Morton order of the cluster instead of in whatever order Louvain emitted labels.
It costs no second on-disk order and no new file — only a different integer in a column the
writer already writes. The gap between (b) and (b') is how much of (b)'s failure is the
LABELLING rather than the layout.

${table(
  ['corpus', 'members mean/max', '(a) scan', '(b) footer-pruned tiles mean/med/p95/max', '(b) runs mean/max', '(b) bytes mean', '(b) precision', 'oracle tiles mean/max', "(b') footer after renumbering, tiles mean/max", '(c) candidate tiles mean/max'],
  [
    [
      'real',
      `${N(fetchCost.real.members_mean)} / ${N(fetchCost.real.members_max)}`,
      `${N(fetchCost.real.scan_tiles)} tiles, ${N(fetchCost.real.scan_mb)} MB`,
      `**${N(fetchCost.real.pruned_tiles_mean)}** / ${N(fetchCost.real.pruned_tiles_med)} / ${N(fetchCost.real.pruned_tiles_p95)} / ${N(fetchCost.real.pruned_tiles_max)}`,
      `${N(fetchCost.real.pruned_runs_mean)} / ${N(fetchCost.real.pruned_runs_max)}`,
      `${N(fetchCost.real.pruned_kb_mean)} KB`,
      `${N(fetchCost.real.pruned_precision)}%`,
      `${N(fetchCost.real.true_tiles_mean)} / ${N(fetchCost.real.true_tiles_max)}`,
      `${N(fetchCost.real.ren_tiles_mean)} / ${N(fetchCost.real.ren_tiles_max)}`,
            `${N(fetchCost.real.cand_tiles_mean)} / ${N(fetchCost.real.cand_tiles_max)}`,
    ],
    ...(synth
      ? [
          [
            'synthetic',
            `${N(fetchCost.synthetic.members_mean)} / ${N(fetchCost.synthetic.members_max)}`,
            `${N(fetchCost.synthetic.scan_tiles)} tiles, ${N(fetchCost.synthetic.scan_mb)} MB`,
            `**${N(fetchCost.synthetic.pruned_tiles_mean)}** / ${N(fetchCost.synthetic.pruned_tiles_med)} / ${N(fetchCost.synthetic.pruned_tiles_p95)} / ${N(fetchCost.synthetic.pruned_tiles_max)}`,
            `${N(fetchCost.synthetic.pruned_runs_mean)} / ${N(fetchCost.synthetic.pruned_runs_max)}`,
            `${N(fetchCost.synthetic.pruned_kb_mean)} KB`,
            `${N(fetchCost.synthetic.pruned_precision)}%`,
            `${N(fetchCost.synthetic.true_tiles_mean)} / ${N(fetchCost.synthetic.true_tiles_max)}`,
            `${N(fetchCost.synthetic.ren_tiles_mean)} / ${N(fetchCost.synthetic.ren_tiles_max)}`,
            `${N(fetchCost.synthetic.cand_tiles_mean)} / ${N(fetchCost.synthetic.cand_tiles_max)}`,
          ],
        ]
      : []),
  ],
)}

---

**What this cannot see.** No latency, no byte actually transferred, no server, no triplestore. One
graph, one Louvain run, one placement, one tile size, one vertex type and one relation. A run is a
request, not a millisecond.
`);

if (keepDb) process.stderr.write(`database kept at ${db}\n`);
else rmSync(work, { recursive: true, force: true });
