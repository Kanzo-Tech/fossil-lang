/**
 * The conformance corpus, executed.
 *
 * `expected.json` is a table of addresses: what a reader must compose from a manifest, and what it
 * must refuse to compose. This file is one of the two implementations that execute it. The other is
 * `packages/graph/tests/conformance.test.ts`, which runs `resolveCorpus` — the published module —
 * against the same table. Neither wrote it, and a change to either that moves an address moves it
 * away from the other.
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

import { existsSync, readFileSync } from "node:fs";
import { dirname, join as pathJoin } from "node:path";
import { fileURLToPath } from "node:url";
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

for (const note of notes) console.log(`  ${note}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} address(es) do not reproduce:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  process.exit(1);
}
console.log(`\n${table.cases.length}/${table.cases.length} conformance cases reproduce`);
