/**
 * A conforming corpus, written by something that is not fossil.
 *
 * This is the checker's own evidence. A guard suite with no corpus to run on is a set of sentences;
 * one that only ever runs on the corpus its authors wrote is a set of sentences about themselves.
 * So the fixture is written here, in JavaScript, against the published conventions and nothing else
 * — no Rust, no `@fossil-lang/*`, and no import from anything in this repository except the
 * arithmetic module, which exists to be copied.
 *
 * That makes it the second implementation the conventions ask for, at fixture scale. It is also
 * what `self-test.mjs` mutates: every guard is proved to fire by breaking exactly one convention in
 * a corpus that otherwise satisfies all of them.
 *
 * **What it is not.** It is not a benchmark and not a realistic graph — the positions come from a
 * grid of phyllotactic discs because that is a shape with real clustering and no dependencies, not
 * because a corpus has to look like that. Nothing here is normative. The conventions are.
 *
 *   node guards/fixture.mjs <dir> [--vertices 70000] [--layout rowgroups|files] [--chunk-size 4096]
 *
 * `--chunk-size` is here because 4,096 is a measured trade-off between requests and bytes on a
 * corpus of millions and not an invariant — the conventions say a power of two, and a corpus that
 * has to be small enough to read by hand declares a smaller one and is addressed identically.
 */

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { execute, lit } from "./duck.mjs";
import { TILE_ROWS, mortonOf } from "./arithmetic.mjs";

const GOLDEN_ANGLE = 2.3999632;
const CLUSTER_SPACING = 100;
const INTRA_CLUSTER_RADIUS = 12;

/** Positions: `clusters` discs on a square grid, phyllotaxis-packed inside each. */
function positions(count, clusters) {
  const side = Math.ceil(Math.sqrt(clusters));
  const per = Math.ceil(count / clusters);
  const out = [];
  for (let i = 0; i < count; i += 1) {
    const cluster = Math.floor(i / per);
    const k = i % per;
    const radius = INTRA_CLUSTER_RADIUS * Math.sqrt(k);
    const angle = GOLDEN_ANGLE * k;
    out.push({
      cluster,
      x: (cluster % side) * CLUSTER_SPACING + radius * Math.cos(angle),
      y: Math.floor(cluster / side) * CLUSTER_SPACING + radius * Math.sin(angle),
    });
  }
  return out;
}

/**
 * The renumbering, which is the whole of the spatial order: rank every vertex by the Morton code of
 * its quantised position and let that rank *be* its `dense_id`. Ties break on the pre-layout index,
 * so the ranking is total and the same input always produces the same corpus.
 */
function renumber(points) {
  // Folded rather than spread: `Math.min(...a)` passes one argument per point,
  // and a corpus large enough to be worth measuring on overflows the call stack
  // before it overflows anything else. It did, at a million.
  const extent = { minX: Infinity, maxX: -Infinity, minY: Infinity, maxY: -Infinity };
  for (const p of points) {
    if (p.x < extent.minX) extent.minX = p.x;
    if (p.x > extent.maxX) extent.maxX = p.x;
    if (p.y < extent.minY) extent.minY = p.y;
    if (p.y > extent.maxY) extent.maxY = p.y;
  }
  const coded = points.map((p, index) => ({ ...p, index, morton: mortonOf(p.x, p.y, extent) }));
  coded.sort((a, b) => a.morton - b.morton || a.index - b.index);
  const denseOf = new Array(points.length);
  coded.forEach((p, dense) => {
    denseOf[p.index] = dense;
  });
  return { ordered: coded, denseOf };
}

/**
 * Write a corpus of `count` vertices of one type with one self-edge type.
 *
 * The edges are a **ring** — vertex `i` knows `(i+1) mod n` in the pre-layout numbering — plus one
 * chord per vertex inside its own cluster. The ring makes the whole graph statable in one line of
 * arithmetic; the chords make the adjacency non-trivial across tiles.
 */
export function write(dir, { count = 70_000, clusters = 256, layout = "rowgroups", chunkSize } = {}) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, "vertex", "Person"), { recursive: true });
  mkdirSync(join(dir, "vertex", "Person", "index"), { recursive: true });
  mkdirSync(join(dir, "edge", "Person_knows_Person", "by_source"), { recursive: true });
  mkdirSync(join(dir, "edge", "Person_knows_Person", "by_target"), { recursive: true });

  const points = positions(count, clusters);
  const { ordered, denseOf } = renumber(points);

  const rows = ordered.map(
    (p, dense) => `${dense},https://example.org/person/${p.index},${p.x},${p.y},${p.cluster}`,
  );
  const vertexCsv = join(dir, "vertices.csv");
  writeFileSync(vertexCsv, `dense_id,subject,x,y,cluster_id\n${rows.join("\n")}\n`);

  const per = Math.ceil(count / clusters);
  const pairs = [];
  for (let i = 0; i < count; i += 1) {
    pairs.push([denseOf[i], denseOf[(i + 1) % count]]);
    const chord = Math.floor(i / per) * per + ((i + 7) % per);
    if (chord < count && chord !== i) pairs.push([denseOf[i], denseOf[chord]]);
  }
  const edgeCsv = join(dir, "edges.csv");
  writeFileSync(edgeCsv, `src_dense,dst_dense\n${pairs.map((p) => p.join(",")).join("\n")}\n`);

  const tileRows = chunkSize === undefined ? Number(TILE_ROWS) : Number(chunkSize);
  if (!Number.isInteger(tileRows) || tileRows <= 0 || (tileRows & (tileRows - 1)) !== 0) {
    throw new Error(`chunk_size ${tileRows} is not a power of two, so no shift addresses it`);
  }
  const tiles = Math.ceil(count / tileRows);
  const vertexPrefix = join(dir, "vertex", "Person");
  const edgeDir = join(dir, "edge", "Person_knows_Person");

  const vertexCopy =
    layout === "files"
      ? Array.from(
          { length: tiles },
          (_, k) =>
            `COPY (SELECT * FROM v WHERE dense_id >= ${k * tileRows} AND dense_id < ${(k + 1) * tileRows}
                    ORDER BY dense_id) TO '${lit(join(vertexPrefix, `chunk${k}.parquet`))}' (FORMAT PARQUET);`,
        ).join("\n")
      : `COPY (SELECT * FROM v ORDER BY dense_id) TO '${lit(join(vertexPrefix, "tiles.parquet"))}'
           (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`;

  // The identity index: the SAME rows a second time, ordered by `subject` instead
  // of by position, carrying only the identity and the address it maps to.
  //
  // It cannot be a column of the payload, and that is the whole reason it is a
  // second table: one table has one sort, the payload's is Morton because the
  // spatial order IS the id space, and a lookup by identity needs the other one.
  // Tiled with the same arithmetic in whichever container the corpus declares —
  // an index tile is a fixed slice of a total order, so a row group of
  // `chunk_size` rows IS tile `k`, exactly as it is for the payload.
  const indexPrefix = join(vertexPrefix, "index");
  const indexCopy =
    layout === "files"
      ? Array.from(
          { length: tiles },
          (_, k) =>
            `COPY (SELECT subject, dense_id FROM v ORDER BY subject
                    LIMIT ${tileRows} OFFSET ${k * tileRows})
               TO '${lit(join(indexPrefix, `tile${k}.parquet`))}' (FORMAT PARQUET);`,
        ).join("\n")
      : `COPY (SELECT subject, dense_id FROM v ORDER BY subject)
           TO '${lit(join(indexPrefix, "tiles.parquet"))}' (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`;

  // Both orientations tiled, each on the column it is ordered by: the out-edges
  // of a vertex are in the `by_source` tile its id names and the in-edges in the
  // `by_target` one, and a fixture that only wrote the source half would leave
  // every target-half guard passing on nothing.
  //
  // And ONLY tiled. `by_source.parquet` beside `by_source/` was the uncut relation published next
  // to its own cut, which is what a `<Type>.parquet` beside the vertex tiles already is — and that
  // one has been a violation for as long as `exactly-once` has existed, in as many words, "a second
  // copy of every vertex". The asymmetry was never argued, and it is the difference between a
  // staging artefact that gets deleted and one that ships: two containers is two places a reader
  // can look, and two readers here looked in different ones.
  //
  // In the row-group container an orientation is one file, and its tiles are runs of row groups
  // rather than one row group each: a vertex tile is exactly `chunk_size` gapless rows, and an
  // adjacency tile is however many edges those vertices happen to have. A writer cannot make the
  // ordinal the address here — a vertex with no out-edges contributes no row, and a tile whose
  // vertices have none contributes no row group to be numbered. So the file is written in key
  // order at `chunk_size` rows per group and the footer's box on the key column is what locates a
  // tile, which is the general rule the vertex payload satisfies by being fixed-stride.
  const edgeTileCopy = ["by_source", "by_target"]
    .flatMap((orientation) => {
      const [key, other] = orientation === "by_source" ? ["src_dense", "dst_dense"] : ["dst_dense", "src_dense"];
      if (layout !== "files") {
        return [
          `COPY (SELECT * FROM e ORDER BY ${key}, ${other})
             TO '${lit(join(edgeDir, orientation, "tiles.parquet"))}'
             (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`,
        ];
      }
      return Array.from({ length: tiles }, (_, k) => {
        const target = join(edgeDir, orientation, `tile${k}.parquet`);
        return `COPY (SELECT * FROM e WHERE ${key} >= ${k * tileRows} AND ${key} < ${(k + 1) * tileRows}
                       ORDER BY ${key}, ${other}) TO '${lit(target)}' (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`;
      });
    })
    .join("\n");

  execute(`
    CREATE TEMP TABLE v AS
      SELECT dense_id::UINTEGER AS dense_id, subject::VARCHAR AS subject,
             x::FLOAT AS x, y::FLOAT AS y, cluster_id::UINTEGER AS cluster_id
        FROM read_csv('${lit(vertexCsv)}', header = true);
    CREATE TEMP TABLE e AS
      SELECT src_dense::UINTEGER AS src_dense, dst_dense::UINTEGER AS dst_dense
        FROM read_csv('${lit(edgeCsv)}', header = true);
    ${vertexCopy}
    ${indexCopy}
    ${edgeTileCopy}
  `);

  rmSync(vertexCsv);
  rmSync(edgeCsv);

  writeFileSync(
    join(dir, "graph.graph.yml"),
    [
      "name: graph",
      "prefix: ''",
      // Which container carries every payload set of this corpus. A reader has no
      // directory to list, so the one thing it cannot derive is which of the two
      // it is looking at, and this is where it is told.
      `container: ${layout}`,
      "vertices:",
      "- vertex/Person.vertex.yml",
      "edges:",
      "- edge/Person_knows_Person/Person_knows_Person.edge.yml",
      "version: gar/v1",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(dir, "vertex", "Person.vertex.yml"),
    [
      "type: Person",
      // How far the corpus goes, which nothing else on disk says: tiles are
      // addressed and never listed, so a tree one tile short reads clean.
      `vertex_count: ${count}`,
      `chunk_size: ${tileRows}`,
      "prefix: vertex/Person/",
      "property_groups:",
      "- file_type: parquet",
      "  properties:",
      "  - name: subject",
      "    data_type: string",
      "    is_primary: true",
      "index:",
      "  prefix: index/",
      "  ordered_by: subject",
      `  chunk_size: ${tileRows}`,
      "version: gar/v1",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(edgeDir, "Person_knows_Person.edge.yml"),
    [
      "src_type: Person",
      "edge_type: knows",
      "dst_type: Person",
      // One number for both orientations — they are one relation stored twice.
      `edge_count: ${pairs.length}`,
      `chunk_size: ${tileRows}`,
      `src_chunk_size: ${tileRows}`,
      `dst_chunk_size: ${tileRows}`,
      "directed: true",
      "prefix: edge/Person_knows_Person/",
      "adj_lists:",
      "- ordered: true",
      "  aligned_by: src",
      "  prefix: by_source/",
      "  file_type: parquet",
      "- ordered: true",
      "  aligned_by: dst",
      "  prefix: by_target/",
      "  file_type: parquet",
      "property_groups: []",
      "version: gar/v1",
      "",
    ].join("\n"),
  );

  return { dir, count, edges: pairs.length, tiles, layout, chunkSize: tileRows };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [dir] = process.argv.slice(2).filter((a) => !a.startsWith("--"));
  if (!dir) {
    console.error("usage: node guards/fixture.mjs <dir> [--vertices N] [--layout rowgroups|files]");
    process.exit(2);
  }
  const flag = (name, fallback) => {
    const at = process.argv.indexOf(`--${name}`);
    return at === -1 ? fallback : process.argv[at + 1];
  };
  const chunkSize = flag("chunk-size", undefined);
  const written = write(dir, {
    count: Number(flag("vertices", 70_000)),
    clusters: Number(flag("clusters", 256)),
    layout: String(flag("layout", "rowgroups")),
    chunkSize: chunkSize === undefined ? undefined : Number(chunkSize),
  });
  console.log(
    `${written.count} vertices · ${written.edges} edges · ${written.tiles} tiles · ${written.layout} → ${written.dir}`,
  );
}
