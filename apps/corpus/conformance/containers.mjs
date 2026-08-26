#!/usr/bin/env node
/**
 * One corpus, two containers, the same answers.
 *
 * A tile is a fixed range of `dense_id`; which container carries it is a second question, and the
 * corpus answers it in `graph.graph.yml`'s `container`. This writes the SAME graph both ways —
 * `guards/fixture.mjs` is deterministic, so the two trees hold the same vertices with the same
 * `dense_id` and the same edges — and requires every answer to come back identical.
 *
 *   node conformance/containers.mjs [--vertices 20000]
 *
 * **This is the diff a format change is allowed to arrive behind.** Two implementations move to a
 * new container and are compared before a writer emits one, never after, because after is when the
 * only corpus that exists is the one the new writer produced and there is nothing left to compare
 * against. `verify.mjs` asks whether two readers agree on a table; this asks whether one reader
 * gives one corpus one answer however it is stored.
 *
 * **What it cannot prove.** That the row-group container is cheaper, which is the whole reason for
 * it: the measured win is HTTP requests per window, and a local file makes none. What it does
 * measure is the count that stands in for them — how many distinct files each container has to
 * open to answer the same window — and that is printed rather than asserted, because the ratio is a
 * property of the corpus and not of the format.
 *
 * The published module's half of this is `packages/graph/tests/containers.test.ts`, which asks the
 * same question of `openCorpus`. Neither shares a line with the other.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { available } from "../guards/duck.mjs";
import { write } from "../guards/fixture.mjs";
import * as answers from "./answers.mjs";
import { resolve } from "./reader.mjs";

if (!available()) {
  console.error("the `duckdb` binary is not on PATH. The checker needs it and nothing else.");
  process.exit(2);
}

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at === -1 ? fallback : Number(process.argv[at + 1]);
};

// The default `chunk_size` and not a smaller one, and this is a constraint of the tool rather than
// of the format: DuckDB emits row groups in multiples of its 2,048-row vector, so `ROW_GROUP_SIZE`
// below that is silently clamped and every tile boundary lands somewhere else. A corpus whose
// `chunk_size` is under 2,048 cannot be written in the row-group container by DuckDB at all —
// measured: 300 rows at `ROW_GROUP_SIZE 64` come back as one row group of 300.
const COUNT = flag("vertices", 20_000);
const CLUSTERS = flag("clusters", 64);

const scratch = mkdtempSync(join(tmpdir(), "fossil-containers-"));
const failures = [];
const fail = (message) => failures.push(message);

/** Two answers, compared by value. The whole file is this assertion with different arguments. */
function same(what, files, rowgroups) {
  const a = JSON.stringify(files);
  const b = JSON.stringify(rowgroups);
  if (a !== b) fail(`${what}: files gave ${a}, rowgroups gave ${b}`);
}

const written = {};
for (const layout of ["files", "rowgroups"]) {
  written[layout] = write(join(scratch, layout), { count: COUNT, clusters: CLUSTERS, layout });
}
const root = (layout) => written[layout].dir;

console.log(
  `\n${COUNT.toLocaleString("en-US")} vertices · ${written.files.edges.toLocaleString("en-US")} edges · ` +
    `${written.files.tiles} tiles of ${written.files.chunkSize}\n`,
);

// ── the addressing, which is the half that is SUPPOSED to differ ────────────────────────────────
//
// Same tiling, same shift, same orientations; different URLs, and that is the container. Asserting
// the tiling is identical is what makes the answer comparison below mean something: two corpora
// that disagreed about how many tiles there are would agree on nothing by accident.
{
  const resolved = { files: resolve(root("files")), rowgroups: resolve(root("rowgroups")) };
  // The comparison is worthless if both trees are the same container. Both halves are asserted,
  // because the failure is silent in either direction.
  if (resolved.files.container !== "files") fail("the files corpus does not declare the files container");
  if (resolved.rowgroups.container !== "rowgroups") {
    fail("the rowgroups corpus does not declare the rowgroups container");
  }
  for (const key of ["chunkSize", "shift", "type"]) {
    same(
      `vertex ${key}`,
      resolved.files.types.map((t) => t[key]),
      resolved.rowgroups.types.map((t) => t[key]),
    );
  }
  same(
    "tile_of over the corpus",
    resolved.files.types.map((t) => [0, 1, COUNT - 1].map((d) => String(t.tileOf(BigInt(d))))),
    resolved.rowgroups.types.map((t) => [0, 1, COUNT - 1].map((d) => String(t.tileOf(BigInt(d))))),
  );

  const tiles = [...Array(written.files.tiles).keys()];
  const opened = {};
  for (const layout of ["files", "rowgroups"]) {
    const corpus = resolved[layout];
    const plan = corpus.window({ tiles, directions: ["src", "dst"] });
    opened[layout] = plan.vertex_urls.length + plan.edge_urls.length;
    same(`${layout}: window completeness`, plan.complete, true);
    // Nothing an address composes may repeat: `read_parquet` scans every element of its list, so a
    // URL named twice is a relation counted twice. Under `files` this is free; under `rowgroups` it
    // is the difference between an answer and four copies of one.
    same(
      `${layout}: addresses are distinct`,
      [...plan.vertex_urls, ...plan.edge_urls].length,
      new Set([...plan.vertex_urls, ...plan.edge_urls]).size,
    );
  }
  console.log(
    `  addressing   a window over every tile opens ${opened.files} file(s) as files and ` +
      `${opened.rowgroups} as rowgroups — ${(opened.files / opened.rowgroups).toFixed(1)}×`,
  );
}

// ── the answers, which are the half that must NOT differ ────────────────────────────────────────

// A third of the fixture's own grid — `ceil(sqrt(clusters))` discs at a spacing of 100 — so the
// window holds vertices at any `--vertices` and does not hold all of them. A box that selected
// nothing would compare two empty answers and pass.
const side = Math.ceil(Math.sqrt(CLUSTERS)) * 100;
const box = { x: 0, y: 0, w: side / 3, h: side / 3 };

same(
  "types",
  answers.types(root("files")),
  answers.types(root("rowgroups")),
);

for (const directions of [["src"], ["src", "dst"]]) {
  const found = answers.window(root("files"), COUNT, { ...box, directions });
  // Non-vacuity, and it is not a formality: two windows that selected nothing are identical, and a
  // box wrong by a factor of a hundred is exactly how this file comes to pass on air.
  if (found.vertices === 0 || found.edges === 0 || found.vertices >= COUNT) {
    fail(`window ${directions.join("+")}: ${found.vertices} of ${COUNT} vertices, ${found.edges} edges`);
  }
  same(
    `window ${directions.join("+")}`,
    found,
    answers.window(root("rowgroups"), COUNT, { ...box, directions }),
  );
}

const ids = [0, 1, Math.floor(COUNT / 2), COUNT - 1].map((i) => `https://example.org/person/${i}`);
for (const id of [...ids, "https://example.org/person/nobody"]) {
  same(`node ${id}`, answers.node(root("files"), COUNT, id), answers.node(root("rowgroups"), COUNT, id));
}

for (const depth of [1, 2]) {
  same(
    `neighbours depth ${depth}`,
    answers.neighbours(root("files"), COUNT, ids.slice(0, 2), { depth }),
    answers.neighbours(root("rowgroups"), COUNT, ids.slice(0, 2), { depth }),
  );
}

rmSync(scratch, { recursive: true, force: true });

if (failures.length > 0) {
  console.error(`\n${failures.length} answer(s) depend on the container:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  process.exit(1);
}
console.log("\n  answers      identical in both containers\n");
