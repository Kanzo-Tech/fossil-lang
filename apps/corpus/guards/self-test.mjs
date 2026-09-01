#!/usr/bin/env node
/**
 * The guards, checked.
 *
 *   node guards/self-test.mjs
 *
 * A guard nobody has ever seen fail is a sentence with a `git blame` on it. Every check in
 * `guards.mjs` is a count of violations, which means an empty corpus satisfies all of them at once
 * — the classic way a conformance suite comes to pass vacuously and stop being evidence. So this
 * file does two things and neither is optional:
 *
 *   1. **Non-vacuity.** A conforming corpus is written, in both containers, and every guard passes
 *      on it. Then every guard's query is confirmed to have actually looked at something.
 *   2. **Mutation.** For each guard, a copy of that corpus is broken in exactly one way and the
 *      guard is required to fire. Collateral — other guards firing on the same break — is reported
 *      rather than forbidden: some conventions genuinely cannot be violated in isolation, and
 *      pretending otherwise would mean weakening the guards until they could.
 *
 * The three mutations this cannot perform are printed at the end, because the honest list of what a
 * document-plus-guards contract does *not* cover is the price of not having a type.
 */

import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { available, execute, lit } from "./duck.mjs";
import { inspect } from "./inspect.mjs";
import { GUARDS, checkVectors, runAll } from "./guards.mjs";
import { write } from "./fixture.mjs";

if (!available()) {
  console.error("the `duckdb` binary is not on PATH. The checker needs it and nothing else.");
  process.exit(2);
}

const scratch = mkdtempSync(join(tmpdir(), "fossil-corpus-guards-"));
const pristine = { rowgroups: join(scratch, "rowgroups"), files: join(scratch, "files") };

const VERTICES = join("vertex", "Person", "tiles.parquet");
const VERTEX_DIR = join("vertex", "Person");
const EDGE_DIR = join("edge", "Person_knows_Person");

const green = (s) => `\x1b[32m${s}\x1b[0m`;
const red = (s) => `\x1b[31m${s}\x1b[0m`;
const dim = (s) => `\x1b[2m${s}\x1b[0m`;

let failed = 0;
function assert(ok, what, detail = "") {
  console.log(`  ${ok ? green("✓") : red("✗")} ${what}${detail ? dim(` — ${detail}`) : ""}`);
  if (!ok) failed += 1;
}

/** A copy of the conforming corpus, broken by `mutate` and nothing else. */
function broken(layout, mutate) {
  const dir = mkdtempSync(join(scratch, "mutant-"));
  cpSync(pristine[layout], dir, { recursive: true });
  mutate(dir);
  return dir;
}

/**
 * Re-cut one orientation's tiles through a SELECT, **in the container the corpus declares.**
 *
 * The edge mutations used to rewrite `by_source.parquet`, the uncut relation, and that file is not
 * published any more: a corpus carries an orientation one way, as tiles. So they read the tiles as
 * one relation and write them back cut on the column the orientation is ordered by — which also
 * means a mutation aimed at ordering or at content no longer moves a row into the wrong tile by
 * accident, and each one still fails the single guard it names.
 *
 * The container matters and it took two red mutations to notice: writing `tile{k}.parquet` into a
 * corpus whose orientation is one `tiles.parquet` leaves BOTH, which is `mixed` — `declared-tiling`
 * fires, the guard the mutation is aimed at reads the untouched copy, and the break is reported as
 * somebody else's.
 */
function recut(dir, orientation, key, { select = "SELECT * FROM m", order } = {}) {
  const tileRows = 4096;
  const into = join(dir, EDGE_DIR, orientation);
  const other = key === "src_dense" ? "dst_dense" : "src_dense";
  const sorted = `ORDER BY ${order ?? `${key}, ${other}`}`;
  const rowgroups = readFileSync(join(dir, "graph.graph.yml"), "utf8").includes("container: rowgroups");
  const copies = rowgroups
    ? [
        `COPY (SELECT * FROM (${select}) ${sorted})
           TO '${lit(join(into, "tiles.parquet"))}' (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`,
      ]
    : [...Array(Math.ceil(70_000 / tileRows)).keys()].map(
        (k) =>
          `COPY (SELECT * FROM (${select}) WHERE ${key} >= ${k * tileRows} AND ${key} < ${(k + 1) * tileRows}
                  ${sorted})
             TO '${lit(join(into, `tile${k}.parquet`))}' (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`,
      );
  execute(
    `CREATE TEMP TABLE m AS SELECT * FROM read_parquet('${lit(join(into, "*.parquet"))}');
     ${copies.join("\n")}`,
  );
}

/** Rewrite one Parquet file through a SELECT, which is how every mutation below is expressed. */
function rewrite(path, select) {
  execute(
    `CREATE TEMP TABLE m AS SELECT * FROM read_parquet('${lit(path)}');
     COPY (${select}) TO '${lit(path)}' (FORMAT PARQUET, ROW_GROUP_SIZE 4096);`,
  );
}

/**
 * Rewrite the tile-code anchor at `dir`, leaving every row of the corpus alone.
 *
 * The mutation the rest of this file cannot express: every other break goes
 * through a `SELECT` over the payload, and the anchor is not in the payload. It
 * is a document beside it, which is exactly why it can be wrong without anything
 * else noticing.
 */
function reanchor(dir, edit) {
  const path = join(dir, VERTEX_DIR, "codes.json");
  writeFileSync(path, `${JSON.stringify(edit(JSON.parse(readFileSync(path, "utf8"))), null, 2)}\n`);
}

/** Which guards fail on the corpus at `dir`. */
function failing(dir) {
  return runAll(inspect(dir))
    .filter((r) => r.failures.length > 0)
    .map((r) => r.guard.id);
}

// ── 1. non-vacuity ──────────────────────────────────────────────────────────────────────────────

console.log("\nA conforming corpus, in both containers");
for (const [layout, dir] of Object.entries(pristine)) {
  const written = write(dir, { layout });
  const results = runAll(inspect(dir));
  const broke = results.filter((r) => r.failures.length > 0);
  assert(
    broke.length === 0,
    `${layout}: ${results.length} guards pass`,
    `${written.count.toLocaleString("en-US")} vertices · ${written.edges.toLocaleString("en-US")} edges · ` +
      `${written.tiles} tiles${broke.length ? ` · broke: ${broke.map((b) => b.guard.id).join(", ")}` : ""}`,
  );
}

console.log("\nEvery guard declares what it cannot prove");
{
  const ids = new Set(GUARDS.map((g) => g.id));
  assert(ids.size === GUARDS.length, `${GUARDS.length} guards, ${ids.size} distinct ids`);
  const undeclared = GUARDS.filter((g) => !g.proves?.trim() || !g.cannotProve?.trim());
  assert(
    undeclared.length === 0,
    "every guard says what it proves and what it does not",
    undeclared.map((g) => g.id).join(", "),
  );
}

// ── 2. mutation ─────────────────────────────────────────────────────────────────────────────────

/**
 * One break per guard. `layout` picks the container the break is expressible in — a row placed in
 * the wrong tile is only nameable when the tile has a name.
 */
const MUTATIONS = [
  {
    guard: "not-empty",
    what: "a corpus of one tile, which has no boundary to get wrong",
    layout: "rowgroups",
    mutate(dir) {
      rmSync(dir, { recursive: true, force: true });
      write(dir, { count: 4_000, clusters: 32, layout: "rowgroups" });
    },
  },
  {
    guard: "entry-point",
    what: "the index names a per-type manifest that is not on disk",
    layout: "rowgroups",
    mutate: (dir) => rmSync(join(dir, "vertex", "Person.vertex.yml")),
  },
  {
    guard: "plain-parquet",
    what: "the vertex payload has no `y` column",
    layout: "rowgroups",
    mutate: (dir) => rewrite(join(dir, VERTICES), "SELECT * EXCLUDE (y) FROM m ORDER BY dense_id"),
  },
  // Two breaks, because the two ways an address stops being an address are not
  // ones a reader would confuse. A string fails to shift at all and is loud
  // wherever anything touches it; a SIGNED integer shifts perfectly, addresses a
  // real tile, and every other guard here stays green — `dense-ids` reads
  // min/max/count and a BIGINT satisfies all three. The second is the one this
  // guard exists for.
  {
    guard: "addressing-is-unsigned",
    what: "`dense_id` is stored as a string, so nothing can shift it",
    layout: "rowgroups",
    // The rows keep their order and every other column: the subquery sorts on the
    // original numeric column, so the tiling and the Morton order survive and the
    // only thing that changed is the type under the name.
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        "SELECT * REPLACE (dense_id::VARCHAR AS dense_id) FROM (SELECT * FROM m ORDER BY dense_id)",
      ),
  },
  {
    guard: "addressing-is-unsigned",
    what: "`dense_id` is signed, so `>>` sign-extends — and it is the break nothing else here sees",
    layout: "rowgroups",
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        "SELECT * REPLACE (dense_id::BIGINT AS dense_id) FROM (SELECT * FROM m ORDER BY dense_id)",
      ),
  },
  {
    guard: "declared-tiling",
    what: "the manifest declares a tile of 5,000 rows, which no shift addresses",
    layout: "rowgroups",
    mutate(dir) {
      const path = join(dir, "vertex", "Person.vertex.yml");
      writeFileSync(path, readFileSync(path, "utf8").replace("chunk_size: 4096", "chunk_size: 5000"));
    },
  },
  {
    // The break this whole guard exists for, and the one that used to pass. A
    // hole in the middle of the tiling is caught by `tile-of`; the TAIL is
    // caught by nothing, because 17 tiles on disk and 17 tiles declared are the
    // same picture as 18 declared and 17 uploaded. `no-dangling-endpoint` fires
    // as collateral, which is not an accident: closing this sideways through the
    // edge endpoints is exactly what `conformance/writer.mjs` had to do while
    // the manifest carried no count. A corpus with no edges has neither.
    guard: "declared-count",
    what: "the last vertex tile is deleted, so the corpus stops one tile early",
    layout: "files",
    mutate: (dir) => rmSync(join(dir, VERTEX_DIR, "chunk17.parquet")),
  },
  {
    guard: "declared-count",
    what: "the manifest declares a `vertex_count` the tiles do not add up to",
    layout: "rowgroups",
    mutate(dir) {
      const path = join(dir, "vertex", "Person.vertex.yml");
      writeFileSync(
        path,
        readFileSync(path, "utf8").replace("vertex_count: 70000", "vertex_count: 69999"),
      );
    },
  },
  {
    guard: "dense-ids",
    what: "one vertex is deleted, so the ids have a gap",
    layout: "rowgroups",
    mutate: (dir) => rewrite(join(dir, VERTICES), "SELECT * FROM m WHERE dense_id <> 9000 ORDER BY dense_id"),
  },
  {
    guard: "tile-of",
    what: "one vertex sits in the file of the tile next door",
    layout: "files",
    mutate(dir) {
      const chunk0 = join(dir, VERTEX_DIR, "chunk0.parquet");
      const chunk1 = join(dir, VERTEX_DIR, "chunk1.parquet");
      execute(
        `CREATE TEMP TABLE stray AS SELECT * FROM read_parquet('${lit(chunk1)}') WHERE dense_id = 4096;
         CREATE TEMP TABLE zero AS SELECT * FROM read_parquet('${lit(chunk0)}');
         CREATE TEMP TABLE one AS SELECT * FROM read_parquet('${lit(chunk1)}') WHERE dense_id <> 4096;
         COPY (SELECT * FROM zero UNION ALL SELECT * FROM stray ORDER BY dense_id)
           TO '${lit(chunk0)}' (FORMAT PARQUET);
         COPY (SELECT * FROM one ORDER BY dense_id) TO '${lit(chunk1)}' (FORMAT PARQUET);`,
      );
    },
  },
  {
    guard: "published-vectors",
    what: "the vector table claims tile_of(4096) = 0",
    vectors: (table) => {
      const copy = structuredClone(table);
      copy.tile_of.vectors.find((v) => v.dense_id === "4096").tile = "0";
      return copy;
    },
  },
  {
    guard: "published-vectors",
    what: "the vector table gives the row-group container a tile in the filename",
    vectors: (table) => {
      const copy = structuredClone(table);
      // The break a port makes: carrying the file-per-tile composition into the other container.
      // It composes cleanly, and the file it names is not there.
      const row = copy.tile_url.vectors.find((v) => v.container === "rowgroups" && v.tile === "7");
      row.url = `${row.prefix}tiles7.parquet`;
      return copy;
    },
  },
  {
    guard: "morton-order",
    what: "two distant vertices swap positions, so the code falls where the id rises",
    layout: "rowgroups",
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        `SELECT dense_id, subject,
                CASE dense_id WHEN 100 THEN (SELECT x FROM m WHERE dense_id = 60000)
                              WHEN 60000 THEN (SELECT x FROM m WHERE dense_id = 100) ELSE x END AS x,
                CASE dense_id WHEN 100 THEN (SELECT y FROM m WHERE dense_id = 60000)
                              WHEN 60000 THEN (SELECT y FROM m WHERE dense_id = 100) ELSE y END AS y,
                cluster_id
           FROM m ORDER BY dense_id`,
      ),
  },
  {
    guard: "spatial-tiles",
    what: "the ids are renumbered at random — the control the bound was chosen against",
    layout: "rowgroups",
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        // The cast back to UINTEGER is not cosmetic: `row_number()` is a BIGINT, and
        // without it this break is also a signed `dense_id` and fires
        // `addressing-is-unsigned` as well. The control has to change the ORDER and
        // nothing else, or the collateral list stops naming what was broken.
        `SELECT (row_number() OVER (ORDER BY hash(dense_id::BIGINT * 2654435761)) - 1)::UINTEGER AS dense_id,
                subject, x, y, cluster_id FROM m ORDER BY 1`,
      ),
  },
  // Two breaks, because the anchor has two halves and a reader would not confuse
  // them. A wrong CODE puts one tile in the wrong place; a wrong EXTENT moves
  // every one of them, and neither is visible to any other guard here — the rows
  // are untouched, so the corpus counts, tiles, orders and prunes exactly as it
  // did. That is the whole reason this file is published rather than derived.
  {
    guard: "code-anchor",
    what: "one published code is one greater than the row it names",
    layout: "rowgroups",
    mutate: (dir) =>
      reanchor(dir, (doc) => {
        doc.hi[1] += 1;
        return doc;
      }),
  },
  {
    guard: "code-anchor",
    what: "the published extent is widened, so every code is quantised onto a different grid",
    layout: "rowgroups",
    mutate: (dir) =>
      reanchor(dir, (doc) => {
        doc.extent.xhi += 10;
        return doc;
      }),
  },
  {
    guard: "csr-and-csc",
    what: "`by_source` is written in target order",
    layout: "rowgroups",
    mutate: (dir) => recut(dir, "by_source", "src_dense", { order: "dst_dense, src_dense" }),
  },
  {
    guard: "one-relation-twice",
    what: "one edge is missing from `by_target`",
    layout: "rowgroups",
    mutate: (dir) =>
      recut(dir, "by_target", "dst_dense", {
        select:
          "SELECT * FROM m WHERE NOT (src_dense = 0 AND dst_dense = (SELECT min(dst_dense) FROM m WHERE src_dense = 0))",
      }),
  },
  {
    guard: "no-dangling-endpoint",
    what: "one edge points at a `dense_id` no vertex has",
    layout: "rowgroups",
    mutate: (dir) =>
      recut(dir, "by_source", "src_dense", {
        select: "SELECT src_dense, CASE WHEN src_dense = 5 THEN 999999 ELSE dst_dense END AS dst_dense FROM m",
      }),
  },
  {
    guard: "exactly-once",
    what: "the staged single-file vertex Parquet is left beside the tiles",
    layout: "rowgroups",
    mutate: (dir) => cpSync(join(dir, VERTICES), join(dir, "vertex", "Person.parquet")),
  },
  {
    guard: "footer-is-the-index",
    what: "`x` is nested in a struct, so the footer carries a box for `x, v` and none for `x`",
    layout: "rowgroups",
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        "SELECT dense_id, subject, {'v': x} AS x, y, cluster_id FROM m ORDER BY dense_id",
      ),
  },
  {
    guard: "identity-is-the-subject",
    what: "two vertices share a subject IRI",
    layout: "rowgroups",
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        `SELECT dense_id, CASE WHEN dense_id = 7 THEN (SELECT subject FROM m WHERE dense_id = 8) ELSE subject END AS subject,
                x, y, cluster_id FROM m ORDER BY dense_id`,
      ),
  },
  // The target half, broken the two ways an emitter breaks it: not written, and
  // written on the wrong column. Both leave the source half perfect, and a
  // reader that only ever draws never notices either — which is how the in-edge
  // direction went untiled while every guard was green.
  // The index, broken the way that matters: not by losing a row, which a scan
  // would also lose, but by pointing one at the wrong address. A missing index
  // makes a lookup slow; a wrong one makes it CONFIDENT, and the vertex it hands
  // back is a real vertex with the wrong identity.
  {
    guard: "index-agrees-with-the-payload",
    what: "one index row names the wrong address, so a lookup returns a plausible stranger",
    layout: "files",
    mutate: (dir) =>
      rewrite(
        join(dir, VERTEX_DIR, "index", "tile0.parquet"),
        // The FIRST row of this tile by subject order, which is the one row that
        // is certainly in it. `dense_id = 0` was written here first and did not
        // fire: the index is sorted by SUBJECT, so the vertex at address zero is
        // in whichever tile its IRI sorts into, and at fixture scale that is not
        // tile zero. The mutation has to name a row by the index's own order.
        `SELECT subject,
                CASE WHEN subject = (SELECT min(subject) FROM m) THEN dense_id + 1 ELSE dense_id END
                  AS dense_id
           FROM m ORDER BY subject`,
      ),
  },
  {
    guard: "not-empty",
    what: "the `by_target` tiles are not written, so a hop has one direction",
    layout: "rowgroups",
    mutate: (dir) => rmSync(join(dir, EDGE_DIR, "by_target"), { recursive: true, force: true }),
  },
  {
    guard: "declared-privacy",
    what: "one record is moved out of its equivalence class, so the release reaches k=1",
    layout: "rowgroups",
    // The break the guard exists for, and the one a producer actually ships: the
    // manifest's numbers are untouched and the BYTES stop supporting them. A
    // reader that trusted `reached: N` would see nothing.
    mutate: (dir) =>
      rewrite(
        join(dir, VERTICES),
        "SELECT * REPLACE (CASE WHEN dense_id = 0 THEN 1234 ELSE birth_year END AS birth_year) FROM m",
      ),
  },
  {
    guard: "declared-privacy",
    what: "the manifest claims a larger k than the files hold, with the files untouched",
    layout: "rowgroups",
    // The other direction, and the reason `reached` is published beside `k`
    // rather than only `k`: a corpus that merely CLEARS the bar cannot be told
    // apart from one whose producer wrote a number down. Publishing what was
    // reached gives a stranger something to recompute against, and this is that
    // recomputation going red.
    mutate(dir) {
      const path = join(dir, "graph.graph.yml");
      const yaml = readFileSync(path, "utf8");
      const reached = /reached: (\d+)/.exec(yaml);
      writeFileSync(path, yaml.replace(reached[0], `reached: ${Number(reached[1]) + 1}`));
    },
  },
  {
    guard: "tile-of",
    what: "the `by_target` tiles are cut on `src_dense`, which is the source half again",
    // The one break that is only nameable where a tile has a name: `tile-of` asks the row-group
    // container for ascending boxes rather than for an ordinal, and a target half cut on the source
    // column ascends perfectly well.
    layout: "files",
    mutate: (dir) => recut(dir, "by_target", "src_dense"),
  },
];

console.log("\nEvery guard fires when its convention is broken");
const collateral = [];
for (const mutation of MUTATIONS) {
  if (mutation.vectors) {
    const { failures } = checkVectors(mutation.vectors(JSON.parse(readFileSync(new URL("./vectors.json", import.meta.url), "utf8"))));
    assert(failures.length > 0, `${mutation.guard.padEnd(24)} ${mutation.what}`);
    continue;
  }
  const dir = broken(mutation.layout, mutation.mutate);
  const fired = failing(dir);
  const also = fired.filter((id) => id !== mutation.guard);
  assert(fired.includes(mutation.guard), `${mutation.guard.padEnd(24)} ${mutation.what}`, also.length ? `also: ${also.join(", ")}` : "in isolation");
  if (also.length > 0) collateral.push([mutation.guard, also]);
  rmSync(dir, { recursive: true, force: true });
}

{
  // Coverage, not a bijection. Every guard owes at least one break it fires on;
  // a convention with two ways of being broken that a reader would not confuse —
  // an orientation absent against an orientation cut on the wrong column — owes
  // one apiece, and requiring exactly one mutation per guard would have made the
  // second unwritable.
  const mutated = new Set(MUTATIONS.map((m) => m.guard));
  const untested = GUARDS.filter((g) => !mutated.has(g.id)).map((g) => g.id);
  assert(
    untested.length === 0,
    `every one of the ${GUARDS.length} guards has a mutation`,
    untested.length === 0 ? `${MUTATIONS.length} breaks` : `untested: ${untested.join(", ")}`,
  );
}

// ── 3. what this cannot do ──────────────────────────────────────────────────────────────────────

console.log(`\n${dim("What no mutation here can express, and therefore what the guards are untested against:")}`);
for (const line of [
  "A corpus written by a *different* writer. The fixture is one implementation checked against " +
    "itself; a second one is what the conventions exist for and what this cannot stand in for.",
  "Statistics that are present and wrong. DuckDB writes true boxes, so the only way to get a false " +
    "one here is to write it by hand with a Parquet library — which is the dependency this checker " +
    "does not take. `footer-is-the-index` is mutated by removing a box, never by lying in one.",
  "A corpus that does not fit in memory. Every mutation rewrites a file through a SELECT, so the " +
    "largest thing tested is 70,000 vertices, and nothing here says a guard's query stays bounded " +
    "at ten million.",
]) {
  console.log(dim(`  · ${line.replace(/\s+/g, " ")}`));
}

if (collateral.length > 0) {
  console.log(`\n${dim("Conventions that cannot be broken in isolation:")}`);
  for (const [guard, also] of collateral) console.log(dim(`  · ${guard} → ${also.join(", ")}`));
}

rmSync(scratch, { recursive: true, force: true });
console.log(failed === 0 ? green("\nthe guards guard\n") : red(`\n${failed} check(s) failed\n`));
process.exit(failed === 0 ? 0 : 1);
