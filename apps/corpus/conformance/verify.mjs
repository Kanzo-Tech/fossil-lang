/**
 * The conformance corpus, executed.
 *
 * `expected.json` is two tables. `cases` is the ADDRESSING: what a reader must compose from a
 * manifest and what it must refuse to compose, and nothing in it opens a byte. `answers` is what
 * the reference API returns over the one case that has bytes — the four members of `openCorpus`,
 * whose numbers come from a **full scan**, every tile read with no addressing at all and filtered
 * in SQL. That is a third thing neither implementation wrote, and it is what makes the block worth
 * having: both readers PRUNE, and both must land where an unpruned read already is.
 *
 * This file is one of the two implementations that execute it. The other is
 * `packages/graph/tests/conformance.test.ts`, which runs `resolveCorpus` — the published module —
 * against the same table. Neither wrote it, and a change to either that moves an address moves it
 * away from the other.
 *
 * The `answers` half is executed here through `./answers.mjs` and, on the published side, by
 * `openCorpus` itself. The first bug it caught was in this side: an adjacency tile filtered by
 * `src_dense OR dst_dense` rather than by the column the orientation is aligned on, which read
 * **153** edges where the scan says **152** — edges whose source was outside the window, dragged in
 * by an `OR` that answered a question nobody asked.
 *
 * The reader itself is `./reader.mjs` — written from the conventions and from nothing else, and
 * sharing with `resolveCorpus` only the fact that both read the same four manifest fields. This
 * file is a *harness* over it, and it is one of two: `writer.mjs` points the same reader at a
 * corpus `fossil run` has just written, which is the question a table cannot ask, because a table
 * is written by whoever read the conventions last and the writer never sees it.
 *
 *   node conformance/verify.mjs
 *
 * Exit `0` when every address in the table reproduces, `1` when one does not.
 */

import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join as pathJoin } from "node:path";
import { fileURLToPath } from "node:url";
import * as answers from "./answers.mjs";
import { resolve } from "./reader.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

// ---------------------------------------------------------------------------

const failures = [];
const notes = [];
const fail = (message) => failures.push(message);

function same(what, got, want) {
  const a = JSON.stringify(got);
  const b = JSON.stringify(want);
  if (a !== b) fail(`${what}: ${a} is not ${b}`);
}

const table = JSON.parse(readFileSync(pathJoin(HERE, "expected.json"), "utf8"));

for (const expected of table.cases) {
  const root = pathJoin(HERE, expected.root);
  const label = expected.name;

  if (expected.resolve_throws) {
    let threw = null;
    try {
      resolve(root);
    } catch (error) {
      threw = error.message;
    }
    if (threw === null) fail(`${label}: resolved a manifest that addresses nothing`);
    else if (!threw.includes(expected.resolve_throws)) {
      fail(`${label}: threw "${threw}", which does not say "${expected.resolve_throws}"`);
    } else notes.push(`${label}: refused — ${threw}`);
    continue;
  }

  const corpus = resolve(root);

  same(
    `${label}: vertex types`,
    corpus.types.map((t) => ({
      type: t.type,
      prefix: t.prefix,
      chunk_size: t.chunkSize,
      shift: t.shift,
    })),
    expected.types.map((t) => ({
      type: t.type,
      prefix: t.prefix,
      chunk_size: t.chunk_size,
      shift: t.shift,
    })),
  );

  same(
    `${label}: edge types`,
    corpus.edges.map((e) => ({
      edge_type: e.edgeType,
      src_type: e.srcType,
      dst_type: e.dstType,
      prefix: e.prefix,
      directions: e.directions,
      adjacencies: e.directions.map((d) => {
        const a = e.adjacency(d);
        return { direction: d, prefix: a.prefix, column: a.column, chunk_size: a.chunkSize, shift: a.shift };
      }),
    })),
    expected.edges.map((e) => ({
      edge_type: e.edge_type,
      src_type: e.src_type,
      dst_type: e.dst_type,
      prefix: e.prefix,
      directions: e.directions,
      adjacencies: e.adjacencies,
    })),
  );

  for (const v of expected.tile_of ?? []) {
    const got = corpus.vertexType(v.type).tileOf(BigInt(v.dense_id));
    if (got !== BigInt(v.tile)) fail(`${label}: tile_of(${v.dense_id}) in ${v.type} = ${got}, not ${v.tile}`);
  }

  for (const address of expected.addresses ?? []) {
    const got =
      address.kind === "vertex"
        ? corpus.vertexType(address.type).tileUrl(address.tile)
        : corpus.edges
            .find((e) => e.edgeType === address.edge_type)
            ?.adjacency(address.direction)
            ?.tileUrl(address.tile);
    if (got !== address.path) fail(`${label}: composed ${got}, not ${address.path}`);
    // The whole point, on the one case that has bytes: a composed URL names a file that is there.
    if (expected.on_disk && got !== undefined && !existsSync(pathJoin(root, got))) {
      fail(`${label}: ${got} composes and is not on disk`);
    }
  }

  for (const refused of expected.refused ?? []) {
    const got = corpus.edges.find((e) => e.edgeType === refused.edge_type)?.adjacency(refused.direction);
    if (got !== null) {
      fail(`${label}: ${refused.edge_type} handed back an address for ${refused.direction}, which it does not publish`);
    }
  }

  for (const [index, expectation] of (expected.windows ?? []).entries()) {
    const got = corpus.window({
      type: expectation.type,
      tiles: expectation.tiles,
      directions: expectation.directions,
    });
    same(`${label}: window ${index}`, got, {
      vertex_urls: expectation.vertex_urls,
      edge_urls: expectation.edge_urls,
      complete: expectation.complete,
      gaps: expectation.gaps,
    });
  }

  for (const expectation of expected.throws ?? []) {
    let threw = null;
    try {
      corpus.vertexType(expectation.vertex_type);
    } catch (error) {
      threw = error.message;
    }
    if (threw === null || !threw.includes(expectation.message)) {
      fail(`${label}: naming vertex type ${expectation.vertex_type} did not say "${expectation.message}"`);
    }
  }

  notes.push(
    `${label}: ${corpus.types.length} type(s), ${corpus.edges.length} edge type(s), ` +
      `${(expected.addresses ?? []).length} address(es)${expected.on_disk ? " checked on disk" : ""}`,
  );
}

// The base is prepended and nothing else happens to it.
{
  const { base, path, url, case: name } = table.base_join;
  const target = table.cases.find((c) => c.name === name);
  const corpus = resolve(pathJoin(HERE, target.root), base);
  const got = corpus.types
    .flatMap((t) => [t.tileUrl(3)])
    .find((u) => u.endsWith(path.slice(path.lastIndexOf("/") + 1)));
  if (got !== url) fail(`base_join: composed ${got}, not ${url}`);
}

// ---------------------------------------------------------------------------
// The four members of the reference API, executed against the same table.
//
// Everything above this line is ADDRESSING: which URL a tile has, and nothing opens a byte. This
// executes `answers`, which is what the API on top of that addressing returns. The numbers in the
// table come from a full scan — no addressing at all — so this asks whether a reader that PRUNES
// lands where an unpruned read already is.
{
  const expected = table.answers;
  const target = table.cases.find((c) => c.name === expected.case);
  const root = pathJoin(HERE, target.root);
  const count = expected.vertex_count;
  const at = (what) => `answers.${what}`;

  const types = answers.types(root);
  same(at("types"), types, { vertices: expected.types.vertices, edges: expected.types.edges });

  for (const [i, want] of expected.window.entries()) {
    const got = answers.window(root, count, { ...want.box, directions: want.directions });
    same(at(`window[${i}]`), got, {
      type: expected.types.vertices[0].type,
      vertices: want.vertices,
      tiles: want.tiles,
      edges: want.edges,
      complete: want.complete,
      gaps: want.gaps,
    });
  }

  for (const [i, want] of expected.node.entries()) {
    const got = answers.node(root, count, want.id);
    if (!want.found) {
      if (got !== null) fail(`${at(`node[${i}]`)}: ${want.id} resolved to ${JSON.stringify(got)}`);
    } else if (got === null) {
      fail(`${at(`node[${i}]`)}: ${want.id} resolved to nothing`);
    } else {
      same(at(`node[${i}].id`), got.id, want.id);
      same(at(`node[${i}].dense_id`), got.dense_id, want.dense_id);
    }
  }

  // The index is an optimisation, not a second truth: strip the declaration and the same id has
  // to come back as the same vertex. Nothing else here can see which route ran.
  {
    const stripped = mkdtempSync(pathJoin(tmpdir(), "fossil-noindex-"));
    cpSync(root, stripped, { recursive: true });
    const yml = pathJoin(stripped, "vertex", "Person.vertex.yml");
    writeFileSync(
      yml,
      readFileSync(yml, "utf8").replace(/^index:\n(?: {2}.*\n)*/m, ""),
    );
    // The comparison is worthless if the strip did not strip. Both halves are asserted, because
    // the failure is silent in either direction: an index that survived compares the seek with
    // itself, and one that was never there compares two scans.
    const seeks = resolve(root).vertexType().index !== null;
    const scans = resolve(stripped).vertexType().index === null;
    if (!seeks) fail(`${at("index")}: the corpus declares no index, so there is no seek to compare`);
    if (!scans) fail(`${at("index")}: stripping the manifest left an index behind`);
    if (seeks !== expected.index.indexed) {
      fail(`${at("index")}: the corpus is ${seeks ? "" : "not "}indexed and the table says otherwise`);
    }

    for (const id of expected.index.same_either_way) {
      const withIndex = answers.node(root, count, id);
      const withoutIndex = answers.node(stripped, count, id);
      same(at(`index.same_either_way[${id}]`), withoutIndex, withIndex);
      if (withIndex === null) fail(`${at("index")}: ${id} resolves to nothing either way`);
    }
    rmSync(stripped, { recursive: true, force: true });
  }

  for (const [i, want] of expected.neighbours.entries()) {
    const got = answers.neighbours(root, count, want.ids, { depth: want.depth });
    same(at(`neighbours[${i}]`), got, {
      type: expected.types.vertices[0].type,
      depth: want.depth,
      seeds: want.seeds,
      missing: want.missing,
      vertices: want.vertices,
      edges: want.edges,
      frontier: want.frontier,
      complete: want.complete,
    });
  }

  notes.push(
    `answers: ${expected.window.length} window(s), ${expected.node.length} node(s), ` +
      `${expected.neighbours.length} neighbourhood(s) over ${expected.case}`,
  );
}

for (const note of notes) console.log(`  ${note}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} address(es) do not reproduce:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  process.exit(1);
}
console.log(`\n${table.cases.length}/${table.cases.length} conformance cases reproduce`);
