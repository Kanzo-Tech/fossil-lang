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
 * # Three readers, and none of them is the contract
 *
 * `cases` is executed here against **two** readers and against a third elsewhere, and none of the
 * three wrote the table:
 *
 * | reader | what it is |
 * | --- | --- |
 * | `./reader.mjs` | plain Node, written from the conventions and from nothing else |
 * | `./wasm-reader.mjs` | `fossil_graph::address` compiled to wasm32, through `fossil-graph-wasm` |
 * | `packages/corpus/src/address.ts` | the published module, run by `packages/corpus/tests/conformance.test.ts` |
 *
 * It was two, and both were JavaScript. A mistake they share — a shift taken as signed, a count
 * that went through a `Number` — was invisible to a diff of the two, which is the same shape of
 * blindness `GraphAr`'s fourth implementation landed through: it re-derived the path arithmetic
 * differently from the other three *and* from the corpus on disk, green on both sides, because its
 * tests asserted hand-written strings instead of resolving against a shared table. The Rust reader
 * runs the same table natively in `crates/fossil-graph/tests/conformance.rs`; what runs here is the
 * wasm32 build of it, which is a different claim — `usize` is 64 bits there and 32 here.
 *
 * The wasm leg needs `packages/corpus/pkg/`, which is the one thing under `apps/corpus/` that is not
 * `node` plus a `duckdb` binary. It is therefore **required by default and refused explicitly**:
 * `--without-wasm` is the opt-out, and `.github/workflows/corpus.yml` — which installs `node` and
 * `duckdb` and nothing else — is where it is passed and where the reason is written down. A leg
 * that skipped itself quietly would be a conformance suite passing over nothing.
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
 *   node conformance/verify.mjs                     # both halves, both readers
 *   node conformance/verify.mjs --without-wasm      # a checkout with no Rust toolchain
 *   node conformance/verify.mjs --addressing-only   # a checkout with no `duckdb` binary
 *
 * Exit `0` when every address in the table reproduces for every reader, `1` when one does not.
 */

import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join as pathJoin } from "node:path";
import { fileURLToPath } from "node:url";
import * as answers from "./answers.mjs";
import { resolve } from "./reader.mjs";
import * as wasm from "./wasm-reader.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
/**
 * The two halves this file runs, and the two things a checkout can be missing.
 *
 * Both flags are **explicit refusals, not skips**: without one, the missing half is a failure.
 * A run that quietly does less than it says is the shape of a suite that has stopped being
 * evidence, so a checkout that cannot do something has to say which and CI has to say why.
 *
 * - `--without-wasm` — no Rust toolchain, so no `packages/corpus/pkg/` and no third reader.
 *   `.github/workflows/corpus.yml` passes it: its install list is `node` and a `duckdb` binary,
 *   deliberately, because `guards/` is meant to be copied by somebody who has neither.
 * - `--addressing-only` — no `duckdb` binary, so the `answers` block cannot open a byte.
 *   `.github/workflows/pnpm-ci.yml` passes it: that job builds the wasm and has no DuckDB, and
 *   duplicating `corpus.yml`'s pinned CLI install into it would be one version in two files,
 *   which is the drift this repository keeps deleting.
 *
 * Between the two workflows every half runs against every reader that can run it, and the
 * `answers` half has nothing to gain from the wasm leg in any case: that reader opens no byte.
 */
const WITHOUT_WASM = process.argv.includes("--without-wasm");
const ADDRESSING_ONLY = process.argv.includes("--addressing-only");

// ---------------------------------------------------------------------------

const failures = [];
const notes = [];

/** Which reader the failures and notes being recorded belong to. */
let reader = "reader.mjs";
const fail = (message) => failures.push(`[${reader}] ${message}`);
const note = (message) => notes.push(`[${reader}] ${message}`);

function same(what, got, want) {
  const a = JSON.stringify(got);
  const b = JSON.stringify(want);
  if (a !== b) fail(`${what}: ${a} is not ${b}`);
}

const table = JSON.parse(readFileSync(pathJoin(HERE, "expected.json"), "utf8"));

/**
 * Every case in the table, against one reader.
 *
 * `resolve(root, base)` is the whole of the seam: a reader is a function from a corpus root to
 * addresses, and nothing below knows which language answered. That is also what keeps this
 * indifferent to which engine a reader would open the files with — this block opens none.
 */
function runCases(resolve) {
/**
 * How many LEVEL addresses were composed across the whole table.
 *
 * Its own counter, and it earned one: the `levels` case was deleted from `expected.json` by a
 * `git checkout` of a file that was not yet in the index, and every level assertion in all three
 * harnesses ran over an empty list and stayed green — the six cases with no pyramid carry the
 * other counts. A non-vacuity check shared with them is one that cannot see a section of the
 * table disappear.
 */
let checkedLevels = 0;
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
    } else note(`${label}: refused — ${threw}`);
    continue;
  }

  const corpus = resolve(root);

  // Which container carries the tiles. It is the one thing about a corpus a reader cannot work out
  // — working it out means listing a directory — so it is a manifest field, and it is pinned here
  // rather than inferred from the paths below, which are what it decides.
  same(`${label}: container`, corpus.container, expected.container);

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

  // The written pyramid. A type the table says nothing about must report NONE — a reader that
  // invented a level list would compose `l6/chunk0.parquet` against a corpus that never wrote one,
  // and 404 for a level the predicate over the payload answers perfectly well.
  const pyramids = new Map((expected.levels ?? []).map((l) => [l.type, l]));
  for (const type of corpus.types) {
    const want = pyramids.get(type.type);
    if (want === undefined) {
      if (type.levels !== null) {
        fail(`${label}: ${type.type} reports a pyramid its manifest does not declare`);
      }
      continue;
    }
    const levels = type.levels;
    if (levels === null) {
      fail(`${label}: ${type.type} declares levels ${want.written.join(", ")} and the reader sees none`);
      continue;
    }
    same(
      `${label}: ${type.type} levels`,
      { written: levels.levels, chunk_size: levels.chunkSize },
      { written: want.written, chunk_size: want.chunk_size },
    );
    for (const size of want.sizes ?? []) {
      same(
        `${label}: ${type.type} level ${size.level}`,
        { rows: String(levels.rows(size.level)), tiles: String(levels.tiles(size.level)) },
        { rows: size.rows, tiles: size.tiles },
      );
    }
    for (const v of want.tile_of ?? []) {
      const got = levels.tileOf(v.level, BigInt(v.dense_id));
      if (got !== BigInt(v.tile)) {
        fail(`${label}: level ${v.level} tile_of(${v.dense_id}) = ${got}, not ${v.tile}`);
      }
    }
    for (const address of want.addresses ?? []) {
      const got = levels.tileUrl(address.level, address.tile);
      if (got !== address.path) fail(`${label}: composed ${got}, not ${address.path}`);
      checkedLevels += 1;
    }
    for (const set of want.files ?? []) {
      same(`${label}: level ${set.level} files`, levels.files(set.level), set.paths);
      checkedLevels += 1;
    }
    for (const refused of want.refused ?? []) {
      let threw = null;
      try {
        levels.files(refused.level);
      } catch (error) {
        threw = error.message;
      }
      if (threw === null) {
        fail(`${label}: addressed level ${refused.level}, which this corpus does not write`);
      } else if (!threw.includes(refused.message)) {
        fail(`${label}: refused level ${refused.level} with "${threw}", which does not say "${refused.message}"`);
      }
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

  note(
    `${label}: ${corpus.types.length} type(s), ${corpus.edges.length} edge type(s), ` +
      `${(expected.addresses ?? []).length} address(es)${expected.on_disk ? " checked on disk" : ""}`,
  );
}

if (checkedLevels < 5) {
  fail(`non-vacuity: ${checkedLevels} level address(es) checked, so the table declares no pyramid`);
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
}

// ---------------------------------------------------------------------------
// The readers, run over the same table in turn.
//
// `runCases` is called once per reader and the failures carry which one they came from, so a
// disagreement names the implementation that moved rather than the case that noticed.

runCases(resolve);
let legs = 1;

if (WITHOUT_WASM) {
  reader = "wasm";
  note(
    "not run: --without-wasm. The wasm leg needs `packages/corpus/pkg/`, which needs a Rust " +
      "toolchain; this invocation declared it has none.",
  );
  reader = "reader.mjs";
} else if (!wasm.built()) {
  // Not a skip. A checkout that cannot run a reader says so out loud, or the suite quietly becomes
  // the one leg it started as.
  reader = "wasm";
  fail(
    `wasm: ${wasm.PKG_DIR} holds no build output, so the third reader did not run. ` +
      `Build it with \`${wasm.BUILD_COMMAND}\`, or pass --without-wasm to run the two that need ` +
      `nothing but node.`,
  );
  reader = "reader.mjs";
} else {
  reader = "wasm";
  runCases(wasm.reader(await wasm.load()));
  reader = "reader.mjs";
  legs += 1;
}

// ---------------------------------------------------------------------------
// The four members of the reference API, executed against the same table.
//
// Everything above this line is ADDRESSING: which URL a tile has, and nothing opens a byte. This
// executes `answers`, which is what the API on top of that addressing returns. The numbers in the
// table come from a full scan — no addressing at all — so this asks whether a reader that PRUNES
// lands where an unpruned read already is.
if (ADDRESSING_ONLY) {
  note("answers: not run: --addressing-only. This invocation declared it has no `duckdb` binary.");
} else {
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

  note(
    `answers: ${expected.window.length} window(s), ${expected.node.length} node(s), ` +
      `${expected.neighbours.length} neighbourhood(s) over ${expected.case}`,
  );
}

for (const line of notes) console.log(`  ${line}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} address(es) do not reproduce:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  process.exit(1);
}
// What this file ran, and what it did not. The third reader of the table is
// `packages/corpus/src/address.ts`, executed by `packages/corpus/tests/conformance.test.ts` — a pnpm
// test in another package, so an address moved here turns it red and this file will not tell you.
// Saying so is the point: a line reading "conformance cases reproduce" over one implementation is
// the sentence `GraphAr`'s fourth reader was green under.
console.log(
  `\n${table.cases.length}/${table.cases.length} conformance cases reproduce for ${legs} of the ` +
    `2 readers this file runs` +
    (legs > 1
      ? "; the third is packages/corpus/tests/conformance.test.ts"
      : " — one side is not a diff, and the other two run elsewhere"),
);
