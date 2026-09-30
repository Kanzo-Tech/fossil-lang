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

import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { execute, lit, query, scalar } from "./duck.mjs";
import { TILE_ROWS, hilbertOf } from "./arithmetic.mjs";
import { partOf, tileSql } from "./inspect.mjs";

/**
 * The base a cell tree declares, in vertices per cell.
 *
 * Sixteen, which is `fossil_sinks::manifest::DEFAULT_VERTICES_PER_CELL` — a power of four, because
 * a cell id is a SHIFT of a `dense_id` and a base that is not one makes the finest rung a division.
 * A declared base and not a computed one: which base a corpus used is a knob, and the manifest is
 * where the answer lives rather than a spec.
 */
const VERTICES_PER_CELL = 16;

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
 * The renumbering, which is the whole of the spatial order: rank every vertex by the Hilbert code of
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
  const coded = points.map((p, index) => ({ ...p, index, code: hilbertOf(p.x, p.y, extent) }));
  coded.sort((a, b) => a.code - b.code || a.index - b.index);
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
/**
 * A bound as the tile manifest prints it: the column's own value, exactly.
 *
 * A `FLOAT` footer prints the shortest decimal that reads back as its `f32`, which is not the
 * `f32` — `0.1` is a smaller number than `0.1f32` — so the value is widened and printed as the
 * double it is, the way fossil's writer prints it. An integral float keeps its `.0`, so a reader
 * that types a bare number by its spelling does not take a coordinate for an integer column.
 */
function bound(type, text) {
  if (type === "FLOAT" || type === "DOUBLE") {
    const value = type === "FLOAT" ? Math.fround(Number(text)) : Number(text);
    return Number.isInteger(value) && Math.abs(value) < 1e16 ? value.toFixed(1) : String(value);
  }
  return String(text);
}

/**
 * The tile manifest of a payload and the adjacencies cut on it, read off the footers that were just
 * written — the only honest source, and the one fossil's writer reads too. One zoom, `z = 0`,
 * because this fixture writes no rung. Keys are sorted, as the Rust writer's maps sort them, and an
 * entry's keys are in the order its structs declare them.
 *
 * `adjacencies` is `[{ entry, zooms }]`: the orientation's identity as the manifest spells it, and
 * its files as {@link tileSql} takes them. An orientation whose row groups straddle two tiles is
 * left out rather than approximated — DuckDB cuts a row-group container by row count, so under
 * `rowgroups` an adjacency's groups do not fall on tile boundaries, and a tile sharing a footer with
 * its neighbour has no statistics of its own. A reader then reads that orientation's tiles without
 * knowing in advance which are empty.
 */
function tileManifest(zooms, adjacencies = []) {
  const entries = (rows) => {
    const tiles = new Map();
    for (const row of rows) {
      const key = `${row.z}/${row.tile}`;
      if (!tiles.has(key)) {
        tiles.set(key, { z: Number(row.z), tile: Number(row.tile), rows: String(row.rows), nulls: [], lower: [], upper: [] });
      }
      const entry = tiles.get(key);
      entry.nulls.push([row.col, String(row.nulls)]);
      if (row.lo !== null) entry.lower.push([row.col, bound(row.type, row.lo)]);
      if (row.hi !== null) entry.upper.push([row.col, bound(row.type, row.hi)]);
    }
    return [...tiles.values()].sort((a, b) => a.z - b.z || a.tile - b.tile);
  };
  const object = (pairs) =>
    `{${pairs
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([k, v]) => `${JSON.stringify(k)}:${v}`)
      .join(",")}}`;
  const spell = (entry) =>
    `{"tile":${entry.tile},"record_count":${entry.rows},"null_value_counts":${object(entry.nulls)},` +
    `"lower_bounds":${object(entry.lower)},"upper_bounds":${object(entry.upper)}}`;
  const byZ = new Map();
  for (const entry of entries(query(tileSql(zooms)))) {
    if (!byZ.has(entry.z)) byZ.set(entry.z, []);
    byZ.get(entry.z).push(spell(entry));
  }
  const cut = [];
  for (const { entry, zooms: files } of adjacencies) {
    const rows = files.length === 0 ? [] : query(tileSql(files));
    if (rows.some((row) => row.straddles === true)) continue;
    cut.push(
      `{"edge_type":${JSON.stringify(entry.edge_type)},"src_type":${JSON.stringify(entry.src_type)},` +
        `"dst_type":${JSON.stringify(entry.dst_type)},"aligned_by":${JSON.stringify(entry.aligned_by)},` +
        `"tiles":[${entries(rows).map(spell).join(",")}]}`,
    );
  }
  const top = Math.max(...byZ.keys());
  const matrices = [...byZ].map(
    ([z, list]) =>
      `{"z":${z},"tiles":[${list.join(",")}]` +
      (z === top && cut.length > 0 ? `,"adjacencies":[${cut.join(",")}]` : "") +
      "}",
  );
  return `{"matrices":[${matrices.join(",")}]}`;
}

export function write(
  dir,
  { count = 70_000, clusters = 256, layout = "rowgroups", chunkSize } = {},
) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, "vertex", "Person"), { recursive: true });
  mkdirSync(join(dir, "vertex", "Person", "index"), { recursive: true });
  mkdirSync(join(dir, "edge", "Person_knows_Person", "by_source"), { recursive: true });
  mkdirSync(join(dir, "edge", "Person_knows_Person", "by_target"), { recursive: true });

  const points = positions(count, clusters);
  const { ordered, denseOf } = renumber(points);

  // Two attribute columns beside the position, so the payload carries more
  // than geometry and a channel has something to draw. They are functions of the
  // PRE-layout index rather than of `dense_id`, so their values are a property
  // of the data and not of the tiling.
  //
  // The grain falls with the corpus: `PAIRS` is ordered coarsest-first and the
  // first entry the corpus can fill `K` deep wins — every combination repeats at
  // least `K` times — so the 70,000-record default lands on `(40, 25)` and 300
  // records on `(8, 5)`. `index % 40` and `index % 25` repeat every
  // `lcm(40, 25)`, which is 200 and not 1,000, and `lcm` below is that.
  const K = 5;
  const PAIRS = [
    [40, 25],
    [8, 5],
    [4, 5],
    [2, 5],
    [1, 1],
  ];
  const lcm = (a, b) => {
    const gcd = (x, y) => (y === 0 ? x : gcd(y, x % y));
    return (a / gcd(a, b)) * b;
  };
  const [years, postcodes] =
    PAIRS.find(([y, pc]) => lcm(y, pc) * K <= count) ?? PAIRS[PAIRS.length - 1];
  const attributes = (index) => [1950 + (index % years), `PC${index % postcodes}`];
  const rows = ordered.map((p, dense) => {
    const [birthYear, postcode] = attributes(p.index);
    return `${dense},https://example.org/person/${p.index},${birthYear},${postcode},${p.x},${p.y},${p.cluster}`;
  });
  const vertexCsv = join(dir, "vertices.csv");
  writeFileSync(
    vertexCsv,
    `dense_id,subject,birth_year,postcode,x,y,cluster_id\n${rows.join("\n")}\n`,
  );

  // The categorical channel's domain, COUNTED. `clusters` is what this fixture was asked for and the number
  // of discs that end up carrying a vertex is what it wrote, and those two part
  // company whenever the count does not fill the last disc. A manifest states
  // what is on disk, so the guard that recounts it off the Parquet is comparing
  // against a measurement rather than against the same formula twice.
  const communities = new Set(ordered.map((p) => p.cluster)).size;

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
  // second table: one table has one sort, the payload's is Hilbert because the
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
        const target = join(edgeDir, orientation, `chunk${k}.parquet`);
        return `COPY (SELECT * FROM e WHERE ${key} >= ${k * tileRows} AND ${key} < ${(k + 1) * tileRows}
                       ORDER BY ${key}, ${other}) TO '${lit(target)}' (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`;
      });
    })
    .join("\n");

  execute(`
    CREATE TEMP TABLE v AS
      SELECT dense_id::UINTEGER AS dense_id, subject::VARCHAR AS subject,
             birth_year::INTEGER AS birth_year, postcode::VARCHAR AS postcode,
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
      // Every projection of this type in one list: the payload, the entry at `scale: 1`.
      "projections:",
      "- path: ''",
      "  scale: 1",
      "  file_type: parquet",
      "  properties:",
      "  - name: subject",
      "    data_type: string",
      "    is_primary: true",
      // `is_primary` is not optional, and leaving it off produced a manifest fossil's own
      // reader refuses: `fossil_sinks::manifest::Property` has no default for it, so
      // `open` failed on this file with «missing field `is_primary`» while every
      // JavaScript reader sailed past. That is the asymmetry the third reader was added
      // to catch, catching something.
      "  - name: birth_year",
      "    data_type: int32",
      "    is_primary: false",
      "  - name: postcode",
      "    data_type: string",
      "    is_primary: false",
      // The identity index, and the ONE artefact here that is not a projection: it is a second
      // ORDER over the same rows, so the spatial cut does not address it and it carries a
      // `chunk_size` of its own instead of a scale.
      "index:",
      "  prefix: index/",
      "  ordered_by: subject",
      `  chunk_size: ${tileRows}`,
      // What a reader can draw this type with, and the one number it cannot recover: how many
      // distinct values the categorical has. A quantitative channel declares no domain — its range
      // is min/max in the footers — so BOTH kinds are written here, because a fixture carrying one
      // of them freezes half the contract and leaves the other half of the guard passing on
      // nothing.
      //
      // Flat mappings, like everything else this writes: `manifest.mjs` reads one level of
      // sequence-of-mappings and SKIPS anything deeper, so a nested channel would be a channel
      // half its readers cannot see.
      "channels:",
      "- name: community",
      "  column: cluster_id",
      "  scale: categorical",
      `  domain: ${communities}`,
      // Present because this writer computed the column. `birth_year` below carries no
      // `derived_by`, which is how a column that came off the source is written.
      "  derived_by: phyllotaxis-grid",
      "- name: age",
      "  column: birth_year",
      "  scale: quantitative",
      // The tree, for the one field of it a reader cannot recover from anywhere else: **which of
      // the channels above a rung's `mode` is the mode of.** A cell row's `mode` is a bare
      // `uint32`, so a reader colouring a rung by it has no palette and no domain until the tree
      // says which entry to resolve — and it is a NAME, so the domain stays measured in one place.
      //
      // `rungs: []` is the honest half. This fixture writes no summary rows: a pyramid is a third
      // artefact with its own aggregation to get right, nothing in this directory opens one, and a
      // declared rung with no tile under it would be a conformance corpus claiming bytes it does
      // not have. A tree with no rungs has no bytes — which is exactly what a writer that
      // published none declares — and what is frozen here is the reference's spelling, which is
      // what `mode-names-a-channel` reads.
      "cells:",
      "  prefix: cell/",
      `  vertices_per_cell: ${VERTICES_PER_CELL}`,
      // The relations the partition would be over, without which the mass a rung conserves is not
      // a defined quantity. Required, and required even of a tree that publishes no rung.
      "  relations:",
      "  - knows",
      "  mode_channel: community",
      "  rungs: []",
      // Where every tile's statistics are, so a reader plans without opening a footer. Written
      // below, off the footers the COPYs above closed.
      "tile_manifest: tile-manifest.json",
      "version: gar/v1",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(vertexPrefix, "tile-manifest.json"),
    tileManifest(
      layout === "files"
        ? Array.from({ length: tiles }, (_, k) => ({
            z: 0,
            path: join(vertexPrefix, `chunk${k}.parquet`),
            tile: k,
          }))
        : [{ z: 0, path: join(vertexPrefix, "tiles.parquet"), tile: null }],
      [
        ["by_source", "src", "src_dense"],
        ["by_target", "dst", "dst_dense"],
      ].map(([orientation, alignedBy, key]) => {
        const entry = { edge_type: "knows", src_type: "Person", dst_type: "Person", aligned_by: alignedBy };
        const at = join(edgeDir, orientation);
        const shift = Math.log2(tileRows);
        const zooms =
          layout === "files"
            ? Array.from({ length: tiles }, (_, k) => join(at, `chunk${k}.parquet`))
                .filter((path) => existsSync(path))
                .map((path) => ({ z: 0, part: partOf(entry), path, tile: Number(/chunk(\d+)/.exec(path)[1]), key, shift }))
            : [{ z: 0, part: partOf(entry), path: join(at, "tiles.parquet"), tile: null, key, shift }];
        return { entry, zooms };
      }),
    ),
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
      "projections:",
      "- path: by_source/",
      "  scale: 1",
      "  aligned_by: src",
      "  ordered: true",
      "  file_type: parquet",
      "  properties: []",
      "- path: by_target/",
      "  scale: 1",
      "  aligned_by: dst",
      "  ordered: true",
      "  file_type: parquet",
      "  properties: []",
      "version: gar/v1",
      "",
    ].join("\n"),
  );

  // `ROW_GROUP_SIZE` is a REQUEST, and `rowgroups` is the container that believes it.
  //
  // DuckDB writes row groups in whole vectors and its vector is 2,048 rows, so anything
  // below that is rounded UP and the writer says nothing. Every line above wrote
  // `chunk_size: ${tileRows}` into three manifests from the number that was ASKED FOR,
  // never from the number on disk — so `--chunk-size 1024` produced a corpus whose
  // manifest declared 1024 and whose row groups held 2,048, and the repo's own `tile-of`
  // guard found 14,643 violations in it. A fixture that can emit a corpus violating the
  // conventions it exists to demonstrate is worse than no fixture.
  //
  // It refuses rather than correcting the manifest to match the bytes. Silently writing a
  // chunk size nobody asked for is the same lie pointed the other way: the caller of a
  // `chunk_size` sweep would get a flat curve and no reason for it, which is precisely how
  // this was found — from the outside, by someone measuring.
  //
  // `files` is unaffected: there the tile is a whole file, so no row-group size addresses it.
  if (layout === "rowgroups" && count > 0) {
    const target = lit(join(vertexPrefix, "tiles.parquet"));
    const onDisk = Number(
      scalar(`SELECT max(row_group_num_rows) FROM parquet_metadata('${target}');`),
    );
    if (onDisk !== tileRows && tiles > 1) {
      throw new Error(
        `chunk_size ${tileRows} was requested and DuckDB wrote row groups of ${onDisk}. ` +
          `Row groups come out in whole 2,048-row vectors, so a smaller chunk_size cannot ` +
          `be honoured — and the manifest would declare a tile no reader could address. ` +
          `Ask for ${onDisk} or larger, or write this corpus as \`layout: "files"\`, where ` +
          `a tile is a file and no row-group size is involved.`,
      );
    }
  }

  return { dir, count, edges: pairs.length, tiles, layout, chunkSize: tileRows, communities };
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
