/**
 * Where `conformance/corpus/` came from, executed.
 *
 * The corpus every address in `expected.json` resolves against is checked in — twenty Parquet
 * files and three manifests. Nothing said how it was made, and the cost of that showed up as a
 * diagnosis: regenerating with `guards/fixture.mjs`'s defaults gives **600** edges against the
 * **596** the manifest declares, which reads exactly like a fixture that has drifted from its
 * generator. It has not. `clusters` decides how many chords the ring carries and the default is
 * 256; at 16 the generator reproduces this corpus **byte for byte**, every file, both manifests
 * included. The four edges are the parameter, not a drift.
 *
 *   node conformance/provenance.mjs
 *
 * # What is asserted, and what is only reported
 *
 * **Asserted:** the manifests are identical text, the file set is identical, and every payload
 * holds the same rows in the same order. That is the corpus, and it is stable across DuckDB
 * versions.
 *
 * **Reported:** byte-identity of the Parquet files. It holds on the pinned CLI and it is not a
 * requirement, because a writer is free to change an encoding without changing a row — and a check
 * that went red on a DuckDB point release would teach everyone to regenerate the fixture rather
 * than read the diff, which is how the recorded parameters would get lost a second time.
 *
 * # Why this is not `git diff`
 *
 * Regenerating in place and looking at the diff is the same check with the corpus already
 * overwritten, which is the state nobody wants to be in when the answer is "the parameters were
 * right and the default is not". This writes to a temporary directory and leaves the corpus alone.
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
 * The command that wrote `conformance/corpus/`, as parameters rather than as prose.
 *
 * `clusters: 16` is the one that is not a default and the one the whole file is about. `count: 300`
 * is five tiles of 64 with the last deliberately partial (44 rows), because a corpus whose count
 * divides its tile size never exercises a tail; `chunkSize: 64` is small enough to read by hand and
 * a power of two, which is what the conventions require and 4,096 is only the measured default of.
 */
export const RECIPE = { count: 300, clusters: 16, layout: "files", chunkSize: 64 };

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
    manifests: all.filter((p) => p.endsWith(".yml")),
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
  notes.push(
    `regenerated: ${written.count} vertices · ${written.edges} edges · ${written.tiles} tiles · ` +
      `${written.layout}, chunk_size ${written.chunkSize}`,
  );

  const committed = tree(CORPUS);
  const regenerated = tree(written.dir);

  // Non-vacuity first: two empty trees agree about everything, and so do two trees this failed to
  // read. The committed corpus is 3 manifests and 20 payloads — five vertex tiles, five index
  // five of each adjacency orientation — and the numbers are here so a walker that stopped
  // descending is a failure rather than a smaller success.
  if (committed.manifests.length < 3 || committed.payloads.length < 20) {
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

  // The manifests, as text. This is where 596 lives, and it is the whole of the "drift".
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

  // The payloads, as rows in order. Order is part of the format — `by_source` is CSR and
  // `by_target` is CSC — so a set comparison would pass over a sort that stopped sorting.
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
  console.error(
    `\nThe recipe is RECIPE in this file: ${JSON.stringify(RECIPE)}. Regenerating with ` +
      `guards/fixture.mjs's DEFAULTS gives 600 edges rather than 596, which is the parameter and ` +
      `not a drift — see the note at the top before concluding the fixture is stale.\n`,
  );
  process.exit(1);
}
console.log(`\nconformance/corpus reproduces from ${JSON.stringify(RECIPE)}`);
