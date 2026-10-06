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
 *   1. **Non-vacuity.** A conforming corpus is written and every guard passes on it.
 *   2. **Mutation.** For each guard, a copy of that corpus is broken in exactly one way and the
 *      guard is required to fire. Collateral — other guards firing on the same break — is reported
 *      rather than forbidden: some conventions cannot be violated in isolation, and pretending
 *      otherwise would mean weakening the guards until they could.
 */

import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { available, execute, lit } from "./duck.mjs";
import { write } from "./fixture.mjs";
import { GUARDS, runAll } from "./guards.mjs";
import { inspect } from "./inspect.mjs";
import { ENTRY_POINT } from "./manifest.mjs";

if (!available()) {
  console.error("the `duckdb` binary is not on PATH. The checker needs it and nothing else.");
  process.exit(2);
}

const scratch = mkdtempSync(join(tmpdir(), "fossil-corpus-guards-"));
const pristine = join(scratch, "pristine");

const green = (s) => `\x1b[32m${s}\x1b[0m`;
const red = (s) => `\x1b[31m${s}\x1b[0m`;
const dim = (s) => `\x1b[2m${s}\x1b[0m`;

let failed = 0;
function assert(ok, what, detail = "") {
  console.log(`  ${ok ? green("✓") : red("✗")} ${what}${detail ? dim(` — ${detail}`) : ""}`);
  if (!ok) failed += 1;
}

/** A copy of the conforming corpus, broken by `mutate` and nothing else. */
function broken(mutate) {
  const dir = mkdtempSync(join(scratch, "mutant-"));
  cpSync(pristine, dir, { recursive: true });
  mutate(dir);
  return dir;
}

/** Rewrite one Parquet file through a SELECT over its own rows, called `m`. */
function rewrite(path, select) {
  execute(
    `CREATE TEMP TABLE m AS SELECT * FROM read_parquet('${lit(path)}');
     COPY (${select}) TO '${lit(path)}' (FORMAT PARQUET);`,
  );
}

/** Edit `fossil.json` in place. */
function manifest(dir, edit) {
  const path = join(dir, ENTRY_POINT);
  const json = JSON.parse(readFileSync(path, "utf8"));
  edit(json);
  writeFileSync(path, JSON.stringify(json, null, 2));
}

/** A Person property of a parsed manifest, by name. */
const property = (m, name) => m.vertex_tables.find((t) => t.name === "Person").properties.find((p) => p.name === name);

/** Which guards fail on the corpus at `dir`. */
function failing(dir) {
  return runAll(inspect(dir))
    .filter((r) => r.failures.length > 0)
    .map((r) => r.guard.id);
}

try {
  // ── 1. non-vacuity ────────────────────────────────────────────────────────────────────────────
  console.log("\nA conforming corpus");
  const written = write(pristine, { count: 5_000 });
  const results = runAll(inspect(pristine));
  const broke = results.filter((r) => r.failures.length > 0);
  assert(
    broke.length === 0,
    `${results.length} guards pass`,
    `${written.vertices.toLocaleString("en-US")} vertices · ${written.edges.toLocaleString("en-US")} edges · ` +
      `${written.tables} tables${broke.length ? ` · broke: ${broke.map((b) => `${b.guard.id}: ${b.failures[0]}`).join("; ")}` : ""}`,
  );

  // ── 2. one mutation per guard ─────────────────────────────────────────────────────────────────
  console.log("\nEvery guard fires on the break it exists for");
  const person = (dir) => join(dir, "vertex", "Person.parquet");
  const knows = (dir) => join(dir, "edge", "Person_knows_Person.parquet");
  const MUTATIONS = [
    ["not-empty", "a table emptied, and declared empty", (dir) => {
      rewrite(join(dir, "edge", "Person_tagged_Tag.parquet"), "SELECT * FROM m LIMIT 0");
      manifest(dir, (m) => (m.edge_tables.find((e) => e.label === "tagged").record_count = 0));
    }],
    ["entry-point", "a named file deleted", (dir) => rmSync(knows(dir))],
    ["entry-point", "a format this reader does not know", (dir) => manifest(dir, (m) => (m.format = "fossil/2"))],
    ["entry-point", "a Parquet file the manifest does not name", (dir) => cpSync(knows(dir), join(dir, "edge", "stray.parquet"))],
    ["plain-parquet", "dense_id widened to BIGINT", (dir) =>
      rewrite(person(dir), "SELECT * REPLACE (dense_id::BIGINT AS dense_id) FROM m ORDER BY dense_id")],
    ["plain-parquet", "a column the entry does not declare", (dir) =>
      rewrite(person(dir), "SELECT *, 1 AS extra FROM m ORDER BY dense_id")],
    ["declared-count", "a record_count one too many", (dir) =>
      manifest(dir, (m) => (m.edge_tables[0].record_count += 1))],
    ["dense-ids", "an id repeated", (dir) =>
      rewrite(person(dir), "SELECT * REPLACE (CASE WHEN dense_id = 1 THEN 0 ELSE dense_id END::UINTEGER AS dense_id) FROM m ORDER BY dense_id")],
    ["dense-ids", "a vertex numbered in another table's range", (dir) => {
      const tag = join(dir, "vertex", "Tag.parquet");
      execute(
        `CREATE TEMP TABLE lo AS SELECT min(dense_id) AS id FROM read_parquet('${lit(tag)}');
         CREATE TEMP TABLE p AS SELECT * REPLACE (CASE WHEN dense_id = 0 THEN (SELECT id FROM lo) ELSE dense_id END::UINTEGER AS dense_id) FROM read_parquet('${lit(person(dir))}');
         CREATE TEMP TABLE t AS SELECT * REPLACE (CASE WHEN dense_id = (SELECT id FROM lo) THEN 0 ELSE dense_id END::UINTEGER AS dense_id) FROM read_parquet('${lit(tag)}');
         COPY (SELECT * FROM p ORDER BY dense_id) TO '${lit(person(dir))}' (FORMAT PARQUET);
         COPY (SELECT * FROM t ORDER BY dense_id) TO '${lit(tag)}' (FORMAT PARQUET);`,
      );
    }],
    ["sorted-by-key", "a table written in reverse", (dir) =>
      rewrite(person(dir), "SELECT * FROM m ORDER BY dense_id DESC")],
    ["sorted-by-key", "an edge table out of (src, dst) order", (dir) =>
      rewrite(knows(dir), "SELECT * FROM m ORDER BY dst, src")],
    ["no-dangling-endpoint", "an endpoint moved onto a vertex of another type", (dir) =>
      rewrite(knows(dir), `SELECT * REPLACE ((SELECT max(dense_id) FROM '${lit(join(dir, "vertex", "Tag.parquet"))}')::UINTEGER AS dst) FROM m WHERE src = (SELECT min(src) FROM m)
                            UNION ALL SELECT * FROM m WHERE src <> (SELECT min(src) FROM m) ORDER BY src, dst`)],
    ["identity-is-the-subject", "two vertices given one subject", (dir) =>
      rewrite(person(dir), "SELECT * REPLACE (CASE WHEN dense_id = 1 THEN (SELECT subject FROM m WHERE dense_id = 0) ELSE subject END AS subject) FROM m ORDER BY dense_id")],
    ["declared-term", "a year column declared xsd:date", (dir) =>
      manifest(dir, (m) => (property(m, "birth_year").datatype = "http://www.w3.org/2001/XMLSchema#date"))],
    ["declared-term", "a postcode column declared an IRI", (dir) =>
      manifest(dir, (m) => (property(m, "postcode").term_type = "http://www.w3.org/ns/r2rml#IRI"))],
  ];

  for (const [id, what, mutate] of MUTATIONS) {
    const dir = broken(mutate);
    const fired = failing(dir);
    const collateral = fired.filter((g) => g !== id);
    assert(fired.includes(id), `${id} fires on ${what}`, collateral.length ? `also: ${collateral.join(", ")}` : "");
  }

  const untested = GUARDS.map((g) => g.id).filter((id) => !MUTATIONS.some(([m]) => m === id));
  assert(untested.length === 0, "every guard has a mutation", untested.join(", "));
  for (const guard of GUARDS) {
    assert(
      guard.proves.trim().length > 0 && guard.cannotProve.trim().length > 0,
      `${guard.id} says what it proves and what it cannot`,
    );
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

console.log(failed === 0 ? `\n${green("the guards fire")}` : `\n${red(`${failed} failed`)}`);
process.exit(failed === 0 ? 0 : 1);
