/**
 * The conventions of a fossil corpus, as executable checks.
 *
 * The corpus is documented rather than typed. That was a decision and it has a price, stated here
 * where it is paid: **a guard checks what it was asked to check and nothing more.** There is no
 * compiler between the writer and the reader, so each guard below carries, in its own definition,
 * what it proves and what it cannot — and the second field is the one worth reading.
 *
 * Every check is phrased as a **count of violations**, so the expected answer is always zero and a
 * failure can say what was violated instead of what was expected. That form has one failure mode
 * and it is the classic one: an empty corpus satisfies all of them at once. The first guard is
 * therefore about non-emptiness, and it is not a formality.
 *
 * Nothing here imports a fossil crate, an `@fossil-lang/*` package, or a Parquet library. The
 * corpus is read with plain SQL through the `duckdb` binary, which is the position a stranger is
 * in. A guard that read the corpus back through the writer's own types would prove the types are
 * self-consistent and nothing about the artefact.
 */

import { existsSync, readFileSync } from "node:fs";
import { query, scalar } from "./duck.mjs";
import { fileList, rowGroups } from "./inspect.mjs";
import { TILE_ROWS, morton2, quantize, shiftFor, tileOf } from "./arithmetic.mjs";

/**
 * The published borders, read on first use.
 *
 * Read at module scope this was a file access every importer paid, including one that wants only
 * the `GUARDS` metadata: the corpus site renders the guard index from this module at build time,
 * and inside a bundler `import.meta.url` is not a `file:` URL, so the read threw and the page was
 * blank. One guard reads the vectors; only that guard should pay for them.
 */
let vectorsCache;
function vectors() {
  vectorsCache ??= JSON.parse(
    readFileSync(new URL("./vectors.json", import.meta.url), "utf8"),
  );
  return vectorsCache;
}

/**
 * The bound on how much of a corpus one tile's bounding box may cover, averaged over the tiles.
 *
 * Not a quality target. A Morton-ordered corpus of 70,000 vertices in 18 tiles measures **0.124**;
 * the same vertices with `dense_id` assigned at random measure **0.99**, which is what theory says
 * — with no spatial order every tile's box is the whole corpus, so the average share is 1 whatever
 * the tile count. Half sits four times above the ordered case and just below the disordered one at
 * the smallest corpus worth checking, and the gap only widens with N.
 *
 * It separates *a spatial order* from *no spatial order*. It does not separate a good one from a
 * slightly worse one, and no single number could.
 */
const MAX_MEAN_BOX_SHARE = 0.5;

/** A guard's answer. `failures` are violations of a convention; `notes` are what it measured. */
function result(failures = [], notes = []) {
  return { failures, notes };
}

/** `count` of `what`, phrased so a zero is silent and a non-zero names the convention broken. */
function violations(count, what) {
  return Number(count) === 0 ? [] : [`${Number(count).toLocaleString("en-US")} × ${what}`];
}

/** The SQL that recomputes the Morton code of every vertex from the corpus's own positions. */
function mortonSql(files) {
  return `
    WITH src AS (SELECT dense_id, x, y FROM read_parquet(${fileList(files)})),
    ext AS (SELECT min(x) AS mnx, max(x) AS mxx, min(y) AS mny, max(y) AS mxy FROM src),
    q AS (SELECT dense_id,
            CASE WHEN mxx <= mnx THEN 0::BIGINT ELSE
              CAST(round(least(greatest((x - mnx) / (mxx - mnx), 0::FLOAT), 1::FLOAT) * 65535::FLOAT) AS BIGINT) END AS a0,
            CASE WHEN mxy <= mny THEN 0::BIGINT ELSE
              CAST(round(least(greatest((y - mny) / (mxy - mny), 0::FLOAT), 1::FLOAT) * 65535::FLOAT) AS BIGINT) END AS b0
          FROM src, ext),
    z AS (SELECT dense_id,
            (a0 | (a0 << 8)) & 16711935  AS a1, (b0 | (b0 << 8)) & 16711935  AS b1,
            (a1 | (a1 << 4)) & 252645135 AS a2, (b1 | (b1 << 4)) & 252645135 AS b2,
            (a2 | (a2 << 2)) & 858993459 AS a3, (b2 | (b2 << 2)) & 858993459 AS b3,
            (a3 | (a3 << 1)) & 1431655765 AS a4, (b3 | (b3 << 1)) & 1431655765 AS b4,
            (a4 | (b4 << 1)) AS morton
          FROM q)`;
}

/** The bounding box of every tile, whichever container carries them. */
function tileBoxes(type) {
  const groups = rowGroups(type.files);
  const boxes = [];
  for (const [, byGroup] of groups) {
    for (const [, group] of byGroup) {
      const x = group.stats.get("x");
      const y = group.stats.get("y");
      if (!x || !y || x.min === null || y.min === null) return null;
      boxes.push({ x0: Number(x.min), x1: Number(x.max), y0: Number(y.min), y1: Number(y.max) });
    }
  }
  return boxes;
}

/**
 * Check the arithmetic against a vector table.
 *
 * Taken as an argument rather than read from the module so `self-test.mjs` can hand it a corrupted
 * table and watch this fire. A guard nobody has ever seen fail is a guard nobody has tested.
 */
export function checkVectors(vectors) {
  const failures = [];
  for (const v of vectors.tile_of.vectors) {
    const got = tileOf(BigInt(v.dense_id));
    if (got !== BigInt(v.tile)) failures.push(`tile_of(${v.dense_id}) = ${got}, not ${v.tile}`);
  }
  for (const v of vectors.morton2.vectors) {
    const got = morton2(v.x, v.y);
    if (got !== v.morton) failures.push(`morton2(${v.x}, ${v.y}) = ${got}, not ${v.morton}`);
  }
  for (const v of vectors.quantize.vectors) {
    const got = quantize(v.v, v.lo, v.hi);
    if (got !== v.q) failures.push(`quantize(${v.v}, ${v.lo}, ${v.hi}) = ${got}, not ${v.q}`);
  }
  if (shiftFor(TILE_ROWS) !== 12n) failures.push("the shift and the row count disagree");
  const counted =
    vectors.tile_of.vectors.length + vectors.morton2.vectors.length + vectors.quantize.vectors.length;
  return result(failures, [`${counted} vectors`]);
}

export const GUARDS = [
  {
    id: "not-empty",
    title: "The corpus is not empty",
    proves:
      "Every other guard is a count of violations, so an empty corpus satisfies all of them at " +
      "once. This one asserts there are vertices, edges and more than two tiles — a boundary is " +
      "the only thing a tiling can get wrong, and one tile has none — and that both orientations " +
      "of every edge type are on disk *and tiled*, since a hop that has one of them is wrong in " +
      "one direction rather than slow.",
    cannotProve:
      "That the corpus is complete. A corpus missing half its vertices is non-empty, and the " +
      "manifest carries no vertex count to check it against.",
    run(corpus) {
      const failures = [];
      if (corpus.types.length === 0) failures.push("the manifest names no vertex type");
      for (const type of corpus.types) {
        if (type.count === 0) failures.push(`vertex type ${type.name} has no rows`);
        const tiles = type.chunkSize > 0n ? Math.ceil(type.count / Number(type.chunkSize)) : 0;
        if (tiles < 3) {
          failures.push(
            `vertex type ${type.name} is ${tiles} tile(s), which has no boundary to get wrong`,
          );
        }
      }
      // Both orientations, because half a corpus is not a partial answer. The
      // out-edges of a vertex are in its `by_source` tile and the in-edges in its
      // `by_target` one, so an emitter that writes only the source half leaves a
      // reader following half the graph and reporting it as the whole. Every
      // other guard here skips an orientation with nothing on disk; this is the
      // one that notices it is not there.
      for (const edge of corpus.edges) {
        for (const side of [edge.bySource, edge.byTarget]) {
          const files = [...side.relation, ...side.tiles];
          if (files.length === 0) failures.push(`edge type ${edge.rel} has no ${side.name} payload`);
          if (side.relation.length > 0 && side.tiles.length === 0) {
            failures.push(
              `edge type ${edge.rel} has a ${side.name} relation and no tiles, so a hop through it is a scan`,
            );
          }
        }
      }
      return result(
        failures,
        corpus.types.map((t) => `${t.name}: ${t.count.toLocaleString("en-US")} vertices, ${t.layout}`),
      );
    },
  },

  {
    id: "entry-point",
    title: "One entry point, and every path it names resolves",
    proves:
      "A reader that cannot list a directory — which is every reader over HTTP — starts at " +
      "`graph.graph.yml` and reaches everything else from there. Each path the index names is on " +
      "disk, and each type's declared prefix holds a payload.",
    cannotProve:
      "That the index names everything that is there. A vertex type written but not indexed is " +
      "invisible to this guard for exactly the reason it is invisible to a reader.",
    run(corpus) {
      const failures = corpus.manifest.unreadable.map(
        (entry) => `the manifest names ${entry.rel}, which ${entry.missing}`,
      );
      for (const type of corpus.types) {
        if (type.files.length === 0) failures.push(`${type.name} declares prefix ${type.prefix}/, which holds no Parquet`);
      }
      return result(failures, [`${corpus.manifest.declared.length} declared paths`]);
    },
  },

  {
    id: "plain-parquet",
    title: "Plain Parquet, and the addressing columns are named",
    proves:
      "Every payload opens with `read_parquet` and no extension, and carries the columns the " +
      "conventions address by: `dense_id`, `x` and `y` on a vertex payload; `src_dense` and " +
      "`dst_dense` on an adjacency. A reader needs a Parquet reader and the column names, and " +
      "nothing else.",
    cannotProve:
      "That the *types* of those columns are what a reader assumes. A `dense_id` stored as a " +
      "string opens, describes and addresses — until the reader shifts it.",
    run(corpus) {
      const failures = [];
      for (const type of corpus.types) {
        for (const column of ["dense_id", "x", "y"]) {
          if (!type.columns.has(column)) failures.push(`${type.name} has no ${column} column`);
        }
      }
      for (const edge of corpus.edges) {
        for (const side of [edge.bySource, edge.byTarget]) {
          const files = [...side.relation, ...side.tiles];
          if (files.length === 0) continue;
          for (const column of ["src_dense", "dst_dense"]) {
            if (!side.columns.has(column)) failures.push(`${edge.rel} ${side.name} has no ${column} column`);
          }
        }
      }
      return result(failures);
    },
  },

  {
    id: "declared-tiling",
    title: "The manifest declares the tiling that was emitted",
    proves:
      "`chunk_size` is a power of two, so `dense_id >> shift` addresses a tile and no division " +
      "does; the tile count on disk is the one the row count implies; and an edge type's " +
      "`src_chunk_size` equals its source type's `chunk_size`, because an edge tile is addressed " +
      "by the source's tile and a different number there would address nothing.",
    cannotProve:
      "That the declared size is the *right* size. 4,096 is a measured trade-off between requests " +
      "and bytes, not an invariant, and a corpus may declare another power of two and be read.",
    run(corpus) {
      const failures = [];
      for (const type of corpus.types) {
        if (type.shift === null) {
          failures.push(`${type.name} declares a tile of ${type.chunkSize} rows, which no shift addresses`);
          continue;
        }
        const expected = Math.ceil(type.count / Number(type.chunkSize));
        if (type.layout === "files" && type.files.length !== expected) {
          failures.push(`${type.name} has ${type.files.length} tile files where ${expected} are implied`);
        }
        if (type.layout === "mixed") {
          failures.push(`${type.name} carries tiles two ways at once, so a reader that globs finds both`);
        }
      }
      for (const edge of corpus.edges) {
        const source = corpus.types.find((t) => t.name === edge.srcType);
        if (!source) {
          failures.push(`${edge.rel} names source type ${edge.srcType}, which the index does not`);
          continue;
        }
        if (edge.srcChunkSize !== source.chunkSize) {
          failures.push(
            `${edge.rel} declares src_chunk_size ${edge.srcChunkSize} against ${edge.srcType}'s ${source.chunkSize}`,
          );
        }
        if (edge.chunkSize !== edge.srcChunkSize) {
          failures.push(`${edge.rel} declares chunk_size ${edge.chunkSize} and src_chunk_size ${edge.srcChunkSize}`);
        }
        // The destination half is checked because it is a *different* number on a
        // cross-type edge, and because the target-ordered tiles are addressed
        // with it. An unchecked `dst_chunk_size` is how the in-edge half comes to
        // be cut on the wrong ranges while every same-type corpus passes.
        const destination = corpus.types.find((t) => t.name === edge.dstType);
        if (!destination) {
          failures.push(`${edge.rel} names destination type ${edge.dstType}, which the index does not`);
        } else if (edge.dstChunkSize !== destination.chunkSize) {
          failures.push(
            `${edge.rel} declares dst_chunk_size ${edge.dstChunkSize} against ${edge.dstType}'s ${destination.chunkSize}`,
          );
        }
        // And every orientation on disk was declared with somewhere to be. A
        // prefix is the one part of a tile's URL a reader cannot compute, so an
        // adjacency list without one is tiles nobody can address.
        for (const side of [edge.bySource, edge.byTarget]) {
          if (side.declared === null) {
            failures.push(`${edge.rel} declares no adj_list aligned_by ${side.alignedBy}`);
          } else if (side.tilePrefix === "") {
            failures.push(
              `${edge.rel} declares an adj_list aligned_by ${side.alignedBy} with no prefix, ` +
                `so its tiles have no address`,
            );
          }
        }
      }
      return result(
        failures,
        corpus.types.map((t) => `${t.name}: ${t.chunkSize} rows per tile, shift ${t.shift}`),
      );
    },
  },

  {
    id: "dense-ids",
    title: "`dense_id` is a gapless 0..V−1",
    proves:
      "Per vertex type, the ids are exactly `0..V−1` with no gaps and no repeats. The whole " +
      "addressing story rests on it: a tile is a range of ids, so a gap is either an addressable " +
      "empty tile or a vertex nobody can address, and the arithmetic that drops the `dense_id` " +
      "column entirely — row `j` of tile `i` is `i·4096 + j` — is only sound because of this.",
    cannotProve:
      "That the numbering means anything. Gapless ids in an arbitrary order pass here and fail " +
      "`morton-order`, which is the guard that makes the numbering an address.",
    run(corpus) {
      const failures = [];
      for (const type of corpus.types) {
        if (type.files.length === 0) continue;
        const list = fileList(type.files);
        const row = query(
          `SELECT min(dense_id) AS lo, max(dense_id) AS hi, count(*) AS n,
                  count(DISTINCT dense_id) AS distinct_n, count(*) FILTER (dense_id IS NULL) AS nulls
             FROM read_parquet(${list})`,
        )[0];
        if (Number(row.nulls) > 0) failures.push(`${type.name} has ${row.nulls} null dense_id`);
        if (Number(row.lo) !== 0) failures.push(`${type.name} starts at dense_id ${row.lo}, not 0`);
        if (Number(row.hi) !== Number(row.n) - 1) {
          failures.push(`${type.name} has ${row.n} rows and a maximum dense_id of ${row.hi}`);
        }
        if (Number(row.distinct_n) !== Number(row.n)) {
          failures.push(`${type.name} repeats ${Number(row.n) - Number(row.distinct_n)} dense_id(s)`);
        }
      }
      return result(failures);
    },
  },

  {
    id: "tile-of",
    title: "Every row is in the tile its id names",
    proves:
      "`dense_id >> shift` is the entire index — no table, no listing, no discovery — so a row in " +
      "the wrong container is a row a reader will never fetch and never miss. Checked in whichever " +
      "form the corpus uses: against the number in the filename when a tile is a file, against the " +
      "row-group ordinal when a tile is a row group. An edge is checked against the tile of the " +
      "endpoint its file is ordered by — the source for `by_source`, the destination for " +
      "`by_target` — which is what CSR and CSC placement mean.",
    cannotProve:
      "That the shift is applied the same way elsewhere. This asks the corpus a question; the " +
      "published border vectors are what ask the *reader* one, and they are the next guard.",
    run(corpus) {
      const failures = [];

      const checkFiles = (files, column, label, shift) => {
        const bad = scalar(
          `SELECT count(*) FROM read_parquet(${fileList(files)}, filename = true)
             WHERE (${column} >> ${shift}) <> regexp_extract(filename, '([0-9]+)\\.parquet$', 1)::BIGINT`,
        );
        failures.push(...violations(bad, `${label}: a row is in a tile its ${column} does not name`));
      };

      const checkRowGroups = (files, column, label, shift, chunkSize) => {
        for (const [file, groups] of rowGroups(files)) {
          for (const [id, group] of groups) {
            const stats = group.stats.get(column);
            if (!stats || stats.min === null) {
              failures.push(`${label}: row group ${id} of ${file} carries no ${column} statistics`);
              continue;
            }
            const lo = tileOf(BigInt(stats.min), shift);
            const hi = tileOf(BigInt(stats.max), shift);
            if (lo !== hi || lo !== BigInt(id)) {
              failures.push(
                `${label}: row group ${id} holds ${column} tiles ${lo}..${hi}, so the ordinal is not the address`,
              );
            }
            if (group.rows > Number(chunkSize)) {
              failures.push(`${label}: row group ${id} holds ${group.rows} rows against a tile of ${chunkSize}`);
            }
          }
        }
      };

      for (const type of corpus.types) {
        if (type.shift === null || type.files.length === 0) continue;
        if (type.layout === "files") checkFiles(type.files, "dense_id", type.name, type.shift);
        else if (type.layout === "rowgroups") {
          checkRowGroups(type.files, "dense_id", type.name, type.shift, type.chunkSize);
        }
      }

      // Both orientations, each against the tile size of the endpoint that
      // addresses it. On a cross-type edge those are two different `dense_id`
      // spaces, so checking the target half against `src_chunk_size` would be
      // checking the wrong arithmetic and passing on a same-type corpus.
      for (const edge of corpus.edges) {
        for (const [side, chunkSize] of [
          [edge.bySource, edge.srcChunkSize],
          [edge.byTarget, edge.dstChunkSize],
        ]) {
          const shift = shiftFor(chunkSize);
          if (shift === null || side.tiles.length === 0) continue;
          const label = `${edge.rel} ${side.name}`;
          if (side.layout === "files") checkFiles(side.tiles, side.column, label, shift);
          else if (side.layout === "rowgroups") {
            checkRowGroups(side.tiles, side.column, label, shift, chunkSize);
          }
        }
      }

      return result(failures);
    },
  },

  {
    id: "published-vectors",
    title: "The published vectors reproduce",
    proves:
      "`tile_of`, `morton2` and the quantisation answer what `guards/vectors.json` says they " +
      "answer, at every border where a re-implementation diverges: 2³¹ for a shift taken as " +
      "signed, 2⁵³ for an id that went through a JavaScript `Number`, and bit 31 of a Morton code " +
      "for an interleave that was not coerced back to unsigned. The vectors are the deliverable — " +
      "they are what gets copied.",
    cannotProve:
      "Anything about the corpus. This is a statement about a function, and it is here so that a " +
      "checker run against somebody else's corpus also checks the checker.",
    run() {
      return checkVectors(vectors());
    },
  },

  {
    id: "morton-order",
    title: "`dense_id` ascends with the Morton code of the position",
    proves:
      "The order is the index. Recomputing the code from the corpus's own `x` and `y` — quantised " +
      "over that vertex type's own extent, in binary32, the width the conventions specify — the " +
      "code never decreases as `dense_id` increases. This is what makes a rectangle in space a few " +
      "hundred contiguous runs of `dense_id` instead of a scan: a rectangle over a Morton order " +
      "breaks into O(√n) segments, and that is the whole mechanism.",
    cannotProve:
      "That the *layout* is any good — a Morton order over positions that mean nothing is still a " +
      "Morton order. And it is exact only because the width is specified: a writer that quantised " +
      "in binary64 can report inversions here that are ties in its own arithmetic, which is why " +
      "the conventions name the width rather than leaving it to be inferred.",
    run(corpus) {
      const failures = [];
      for (const type of corpus.types) {
        if (type.files.length === 0 || !type.columns.has("x") || !type.columns.has("y")) continue;
        const bad = scalar(
          `${mortonSql(type.files)}
           SELECT count(*) FROM (SELECT morton, lag(morton) OVER (ORDER BY dense_id) AS prev FROM z)
            WHERE prev IS NOT NULL AND morton < prev`,
        );
        failures.push(...violations(bad, `${type.name}: dense_id ascends where the Morton code falls`));
      }
      return result(failures);
    },
  },

  {
    id: "spatial-tiles",
    title: "A tile is compact in space",
    proves:
      "What the order exists to produce, measured on the footer a reader actually has: the average " +
      "tile's bounding box covers at most half the corpus extent. A Morton-ordered corpus measures " +
      "far below that (0.124 on the fixture); the same vertices renumbered at random measure 0.99, " +
      "because with no spatial order every tile's box is the whole corpus.",
    cannotProve:
      "The difference between a good spatial order and a slightly worse one. It separates having " +
      "one from not having one, and no single number could do more. The measured over-read on a " +
      "real corpus is 1.10×–1.20× at tile granularity; this bound is nowhere near that tight and " +
      "is not a quality target.",
    run(corpus) {
      const failures = [];
      const notes = [];
      for (const type of corpus.types) {
        if (type.files.length === 0 || !type.columns.has("x")) continue;
        const boxes = tileBoxes(type);
        if (boxes === null) {
          failures.push(`${type.name}: a tile carries no x/y statistics, so its box is unknowable`);
          continue;
        }
        const extent = boxes.reduce(
          (a, b) => ({
            x0: Math.min(a.x0, b.x0), x1: Math.max(a.x1, b.x1),
            y0: Math.min(a.y0, b.y0), y1: Math.max(a.y1, b.y1),
          }),
          { x0: Infinity, x1: -Infinity, y0: Infinity, y1: -Infinity },
        );
        const area = (extent.x1 - extent.x0) * (extent.y1 - extent.y0);
        if (!(area > 0)) continue;
        const share =
          boxes.reduce((sum, b) => sum + (b.x1 - b.x0) * (b.y1 - b.y0), 0) / area / boxes.length;
        notes.push(`${type.name}: mean tile box covers ${(share * 100).toFixed(1)}% of the extent`);
        if (share > MAX_MEAN_BOX_SHARE) {
          failures.push(
            `${type.name}: the average tile box covers ${(share * 100).toFixed(1)}% of the corpus, ` +
              `so selecting tiles by box selects nearly all of them`,
          );
        }
      }
      return result(failures, notes);
    },
  },

  {
    id: "csr-and-csc",
    title: "`by_source` is CSR and `by_target` is CSC",
    proves:
      "Zero out-of-order rows by `src_dense` in the source orientation and by `dst_dense` in the " +
      "target one. A reader that trusts the ordering to skip work gets wrong answers rather than " +
      "slow ones if this breaks, and so does the layout pass, which reads the file as the CSR it " +
      "claims to be and builds a different graph if it is not.",
    cannotProve:
      "That the secondary order is anything. Rows sharing a `src_dense` may appear in any order, " +
      "and no reader may depend on which.",
    run(corpus) {
      const failures = [];
      for (const edge of corpus.edges) {
        for (const [side, column] of [
          [edge.bySource, "src_dense"],
          [edge.byTarget, "dst_dense"],
        ]) {
          const files = side.relation.length > 0 ? side.relation : side.tiles;
          if (files.length === 0) continue;
          const bad = scalar(
            `SELECT count(*) FROM (SELECT ${column} AS k, lag(${column}) OVER () AS prev
                                     FROM read_parquet(${fileList(files)})) WHERE prev IS NOT NULL AND k < prev`,
          );
          failures.push(...violations(bad, `${edge.rel} ${side.name} is out of order by ${column}`));
        }
      }
      return result(failures);
    },
  },

  {
    id: "one-relation-twice",
    title: "The two orientations are one relation stored twice",
    proves:
      "`by_source` and `by_target` carry the same edge set, checked in both directions so neither " +
      "a missing edge nor an extra one survives. Nothing else in a corpus says they are the same " +
      "relation; the names do not, and a reader answering a neighbourhood query from one and a " +
      "count from the other would silently disagree with itself.",
    cannotProve:
      "That either orientation is the *whole* relation. Both being wrong identically passes.",
    run(corpus) {
      const failures = [];
      for (const edge of corpus.edges) {
        const source = edge.bySource.relation.length > 0 ? edge.bySource.relation : edge.bySource.tiles;
        const target = edge.byTarget.relation.length > 0 ? edge.byTarget.relation : edge.byTarget.tiles;
        if (source.length === 0 || target.length === 0) continue;
        const bad = scalar(
          `SELECT count(*) FROM (
             (SELECT src_dense, dst_dense FROM read_parquet(${fileList(source)})
              EXCEPT SELECT src_dense, dst_dense FROM read_parquet(${fileList(target)}))
             UNION ALL
             (SELECT src_dense, dst_dense FROM read_parquet(${fileList(target)})
              EXCEPT SELECT src_dense, dst_dense FROM read_parquet(${fileList(source)})))`,
        );
        failures.push(...violations(bad, `${edge.rel}: an edge exists in one orientation and not the other`));
      }
      return result(failures);
    },
  },

  {
    id: "no-dangling-endpoint",
    title: "Every endpoint addresses a vertex that exists",
    proves:
      "Both endpoints of every edge fall inside their own type's `0..V−1`. This is the corruption " +
      "a renumbering can introduce and that no row count anywhere would reveal — the edge counts " +
      "still match, both orientations still agree, and the graph is wrong.",
    cannotProve:
      "That an endpoint addresses the *right* vertex. A renumbering that permuted two ids leaves " +
      "every id in range; only `morton-order` and an identity join catch that.",
    run(corpus) {
      const failures = [];
      for (const edge of corpus.edges) {
        const files = edge.bySource.relation.length > 0 ? edge.bySource.relation : edge.bySource.tiles;
        if (files.length === 0) continue;
        const src = corpus.types.find((t) => t.name === edge.srcType);
        const dst = corpus.types.find((t) => t.name === edge.dstType);
        if (!src || !dst) continue;
        const bad = scalar(
          `SELECT count(*) FROM read_parquet(${fileList(files)})
             WHERE src_dense < 0 OR src_dense >= ${src.count} OR dst_dense < 0 OR dst_dense >= ${dst.count}`,
        );
        failures.push(...violations(bad, `${edge.rel}: an edge points at a dense_id no vertex has`));
      }
      return result(failures);
    },
  },

  {
    id: "exactly-once",
    title: "Every row is in the corpus exactly once",
    proves:
      "A vertex payload set holds each `dense_id` once, and the edge tiles hold exactly the " +
      "relation they cut — no row dropped, none written twice, checked as a symmetric difference " +
      "rather than a count. It also asserts that the staged single-file vertex Parquet a layout " +
      "pass consumes is *gone*: left behind it is a second, stale copy of every vertex, the kind a " +
      "reader picks up by globbing and never questions.",
    cannotProve:
      "That the rows are the right rows. Splitting a file is where rows are silently dropped, and " +
      "this catches that; a file that was split correctly from wrong content passes.",
    run(corpus) {
      const failures = [];
      for (const type of corpus.types) {
        if (existsSync(type.staged)) {
          failures.push(`${type.name}: the staged ${type.prefix}.parquet is a second copy of every vertex`);
        }
        if (type.files.length === 0) continue;
        const bad = scalar(
          `SELECT count(*) - count(DISTINCT dense_id) FROM read_parquet(${fileList(type.files)})`,
        );
        failures.push(...violations(bad, `${type.name}: a dense_id appears in more than one payload file`));
      }
      for (const edge of corpus.edges) {
        for (const side of [edge.bySource, edge.byTarget]) {
          if (side.relation.length === 0 || side.tiles.length === 0) continue;
          const relation = fileList(side.relation);
          const tiles = fileList(side.tiles);
          const counts = query(
            `SELECT (SELECT count(*) FROM read_parquet(${relation})) AS relation,
                    (SELECT count(*) FROM read_parquet(${tiles})) AS tiles`,
          )[0];
          if (Number(counts.relation) !== Number(counts.tiles)) {
            failures.push(
              `${edge.rel} ${side.name}: the tiles hold ${counts.tiles} edges against ${counts.relation} in the file they cut`,
            );
          }
          const bad = scalar(
            `SELECT count(*) FROM (
               (SELECT src_dense, dst_dense FROM read_parquet(${relation})
                EXCEPT SELECT src_dense, dst_dense FROM read_parquet(${tiles}))
               UNION ALL
               (SELECT src_dense, dst_dense FROM read_parquet(${tiles})
                EXCEPT SELECT src_dense, dst_dense FROM read_parquet(${relation})))`,
          );
          failures.push(
            ...violations(bad, `${edge.rel} ${side.name}: the tiles and the file they cut disagree about which edges exist`),
          );
        }
      }
      return result(failures);
    },
  },

  {
    id: "footer-is-the-index",
    title: "The footer is the index",
    proves:
      "Every payload's row groups carry min/max statistics for the columns a reader selects on — " +
      "`x` and `y` on a vertex payload, `dense_id` and `src_dense` where they are the address. " +
      "Arithmetic gives a reader *which tiles exist*; only the boxes give it *which tiles " +
      "intersect the window*, because the `dense_id` range of a rectangle is not computable " +
      "without knowing the Morton range, and the boxes are what say it. One request buys the " +
      "footer and the whole index comes with it.",
    cannotProve:
      "That the statistics are true. They are written by the writer and believed by the reader; a " +
      "box wider than its rows costs a request, a box narrower than its rows loses vertices, and " +
      "only re-reading every page would tell them apart.",
    run(corpus) {
      const failures = [];
      const notes = [];
      const wanted = (columns, names) => names.filter((n) => columns.has(n));

      const check = (files, label, columns) => {
        if (files.length === 0) return;
        for (const [file, groups] of rowGroups(files)) {
          for (const [id, group] of groups) {
            for (const column of columns) {
              const stats = group.stats.get(column);
              if (!stats || stats.min === null || stats.max === null) {
                failures.push(`${label}: row group ${id} of ${file} carries no ${column} statistics`);
              }
            }
          }
        }
      };

      for (const type of corpus.types) {
        check(type.files, type.name, wanted(type.columns, ["dense_id", "x", "y"]));
        const groups = [...rowGroups(type.files).values()].reduce((n, g) => n + g.size, 0);
        notes.push(`${type.name}: ${groups} row group(s) over ${type.files.length} file(s)`);
      }
      for (const edge of corpus.edges) {
        for (const side of [edge.bySource, edge.byTarget]) {
          check(side.tiles, `${edge.rel} ${side.name}`, wanted(side.columns, [side.column]));
        }
      }
      return result(failures, notes);
    },
  },

  {
    id: "identity-is-the-subject",
    title: "The identity is the subject IRI, not the address",
    proves:
      "Where a `subject` column is present it is non-null and unique: it is the key, and it is the " +
      "only thing in the corpus that survives a re-layout. Redoing the placement renumbers every " +
      "vertex, so `dense_id` is an address and cannot also be an identity — a selection, a " +
      "bookmark or a link from outside that was stored as a dense id names a different vertex " +
      "after the next write.",
    cannotProve:
      "Where the identity lives when it is not in the drawing payload. Carrying `subject` in a " +
      "tile costs 1.87× the tile — 8.016 bytes per row against 9.23 for all four drawing columns " +
      "— so the measured default is that it does not, and the path a reader fetches it from " +
      "instead is an open convention, not a checked one. This guard reports its absence and does " +
      "not fail on it.",
    run(corpus) {
      const failures = [];
      const notes = [];
      for (const type of corpus.types) {
        if (type.files.length === 0) continue;
        if (!type.columns.has("subject")) {
          notes.push(`${type.name}: no subject column in the payload — the identity is elsewhere`);
          continue;
        }
        const row = query(
          `SELECT count(*) FILTER (subject IS NULL) AS nulls,
                  count(*) - count(DISTINCT subject) AS repeats
             FROM read_parquet(${fileList(type.files)})`,
        )[0];
        failures.push(...violations(row.nulls, `${type.name}: a vertex has no subject`));
        failures.push(...violations(row.repeats, `${type.name}: two vertices share a subject`));
      }
      return result(failures, notes);
    },
  },
];

/** Run every guard, or the subset whose ids are given. */
export function runAll(corpus, only = null) {
  return GUARDS.filter((g) => only === null || only.includes(g.id)).map((guard) => {
    const started = Date.now();
    try {
      const { failures, notes } = guard.run(corpus);
      return { guard, failures, notes, ms: Date.now() - started };
    } catch (error) {
      return { guard, failures: [`the guard could not run: ${error.message}`], notes: [], ms: Date.now() - started };
    }
  });
}

