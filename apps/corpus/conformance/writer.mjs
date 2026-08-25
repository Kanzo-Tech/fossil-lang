#!/usr/bin/env node
/**
 * The writer, in the loop.
 *
 * `verify.mjs` runs two readers against one table and catches them drifting apart. It cannot catch
 * them being **wrong together**: `expected.json` was written by whoever read the conventions last,
 * the corpus under it was written by `guards/fixture.mjs`, and `fossil` was not consulted by either.
 * Two readers agreeing about a corpus no compiler produced is a closed loop with the writer outside
 * it, and a convention both readers copied from the same stale sentence passes green forever.
 *
 * So this harness runs `fossil run`, and points `./reader.mjs` — the *same* reader `verify.mjs`
 * uses, not a third copy — at the bytes that came out.
 *
 *   node conformance/writer.mjs --fossil ../../target/release/fossil
 *   FOSSIL=path/to/fossil node conformance/writer.mjs
 *   node conformance/writer.mjs --corpus path/to/an/already-written/corpus
 *
 * `--corpus` skips the run and checks a tree that is already there. It is how this file is proved
 * red — break one convention in a corpus fossil wrote and the assertion that covers it fires — and
 * it is also the only form that reaches a corpus **some other writer** produced, which is the
 * question a format asks and a test suite usually cannot.
 *
 * Exit `0` when the reader's addresses match what the writer wrote, `1` when they do not, `2` when
 * the harness could not run — no `fossil`, no `duckdb`. **`2` is not a pass.** This is the one check
 * in `apps/corpus/` that needs a third tool, which is why it is here and not in `guards/`: the claim
 * on `guards/README.md` is that the directory runs on `node` and `duckdb` alone, and it stays true.
 *
 * **What it proves.** That the four manifest fields a reader addresses with (`prefix`, `chunk_size`,
 * `aligned_by`, the adjacency `prefix`) name, in the corpus fossil actually emits, the files fossil
 * actually wrote — and that the rows inside each of those files are the `dense_id` range the shift
 * says they are. A change to fossil's tiling that both readers are ignorant of turns this red while
 * `verify.mjs` stays green.
 *
 * **What it cannot prove.** Three things, and each is a real hole rather than a caveat:
 *
 *   - **The stride.** `fossil run` has no `--chunk-size`; `DEFAULT_CHUNK_SIZE` is 4,096 and nothing
 *     on the CLI moves it. So this harness cannot produce the 64-row corpus `conformance/corpus`
 *     uses to catch a reader that hard-codes the shift, and a reader that hard-codes 4,096 passes
 *     here. That case stays with the fixture, and the two harnesses are complements.
 *   - **Anything but one self-edge over one vertex type.** The cross-type case, the CSR-only case
 *     and the unaddressable case are manifests fossil has no program to emit, so they stay as
 *     hand-written manifests under `manifests/`.
 *   - **That the addresses are the *right* addresses.** It compares a reader against a writer. If
 *     both are wrong in the same way the loop is still closed — that is what `expected.json` is
 *     for, and neither harness subsumes the other.
 */

import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join, relative, sep } from "node:path";
import { available, query } from "../guards/duck.mjs";
import { resolve } from "./reader.mjs";

/**
 * Enough people to cross two tile boundaries.
 *
 * A tile is 4,096 rows and every assertion below is about a boundary, which one tile does not have.
 * The last tile is deliberately partial — 1,808 of 4,096 — because a corpus whose vertex count
 * divides the tile size exactly never exercises the tail, and the tail is where an off-by-one in a
 * range predicate lives.
 */
const PEOPLE = 10_000;

const SHAPE = JSON.stringify({
  "@context": "http://www.w3.org/ns/shex.jsonld",
  type: "Schema",
  shapes: [
    {
      type: "ShapeDecl",
      id: "https://example.org/Person",
      shapeExpr: {
        type: "Shape",
        expression: {
          type: "EachOf",
          expressions: [
            {
              type: "TripleConstraint",
              predicate: "https://example.org/name",
              valueExpr: { type: "NodeConstraint", datatype: "http://www.w3.org/2001/XMLSchema#string" },
            },
            {
              type: "TripleConstraint",
              predicate: "https://example.org/knows",
              valueExpr: "https://example.org/Person",
              min: 0,
              max: 1,
            },
          ],
        },
      },
    },
  ],
});

/**
 * One vertex type, one self-edge, both fed from CSV — the smallest program that emits two orderings
 * and three tiles. The edges are a ring, person `i` knows `(i+1) mod n`, so the whole graph is
 * statable in one line of arithmetic.
 */
const PROGRAM = `type { Person } := io.shex("person.shex")

Users := io.csv("users.csv")
Knows := io.csv("knows.csv")

People : Person from Users
    @subject = "https://example.org/person/{Users.id}"
    name = Users.name

Links : Person from Knows
    @subject = "https://example.org/person/{Knows.id}"
    name = Knows.name
    knows = Person(Knows.target)
`;

function writeProgram(dir) {
  const users = ["id,name"];
  const knows = ["id,name,target"];
  for (let i = 0; i < PEOPLE; i += 1) {
    users.push(`${i},person-${i}`);
    knows.push(`${i},person-${i},${(i + 1) % PEOPLE}`);
  }
  writeFileSync(join(dir, "users.csv"), `${users.join("\n")}\n`);
  writeFileSync(join(dir, "knows.csv"), `${knows.join("\n")}\n`);
  writeFileSync(join(dir, "person.shex"), SHAPE);
  writeFileSync(join(dir, "mapping.fossil"), PROGRAM);
}

/** Every `.parquet` under `root`, as dataset-relative forward-slash paths. */
function payloadFiles(root, at = root) {
  const out = [];
  for (const name of readdirSync(at)) {
    const path = join(at, name);
    if (statSync(path).isDirectory()) out.push(...payloadFiles(root, path));
    else if (name.endsWith(".parquet")) out.push(relative(root, path).split(sep).join("/"));
  }
  return out.sort();
}

/**
 * The whole-relation edge files, which the manifest does not address and this harness does not
 * treat as a finding.
 *
 * `fossil-df/src/lib.rs:1731` emits `edge/<dir>/by_source.parquet` and `by_target.parquet` beside
 * the tiled `by_source/tile{k}.parquet` — the same rows, in the row-group container, with the run
 * status naming them and no `adj_lists` entry pointing at them. A reader holding only the manifest
 * cannot reach them, which is why they are excluded here rather than reported as unreachable: they
 * are addressed by something that is not the manifest. The *count* is reported, because a second
 * copy of every edge is a real cost and the number is what makes it arguable.
 */
const UNADDRESSED_BY_DESIGN = /^edge\/[^/]+\/by_(?:source|target)\.parquet$/;

const failures = [];
const notes = [];
const fail = (message) => failures.push(message);

const argv = process.argv.slice(2);
const flag = (name) => {
  const at = argv.indexOf(`--${name}`);
  return at === -1 ? undefined : argv[at + 1];
};
const given = flag("corpus");
const fossil = flag("fossil") ?? process.env.FOSSIL;

if (given === undefined && (!fossil || !existsSync(fossil))) {
  console.error(
    "the writer half needs a `fossil` binary, and 2 is not a pass.\n" +
      "  cargo build --release --bin fossil\n" +
      "  node conformance/writer.mjs --fossil target/release/fossil",
  );
  process.exit(2);
}
if (given !== undefined && !existsSync(given)) {
  console.error(`no such corpus: ${given}`);
  process.exit(2);
}
const duck = available();
if (!duck) {
  console.error("the `duckdb` binary is not on PATH; see https://duckdb.org/docs/installation.");
  process.exit(2);
}

const work = given === undefined ? mkdtempSync(join(tmpdir(), "fossil-conformance-")) : null;
try {
  let dest = given;
  if (work !== null) {
    writeProgram(work);
    dest = join(work, "out");
    const run = spawnSync(fossil, ["run", join(work, "mapping.fossil"), "--dest", `file://${dest}`], {
      encoding: "utf8",
    });
    if (run.status !== 0) {
      console.error(`fossil run exited ${run.status}\n${run.stdout}\n${run.stderr}`);
      process.exit(2);
    }
  }

  const corpus = resolve(dest);
  const composed = new Set();

  for (const vertex of corpus.types) {
    // The tile set comes off the disk here, and that is now a choice rather than the gap it was.
    // `vertex_count` is in the manifest — `tiles = count.div_ceil(chunk_size)` — but this harness
    // is the one that checks the writer against itself, and dividing the writer's own declaration
    // to decide what the writer should have written is the closed loop it exists to break. The
    // declaration is held against the bytes by `guards/check.mjs` (`declared-count`); what is
    // checked here is that every file on disk has an address.
    const onDisk = payloadFiles(dest)
      .filter((p) => p.startsWith(vertex.prefix) && /chunk\d+\.parquet$/.test(p))
      .sort((a, b) => Number(/(\d+)\.parquet$/.exec(a)[1]) - Number(/(\d+)\.parquet$/.exec(b)[1]));
    const addressed = onDisk.map((_, k) => vertex.tileUrl(k));
    for (const url of addressed) composed.add(url);

    if (JSON.stringify(addressed) !== JSON.stringify(onDisk)) {
      fail(`${vertex.type}: the reader composes ${JSON.stringify(addressed)}, disk holds ${JSON.stringify(onDisk)}`);
      continue;
    }

    // The identity index, addressed the same way and for the same reason. It was written by the
    // pass before this harness knew the field existed, and three tiles came back as bytes no URL
    // reached — which is precisely the failure the orphan check below is for, arriving on its own
    // artefact. A reader that cannot compose these has an index it can never open.
    if (vertex.index !== null) {
      const indexOnDisk = payloadFiles(dest)
        .filter((p) => p.startsWith(vertex.index.prefix) && /tile\d+\.parquet$/.test(p))
        .sort((a, b) => Number(/(\d+)\.parquet$/.exec(a)[1]) - Number(/(\d+)\.parquet$/.exec(b)[1]));
      const indexAddressed = indexOnDisk.map((_, k) => vertex.index.tileUrl(k));
      for (const url of indexAddressed) composed.add(url);
      if (JSON.stringify(indexAddressed) !== JSON.stringify(indexOnDisk)) {
        fail(
          `${vertex.type}: the reader composes index ${JSON.stringify(indexAddressed)}, ` +
            `disk holds ${JSON.stringify(indexOnDisk)}`,
        );
      } else if (indexOnDisk.length === 0) {
        fail(`${vertex.type}: the manifest declares an index and the writer emitted no tile`);
      }
    }

    // The assertion the whole harness exists for: the rows fossil put in tile `k` are the
    // `dense_id` range the reader's shift says tile `k` is. A writer that re-tiled and a reader
    // that did not both stay green against a table; they cannot both stay green against this.
    for (const [k, path] of onDisk.entries()) {
      const [row] = query(
        `SELECT min(dense_id) AS lo, max(dense_id) AS hi, count(*) AS n
           FROM read_parquet('${join(dest, path).replace(/'/g, "''")}')`,
      );
      const lo = k * vertex.chunkSize;
      const hi = (k + 1) * vertex.chunkSize;
      if (Number(row.lo) < lo || Number(row.hi) >= hi) {
        fail(`${vertex.type} tile ${k}: holds dense_id ${row.lo}..${row.hi}, not [${lo}, ${hi})`);
      }
      if (Number(row.lo) >> vertex.shift !== k || Number(row.hi) >> vertex.shift !== k) {
        fail(`${vertex.type} tile ${k}: the shift does not name it`);
      }
    }
    notes.push(`${vertex.type}: ${onDisk.length} tile(s) of ${vertex.chunkSize}, shift ${vertex.shift}`);
  }

  for (const edge of corpus.edges) {
    for (const direction of edge.directions) {
      const adjacency = edge.adjacency(direction);
      const onDisk = payloadFiles(dest)
        .filter((p) => p.startsWith(adjacency.prefix) && /tile\d+\.parquet$/.test(p))
        .sort((a, b) => Number(/(\d+)\.parquet$/.exec(a)[1]) - Number(/(\d+)\.parquet$/.exec(b)[1]));
      const addressed = onDisk.map((_, k) => adjacency.tileUrl(k));
      for (const url of addressed) composed.add(url);

      if (JSON.stringify(addressed) !== JSON.stringify(onDisk)) {
        fail(
          `${edge.edgeType}/${direction}: the reader composes ${JSON.stringify(addressed)}, ` +
            `disk holds ${JSON.stringify(onDisk)}`,
        );
        continue;
      }

      // An edge tile is addressed by a *vertex* tile, on the endpoint column `aligned_by` names.
      // A writer that ordered `by_target` on `src_dense` would produce files that compose, exist,
      // and answer the wrong neighbourhood.
      for (const [k, path] of onDisk.entries()) {
        const [row] = query(
          `SELECT min(${adjacency.column}) AS lo, max(${adjacency.column}) AS hi
             FROM read_parquet('${join(dest, path).replace(/'/g, "''")}')`,
        );
        const lo = k * adjacency.chunkSize;
        const hi = (k + 1) * adjacency.chunkSize;
        if (Number(row.lo) < lo || Number(row.hi) >= hi) {
          fail(
            `${edge.edgeType}/${direction} tile ${k}: ${adjacency.column} is ${row.lo}..${row.hi}, ` +
              `not [${lo}, ${hi})`,
          );
        }
      }
      notes.push(`${edge.edgeType}/${direction}: ${onDisk.length} tile(s) on ${adjacency.column}`);
    }
  }

  // A tile missing from the *tail* is the one break the checks above cannot see: the tile set is
  // read off the disk, and a disk holding tiles 0..1 is indistinguishable from a corpus that has
  // two. A hole in the middle fires — the composed set stops matching the sorted disk set — and a
  // truncation does not. Deleting `chunk2.parquet` from a corpus fossil wrote passed this harness
  // until the check below existed.
  //
  // This was the *only* answer while no manifest field carried a count. There is one now, and the
  // guard that reads it back off the bytes is `declared-count` — so this is no longer the format's
  // answer to truncation, it is this harness's, and the two are independent on purpose. The
  // endpoints close it without the count and without trusting the writer twice: every
  // `src_dense`/`dst_dense` in the edge tiles names a vertex, so the largest endpoint has to land
  // inside a vertex tile the reader addressed. It costs one query per orientation and it is the only
  // statement here that relates two payload sets rather than checking one against the manifest.
  // A corpus with no edges has no such statement available, which is what the declared count is for.
  for (const edge of corpus.edges) {
    for (const direction of edge.directions) {
      const adjacency = edge.adjacency(direction);
      const files = payloadFiles(dest).filter((p) => p.startsWith(adjacency.prefix) && /tile\d+\.parquet$/.test(p));
      if (files.length === 0) continue;
      const vertex = corpus.vertexType(direction === "src" ? edge.srcType : edge.dstType);
      const tiles = payloadFiles(dest).filter((p) => p.startsWith(vertex.prefix) && /chunk\d+\.parquet$/.test(p));
      const list = files.map((p) => `'${join(dest, p).replace(/'/g, "''")}'`).join(", ");
      const [row] = query(`SELECT max(${adjacency.column}) AS hi FROM read_parquet([${list}])`);
      const covered = tiles.length * vertex.chunkSize;
      if (Number(row.hi) >= covered) {
        fail(
          `${edge.edgeType}/${direction}: ${adjacency.column} reaches ${row.hi}, and the ` +
            `${tiles.length} addressed ${vertex.type} tile(s) only cover dense_id < ${covered}`,
        );
      }
    }
  }

  // Coverage, which is the half a table cannot state: not "does every URL name a file" but "does
  // every file have a URL". A writer that emits payload no `adj_lists` entry points at has written
  // bytes a reader with only the manifest can never ask for.
  const orphans = payloadFiles(dest).filter((p) => !composed.has(p) && !UNADDRESSED_BY_DESIGN.test(p));
  for (const orphan of orphans) {
    fail(`${orphan} is on disk and no address in the manifest reaches it`);
  }
  const byDesign = payloadFiles(dest).filter((p) => UNADDRESSED_BY_DESIGN.test(p));
  notes.push(
    `${composed.size} address(es) composed and on disk; ` +
      `${byDesign.length} whole-relation file(s) reachable only from the run status`,
  );
} finally {
  if (work !== null) rmSync(work, { recursive: true, force: true });
}

for (const note of notes) console.log(`  ${note}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} disagreement(s) between the reader and the writer:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  process.exit(1);
}
console.log(`\nthe reader addresses the corpus fossil wrote`);
