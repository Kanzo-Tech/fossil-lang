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
import { execute, lit, scalar } from "./duck.mjs";
import { TILE_ROWS, levelPlan, levelRows, mortonOf } from "./arithmetic.mjs";

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
  // The extent comes back out because it is PUBLISHED now — half of the tile-code
  // anchor, and the half a reader cannot guess. It was computed here and thrown
  // away, which is exactly what fossil's own layout pass did until the anchor
  // existed.
  return { ordered: coded, denseOf, extent };
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
  const { ordered, denseOf, extent } = renumber(points);

  // Two quasi-identifiers, because a corpus with none cannot exercise the
  // convention that a corpus declares what its bytes guarantee — and a guard
  // that only ever runs on a corpus with nothing to protect is a guard that has
  // never been asked a question.
  //
  // They are functions of the PRE-layout index rather than of `dense_id`, so the
  // equivalence classes are a property of the data and not of the tiling, and
  // the smallest class is the same number however the corpus is cut. That is
  // what makes `declared-privacy` and
  // `the_class_is_the_release_and_not_the_tile` the same assertion in two
  // languages.
  //
  // # The grain is chosen, because a fixed one declares a bound small corpora cannot hold
  //
  // This read `1950 + index % 40` and `PC${index % 25}` unconditionally, with a
  // comment claiming 1,000 combinations of `count / 1000` records each. **Both
  // halves were wrong.** `index % 40` and `index % 25` do not range over 40×25
  // pairs: they repeat every `lcm(40, 25)`, and `gcd` is 5, so there are **200**
  // classes and never were a thousand. And at the conformance corpus's 300
  // records, 200 classes hold one or two records apiece — so the manifest
  // declared `k: 5` over files that reach `k: 1`, and `declared-privacy` said so.
  //
  // The grain now falls with the corpus. `PAIRS` is ordered coarsest-first and
  // the first entry whose class count the corpus can fill `K` deep wins, so the
  // 70,000-record default still lands on `(40, 25)` and writes the same bytes it
  // always did, while 300 records land on `(8, 5)` — 40 classes of seven or
  // eight. A fixture that emits a corpus violating the convention it exists to
  // demonstrate is worse than no fixture.
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
  const quasi = (index) => [1950 + (index % years), `PC${index % postcodes}`];
  const rows = ordered.map((p, dense) => {
    const [birthYear, postcode] = quasi(p.index);
    return `${dense},https://example.org/person/${p.index},${birthYear},${postcode},${p.x},${p.y},${p.cluster}`;
  });
  const vertexCsv = join(dir, "vertices.csv");
  writeFileSync(
    vertexCsv,
    `dense_id,subject,birth_year,postcode,x,y,cluster_id\n${rows.join("\n")}\n`,
  );

  // The smallest equivalence class, counted rather than derived — the fixture
  // publishes it and the guard re-derives it off the Parquet, so a formula here
  // and a formula there agreeing would prove only that one was copied.
  const classes = new Map();
  for (const p of ordered) {
    const key = quasi(p.index).join("\u0000");
    classes.set(key, (classes.get(key) ?? 0) + 1);
  }
  const reached = Math.min(...classes.values());

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

  /**
   * **The written pyramid**, or nothing where the type is under the floor.
   *
   * Level `k` is the rows whose `dense_id` is a multiple of `2^k`, which over a Morton-ordered
   * `dense_id` is one vertex per quadtree cell of depth `k`. Every row is a real vertex at its real
   * position — there is no synthetic centroid here, because a centroid cannot nest: replace it with
   * its children and every point on screen moves. A decimation nests by construction, so zooming in
   * only ever ADDS.
   *
   * **The file is a cache of the predicate and nothing else.** A corpus without one draws the same
   * picture and only reads more, which is what keeps the pyramid from being a second contract — so
   * the selection below is the predicate itself rather than a stride over the write order, which
   * would coincide with it only while the numbering is gapless.
   *
   * Each level is a payload set addressed by the same rule: `l{k}/` under the type's own prefix,
   * tiled at the same `chunk_size` into the same container. A reader that can address a type can
   * address a level of it with no new arithmetic — tile `j` of level `k` is the `dense_id` range
   * `[j·chunk·2^k, (j+1)·chunk·2^k)`.
   *
   * Which levels get written is {@link levelPlan}, whose borders are published in
   * `vectors.json` and executed by BOTH writers — this one and
   * `fossil_sinks::manifest::VertexLevels::planned`.
   */
  const plan = levelPlan(BigInt(count), BigInt(tileRows));
  const levelCopy =
    plan === null
      ? ""
      : plan.levels
          .map((level) => {
            // The prefixes are made here rather than beside the others at the top: which of them
            // exist is the plan's answer, and a directory made for a level nobody writes is a
            // prefix a reader can list and find empty.
            mkdirSync(join(vertexPrefix, `l${level}`), { recursive: true });
            const step = 2 ** level;
            const held = Number(levelRows(BigInt(count), BigInt(level)));
            const prefix = join(vertexPrefix, `l${level}`);
            const rows = `SELECT * FROM v WHERE dense_id % ${step} = 0`;
            if (layout !== "files") {
              return `COPY (${rows} ORDER BY dense_id) TO '${lit(join(prefix, "tiles.parquet"))}'
                        (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`;
            }
            return Array.from(
              { length: Math.ceil(held / tileRows) },
              (_, k) =>
                `COPY (${rows} ORDER BY dense_id LIMIT ${tileRows} OFFSET ${k * tileRows})
                   TO '${lit(join(prefix, `chunk${k}.parquet`))}' (FORMAT PARQUET);`,
            ).join("\n");
          })
          .join("\n");

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

  /**
   * **The pyramid of EDGES**, or nothing where the source type is under the floor.
   *
   * Level `k` of a relation is the edges incident to a level-`k` vertex — `src % 2^k = 0 OR
   * dst % 2^k = 0` — and every row carries BOTH endpoints' coordinates. The coordinates are the
   * whole point: a camera keeps an edge with ONE end drawn, so the far end has to be positioned to
   * draw the line, and a vertex level holds one row in `2^k`. Measured on the bench corpus at a
   * three-pixel floor, a vertex level can position 0.79% of the edges the same view draws. With
   * the ends in the row the set is SELF-DRAWING: the lines and their ends come out of one file and
   * no vertex tile is opened for them.
   *
   * Nothing here is contracted. Every row is a real edge between two real vertices at their real
   * positions, which is what keeps the standing refusal intact — an edge standing in for a path
   * through vertices that are not drawn moves every line on screen when the camera zooms.
   *
   * The levels are the SOURCE type's, from the same {@link levelPlan} the vertex sets use, and
   * tiled by the source level's own `dense_id` range so a reader addresses them with the shift it
   * already has.
   */
  const edgeLevelCopy =
    plan === null
      ? ""
      : plan.levels
          .map((level) => {
            const step = 2 ** level;
            const dir = join(edgeDir, `l${level}`);
            mkdirSync(dir, { recursive: true });
            const rows =
              `SELECT e.src_dense, e.dst_dense, s.x AS src_x, s.y AS src_y, d.x AS dst_x, d.y AS dst_y
                 FROM e JOIN v s ON s.dense_id = e.src_dense JOIN v d ON d.dense_id = e.dst_dense
                WHERE (e.src_dense % ${step} = 0 OR e.dst_dense % ${step} = 0)`;
            if (layout !== "files") {
              return `COPY (${rows} ORDER BY e.src_dense, e.dst_dense)
                        TO '${lit(join(dir, "tiles.parquet"))}' (FORMAT PARQUET, ROW_GROUP_SIZE ${tileRows});`;
            }
            // One file per level tile, whose `dense_id` range is the source level's own. The
            // range is appended with `AND`, which is why the disjunction above is PARENTHESISED:
            // `AND` binds tighter than `OR`, so without them every tile file also held every edge
            // whose source is in the level, unrestricted — 152 rows where the predicate has 76.
            const span = tileRows * step;
            return Array.from({ length: Math.ceil(count / span) }, (_, k) =>
              `COPY (${rows} AND e.src_dense >= ${k * span} AND e.src_dense < ${(k + 1) * span}
                       ORDER BY e.src_dense, e.dst_dense)
                 TO '${lit(join(dir, `chunk${k}.parquet`))}' (FORMAT PARQUET);`,
            ).join("\n");
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
    ${levelCopy}
    ${edgeTileCopy}
    ${edgeLevelCopy}
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
      // What the bytes guarantee, beside the container, because both are
      // properties of the whole release rather than of a column. A flat mapping
      // of scalars: the manifest's grammar is what `manifest.mjs` reads, and a
      // nested sequence here would be SKIPPED rather than refused.
      "privacy:",
      "  bound: k-anonymity",
      `  k: ${K}`,
      `  reached: ${reached}`,
      "  absent_quasi_identifier: value",
      `  population: ${count}`,
      "  suppressed: 0",
      "  suppression_budget_ppm: 0",
      "  quasi_identifiers: Person.birth_year Person.postcode",
      "  policy: https://example.org/policies/fixture-v1",
      "  profile: https://fossil-lang.org/ns/privacy/v1",
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
      // `is_primary` is not optional, and leaving it off produced a manifest fossil's own
      // reader refuses: `fossil_sinks::manifest::Property` has no default for it, so
      // `openCorpus` failed on this file with «missing field `is_primary`» while every
      // JavaScript reader sailed past. That is the asymmetry the third reader was added
      // to catch, catching something.
      "  - name: birth_year",
      "    data_type: int32",
      "    is_primary: false",
      "  - name: postcode",
      "    data_type: string",
      "    is_primary: false",
      "index:",
      "  prefix: index/",
      "  ordered_by: subject",
      `  chunk_size: ${tileRows}`,
      // Where the tile-code anchor is. A path and not the numbers themselves:
      // there are two per tile, so inlining them would make this document grow
      // with the corpus, and everything that touches a corpus reads it in full.
      "codes:",
      "  path: codes.json",
      // Which decimated levels are written, and where. The NUMBERS, unlike the anchor above, and
      // the asymmetry is the point: an anchor is two integers per tile and grows with the corpus,
      // a level list is at most three integers whatever the corpus is — and which levels a writer
      // spent bytes on is a POLICY, so a reader re-deriving it would 404 the day the policy moved.
      //
      // A sequence inside a mapping, which is one level deeper than this manifest had ever gone.
      // `manifest.mjs` was grown to read it; before that it scanned to `''` and a corpus with a
      // pyramid read exactly like one without.
      ...(plan === null
        ? []
        : [
            "levels:",
            "  prefix: l",
            "  levels:",
            ...plan.levels.map((level) => `  - ${level}`),
            `  chunk_size: ${tileRows}`,
          ]),
      "version: gar/v1",
      "",
    ].join("\n"),
  );
  // The tile-code anchor: `lo[k]` is the Morton code of tile `k`'s first row and
  // `hi[k]` the code of its last. Both come off `ordered`, which IS the ranking —
  // so this is a projection of what the renumbering already produced and not a
  // second pass over the corpus, which is the same thing that makes it cheap on
  // fossil's side.
  //
  // **This is the second implementation of the anchor**, in the sense the rest of
  // this file is: written from the published convention, checked against a corpus
  // fossil wrote by the `code-anchor` guard rather than against fossil's source.
  {
    const anchorTiles = Math.ceil(count / tileRows);
    const lo = [];
    const hi = [];
    for (let k = 0; k < anchorTiles; k += 1) {
      const first = k * tileRows;
      const last = Math.min(first + tileRows, count) - 1;
      lo.push(ordered[first].morton);
      hi.push(ordered[last].morton);
    }
    writeFileSync(
      join(dir, "vertex", "Person", "codes.json"),
      [
        "{",
        `  "morton_bits": 16,`,
        `  "chunk_size": ${tileRows},`,
        `  "tiles": ${anchorTiles},`,
        `  "extent": { "xlo": ${extent.minX}, "ylo": ${extent.minY}, "xhi": ${extent.maxX}, "yhi": ${extent.maxY} },`,
        `  "lo": [${lo.join(",")}],`,
        `  "hi": [${hi.join(",")}]`,
        "}",
        "",
      ].join("\n"),
    );
  }

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
      // Which decimated levels of this RELATION are written. The source type's levels, because a
      // level of a relation is which vertices are in it — and the numbers rather than a rule, on
      // the vertex block's argument: which levels a writer spent bytes on is a policy.
      ...(plan === null
        ? []
        : [
            "levels:",
            "  prefix: l",
            "  levels:",
            ...plan.levels.map((level) => `  - ${level}`),
            `  chunk_size: ${tileRows}`,
          ]),
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

  return { dir, count, edges: pairs.length, tiles, layout, chunkSize: tileRows, reached };
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
