/**
 * Where `conformance/corpus/` came from, executed.
 *
 * The corpus the reader's tests open is checked in — a `fossil.json` and six Parquet files. It is
 * generated, and the parameters that generated it are `RECIPE` below. Regenerating with
 * `guards/fixture.mjs`'s defaults gives a corpus two hundred times larger; the recipe is what makes
 * it this one, and this file is what keeps the recipe true.
 *
 *   node conformance/provenance.mjs
 *
 * # What is asserted, and what is only reported
 *
 * **Asserted:** the manifest is identical text, the file set is identical, and every table holds
 * the same rows in the same order. That is the corpus, and it is stable across DuckDB versions.
 *
 * **Reported:** byte-identity of the Parquet files. It holds on the pinned CLI and it is not a
 * requirement, because a writer is free to change an encoding without changing a row — and a check
 * that went red on a DuckDB point release would teach everyone to regenerate the fixture rather
 * than read the diff.
 *
 * # Why this is not `git diff`
 *
 * Regenerating in place and looking at the diff is the same check with the corpus already
 * overwritten. This writes to a temporary directory and leaves the corpus alone. To regenerate it
 * on purpose: `node guards/fixture.mjs conformance/corpus --vertices 300`.
 */

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { write } from "../guards/fixture.mjs";
import { query, lit } from "../guards/duck.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const CORPUS = join(HERE, "corpus");

/**
 * The command that wrote `conformance/corpus/`, as parameters rather than as prose. 300 people is
 * small enough to read by hand; the orders and tags are the fixture's defaults for that count.
 */
export const RECIPE = { count: 300 };

const failures = [];
const notes = [];
const fail = (message) => failures.push(message);

/** Every file under a root, dataset-relative, sorted — the manifests and the payloads apart. */
function tree(root) {
  const all = [];
  for (const entry of readdirSync(root, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile()) continue;
    all.push(relative(root, join(entry.parentPath, entry.name)).split("\\").join("/"));
  }
  all.sort();
  return {
    manifests: all.filter((p) => p.endsWith(".json")),
    payloads: all.filter((p) => p.endsWith(".parquet")),
  };
}

/** Every row of one Parquet, in file order, as a comparable string. */
function rows(path) {
  return query(
    `SELECT * FROM read_parquet('${lit(path)}', file_row_number = true) ORDER BY file_row_number`,
  )
    .map((row) => JSON.stringify(row))
    .join("\n");
}

const scratch = mkdtempSync(join(tmpdir(), "fossil-provenance-"));
try {
  const written = write(join(scratch, "corpus"), RECIPE);
  notes.push(`regenerated: ${written.vertices} vertices · ${written.edges} edges · ${written.tables} tables`);

  const committed = tree(CORPUS);
  const regenerated = tree(written.dir);

  // Non-vacuity first: two empty trees agree about everything, and so do two trees this failed to
  // read. The committed corpus is one manifest and six tables.
  if (committed.manifests.length < 1 || committed.payloads.length < 6) {
    fail(
      `non-vacuity: the committed corpus reads as ${committed.manifests.length} manifest(s) and ` +
        `${committed.payloads.length} payload(s), so the comparison below is over almost nothing`,
    );
  }

  const setOf = (t) => JSON.stringify([...t.manifests, ...t.payloads]);
  if (setOf(committed) !== setOf(regenerated)) {
    fail(
      `the file set differs.\n    committed:   ${setOf(committed)}\n    regenerated: ${setOf(regenerated)}`,
    );
  }

  // The manifest, as text.
  for (const path of committed.manifests) {
    if (!regenerated.manifests.includes(path)) continue;
    const a = readFileSync(join(CORPUS, path), "utf8");
    const b = readFileSync(join(written.dir, path), "utf8");
    if (a !== b) {
      fail(
        `${path} is not what the recipe writes. Either the corpus was made with different ` +
          `parameters than RECIPE records, or the generator has changed under it.`,
      );
    }
  }

  // The payloads, as rows in order. Order is part of the format — every table is written in the
  // order of its key — so a set comparison would pass over a sort that stopped sorting.
  let identicalBytes = 0;
  for (const path of committed.payloads) {
    if (!regenerated.payloads.includes(path)) continue;
    const committedPath = join(CORPUS, path);
    const regeneratedPath = join(written.dir, path);
    if (rows(committedPath) !== rows(regeneratedPath)) {
      fail(`${path} holds different rows, or the same rows in a different order`);
      continue;
    }
    if (readFileSync(committedPath).equals(readFileSync(regeneratedPath))) identicalBytes += 1;
  }

  const version = execFileSync("duckdb", ["-version"], { encoding: "utf8" }).trim().split("\n")[0];
  notes.push(
    `${identicalBytes}/${committed.payloads.length} payload(s) byte-identical on ${version} ` +
      `— reported, not required: an encoding may change without a row changing`,
  );
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

for (const line of notes) console.log(`  ${line}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} disagreement(s) with the recorded recipe:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  console.error(`\nThe recipe is RECIPE in this file: ${JSON.stringify(RECIPE)}.\n`);
  process.exit(1);
}
console.log(`\nconformance/corpus reproduces from ${JSON.stringify(RECIPE)}`);
