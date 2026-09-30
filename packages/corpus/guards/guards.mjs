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

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { lit, query, scalar } from "./duck.mjs";
import { fileList, partOf, payload, rowGroups, tileSql } from "./inspect.mjs";
import {
  TILE_ROWS,
  hilbert2,
  quantize,
  shiftFor,
  tailRows,
  tileOf,
  tileUrl,
  tilesOf,
} from "./arithmetic.mjs";

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
 * Not a quality target. A Hilbert-ordered corpus of 70,000 vertices in 18 tiles measures **0.080**;
 * the same vertices with `dense_id` assigned at random measure **0.99**, which is what theory says
 * — with no spatial order every tile's box is the whole corpus, so the average share is 1 whatever
 * the tile count. Half sits six times above the ordered case and just below the disordered one at
 * the smallest corpus worth checking, and the gap only widens with N.
 *
 * It separates *a spatial order* from *no spatial order*. It does not separate a good one from a
 * slightly worse one, and no single number could.
 */
const MAX_MEAN_BOX_SHARE = 0.5;

/**
 * Every spelling DuckDB gives an unsigned integer, which is what an address has to be under a name.
 *
 * Written out rather than matched on a `U` prefix, because the set is closed and small and a prefix
 * test is a rule about spelling rather than about types. It is deliberately *all five* widths and
 * not the one fossil writes: the convention is that the address is unsigned, and the width is a
 * separate commitment stated where the rule is — a corpus another writer produced at `uint64` is
 * addressable by every reader here and is not a violation of anything.
 */
const UNSIGNED = new Set(["UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT", "UHUGEINT"]);

/** A guard's answer. `failures` are violations of a convention; `notes` are what it measured. */
function result(failures = [], notes = []) {
  return { failures, notes };
}

/** `count` of `what`, phrased so a zero is silent and a non-zero names the convention broken. */
function violations(count, what) {
  return Number(count) === 0 ? [] : [`${Number(count).toLocaleString("en-US")} × ${what}`];
}

/**
 * The SQL that recomputes the Hilbert code of every vertex from the corpus's own positions.
 *
 * `hilbert2` in `arithmetic.mjs`, unrolled: one CTE per bit from the top, each adding the quadrant's
 * `s·s·((3·rx) ^ ry)` and turning the frame. Written as `CASE` rather than `xor`, because `^` is
 * exponentiation in DuckDB; and as sixteen CTEs rather than one `SELECT` of lateral aliases, because
 * the binder inlines an alias into every use and each step reads the last one's coordinates twice —
 * two to the sixteenth copies of the first step.
 */
function hilbertSql(files) {
  const steps = [];
  for (let k = 0; k < 16; k += 1) {
    const bit = 15 - k;
    const s2 = 4 ** bit;
    steps.push(`h${k + 1} AS (SELECT dense_id,
            CASE WHEN ry = 1 THEN x WHEN rx = 1 THEN 65535 - y ELSE y END AS x,
            CASE WHEN ry = 1 THEN y WHEN rx = 1 THEN 65535 - x ELSE x END AS y,
            d + ${s2}::BIGINT * CASE WHEN rx = 0 THEN ry ELSE 3 - ry END AS d
          FROM (SELECT *, (x >> ${bit}) & 1 AS rx, (y >> ${bit}) & 1 AS ry FROM h${k}))`);
  }
  return `
    WITH src AS (SELECT dense_id, x, y FROM read_parquet(${fileList(files)})),
    ext AS (SELECT min(x) AS mnx, max(x) AS mxx, min(y) AS mny, max(y) AS mxy FROM src),
    h0 AS (SELECT dense_id,
            CASE WHEN mxx <= mnx THEN 0::BIGINT ELSE
              CAST(round(least(greatest((x - mnx) / (mxx - mnx), 0::FLOAT), 1::FLOAT) * 65535::FLOAT) AS BIGINT) END AS x,
            CASE WHEN mxy <= mny THEN 0::BIGINT ELSE
              CAST(round(least(greatest((y - mny) / (mxy - mny), 0::FLOAT), 1::FLOAT) * 65535::FLOAT) AS BIGINT) END AS y,
            0::BIGINT AS d
          FROM src, ext),
    ${steps.join(",\n    ")},
    z AS (SELECT dense_id, d AS hilbert FROM h16)`;
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
  for (const v of vectors.hilbert2.vectors) {
    const got = hilbert2(v.x, v.y);
    if (got !== v.hilbert) failures.push(`hilbert2(${v.x}, ${v.y}) = ${got}, not ${v.hilbert}`);
  }
  for (const v of vectors.quantize.vectors) {
    const got = quantize(v.v, v.lo, v.hi);
    if (got !== v.q) failures.push(`quantize(${v.v}, ${v.lo}, ${v.hi}) = ${got}, not ${v.q}`);
  }
  for (const v of vectors.tile_url.vectors) {
    const got = tileUrl(v.prefix, v.stem, v.container, v.tile);
    if (got !== v.url) {
      failures.push(`tileUrl(${v.prefix}, ${v.stem}, ${v.container}, ${v.tile}) = ${got}, not ${v.url}`);
    }
  }
  for (const v of vectors.declared_count.vectors) {
    const count = BigInt(v.count);
    const chunk = BigInt(v.chunk_size);
    const tiles = tilesOf(count, chunk);
    const tail = tailRows(count, chunk);
    if (tiles !== BigInt(v.tiles)) failures.push(`tilesOf(${v.count}, ${v.chunk_size}) = ${tiles}, not ${v.tiles}`);
    if (tail !== BigInt(v.tail_rows)) {
      failures.push(`tailRows(${v.count}, ${v.chunk_size}) = ${tail}, not ${v.tail_rows}`);
    }
  }
  if (shiftFor(TILE_ROWS) !== 12n) failures.push("the shift and the row count disagree");
  const counted = Object.values(vectors)
    .filter((section) => Array.isArray(section?.vectors))
    .reduce((n, section) => n + section.vectors.length, 0);
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
      "That the corpus is complete. A corpus missing half its vertices is non-empty, and this " +
      "guard counts what is there rather than what was promised. `declared-count` is the one that " +
      "holds the manifest's `vertex_count` against the disk — a sentence that used to end here " +
      "with «and the manifest carries no vertex count to check it against».",
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
      "That a column carries what its *name* suggests. `x` and `y` are read as positions by the " +
      "ordering guards and by nothing here; the types of the three columns the address is computed " +
      "from are `addressing-is-unsigned`'s question, not this one's.",
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
    id: "addressing-is-unsigned",
    title: "The columns the address is computed from are unsigned integers",
    proves:
      "`dense_id` on a vertex payload and on its identity index, and `src_dense` and `dst_dense` " +
      "on each adjacency orientation, hold an unsigned integer. Those are the three names every " +
      "convention here **shifts**, and a name says nothing about what a shift will do to the " +
      "value under it: a `dense_id` stored as a string opens, describes and addresses until a " +
      "reader shifts it. Signed is the quieter half — `>>` sign-extends on a signed type, so a " +
      "negative value that should not exist addresses a tile that does not exist instead of " +
      "failing to parse.",
    cannotProve:
      "The **width**. `uint32` and `uint64` both address correctly and neither says which ceiling " +
      "the writer was working to; that a fossil corpus stores 32 bits and shifts 64 is stated on " +
      "the identity convention and is not derivable from a column. Nor that the manifest's " +
      "declared `data_type` is the spelling on disk: the manifest is read by a line scanner whose " +
      "grammar is a flat mapping and one sequence of small mappings, and a property list is one " +
      "level deeper than that goes.",
    run(corpus) {
      const failures = [];
      const seen = new Set();
      let checked = 0;
      // An ABSENT column is `plain-parquet`'s finding and not this one. Two guards reporting one
      // break is how a corpus comes back with the second-most-useful message at the top.
      const shifted = (types, column, where) => {
        const got = types?.get(column);
        if (got === undefined) return;
        checked += 1;
        seen.add(got);
        if (!UNSIGNED.has(got.toUpperCase())) {
          failures.push(`${where} carries ${column} as ${got}, which is not an unsigned integer`);
        }
      };
      for (const type of corpus.types) {
        shifted(type.columnTypes, "dense_id", type.name);
        if (type.index) shifted(type.index.columnTypes, "dense_id", `${type.name} ${type.index.prefix}/`);
      }
      for (const edge of corpus.edges) {
        for (const side of [edge.bySource, edge.byTarget]) {
          shifted(side.columnTypes, "src_dense", `${edge.rel} ${side.name}`);
          shifted(side.columnTypes, "dst_dense", `${edge.rel} ${side.name}`);
        }
      }
      return result(failures, [
        `${checked} address column(s) read · ${[...seen].sort().join(", ") || "none"}`,
      ]);
    },
  },

  {
    id: "declared-tiling",
    title: "The manifest declares the tiling that was emitted",
    proves:
      "`chunk_size` is a power of two, so `dense_id >> shift` addresses a tile and no division " +
      "does; the tile count on disk is the one the row count implies; the container on disk is " +
      "the one `graph.graph.yml` declares, for every payload set, because a reader over HTTP " +
      "cannot list a directory to find out; and an edge type's `src_chunk_size` equals its source " +
      "type's `chunk_size`, because an edge tile is addressed by the source's tile and a " +
      "different number there would address nothing.",
    cannotProve:
      "That the declared size is the *right* size. 4,096 is a measured trade-off between requests " +
      "and bytes, not an invariant, and a corpus may declare another power of two and be read.",
    run(corpus) {
      const failures = [];
      // The manifest against the disk, per payload set. `mixed` is caught below as the violation
      // it is; this catches the quieter one — a corpus that declares one container and carries the
      // other, where every guard here passes and every reader composes a URL that 404s.
      if (corpus.container !== "files" && corpus.container !== "rowgroups") {
        failures.push(
          `graph.graph.yml declares container ${corpus.container}; a tile is a file or a row group`,
        );
      }
      for (const [label, layout] of [
        ...corpus.types.flatMap((t) => [
          [t.name, t.layout],
          ...(t.index === null ? [] : [[`${t.name} index`, t.index.layout]]),
        ]),
        ...corpus.edges.flatMap((e) => [
          [`${e.rel} by_source`, e.bySource.layout],
          [`${e.rel} by_target`, e.byTarget.layout],
        ]),
      ]) {
        if (layout === "mixed") {
          failures.push(`${label} carries tiles two ways at once, so a reader that globs finds both`);
        } else if (layout !== "empty" && layout !== corpus.container) {
          failures.push(
            `${label} carries its tiles as ${layout} and graph.graph.yml declares ${corpus.container}`,
          );
        }
      }
      for (const type of corpus.types) {
        if (type.shift === null) {
          failures.push(`${type.name} declares a tile of ${type.chunkSize} rows, which no shift addresses`);
          continue;
        }
        const expected = Math.ceil(type.count / Number(type.chunkSize));
        if (type.layout === "files" && type.files.length !== expected) {
          failures.push(`${type.name} has ${type.files.length} tile files where ${expected} are implied`);
        }
      }
      // The same rule on the edge side, where it was not being applied and where it is the only
      // place it bites: a vertex type never carries two containers, and every relation does.
      // `by_source.parquet` beside `by_source/chunk{k}.parquet` is the uncut relation shipped next
      // to its own cut, which is what `<Type>.parquet` beside the vertex tiles already is — and
      // that one is a violation two guards down, in as many words, "a second copy of every vertex".
      // The asymmetry was never argued; it is the difference between a staging artefact that is
      // deleted and one that is published.
      //
      // It matters because the two are read by different consumers. A reader that registers its
      // views over `by_source.parquet` and the reference reader, which addresses the tiles, are two
      // readers disagreeing about where the corpus's bytes are — which is the failure this whole
      // suite exists to make impossible.
      for (const edge of corpus.edges) {
        for (const side of [edge.bySource, edge.byTarget]) {
          if (side.relation.length > 0 && side.tiles.length > 0) {
            failures.push(
              `${edge.rel} ${side.name}: ${side.name}.parquet is the uncut relation published beside ` +
                `its own tiles, so a reader that globs finds both`,
            );
          }
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
            failures.push(`${edge.rel} declares no projection at scale 1 aligned_by ${side.alignedBy}`);
          } else if (side.tilePrefix === "") {
            failures.push(
              `${edge.rel} declares a projection aligned_by ${side.alignedBy} with no path, ` +
                `so its tiles have no address`,
            );
          }
        }
        // And every projection carries a scale a shift addresses. A scale that is
        // not a power of two forces a division where the whole format is a shift,
        // and a projection with none is one nothing can address at all.
        for (const projection of [...edge.projections, ...(source?.projections ?? [])]) {
          if (projection.scale === null) {
            failures.push(
              `${edge.rel} declares a projection at ${projection.prefix}/ with no scale a shift addresses`,
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
    id: "declared-count",
    title: "The manifest says how many rows there are, and they are there",
    proves:
      "`vertex_count` is the rows across every tile of that type, and `edge_count` is the rows of " +
      "each orientation of that relation. **This is the only guard a truncated corpus fails.** " +
      "Tiles are addressed and never listed — that is the whole of the addressing — so a tree " +
      "holding `chunk0..chunk16` is indistinguishable from a corpus that has seventeen tiles. A " +
      "hole in the middle breaks `tile-of` and is caught; a missing tail breaks nothing, and the " +
      "corpus reads clean and short. Together with `dense-ids` it also fixes the largest id in the " +
      "corpus at `count − 1`, so a reader knows how far the ids go before it opens a file.",
    cannotProve:
      "That the count is the *right* count. It is written by the writer and checked against bytes " +
      "the same writer produced: both wrong together passes, and only a second writer would show " +
      "it. Nor which rows — a corpus that lost its tail and gained as many duplicates in the " +
      "middle satisfies this and fails `dense-ids` and `exactly-once`, which is where that lives.",
    run(corpus) {
      const failures = [];
      const notes = [];
      const rowsIn = (files) => BigInt(scalar(`SELECT count(*) FROM read_parquet(${fileList(files)})`));

      for (const type of corpus.types) {
        if (type.declared === null) {
          failures.push(
            `${type.name} declares no vertex_count, so a reader cannot tell a corpus that stops ` +
              `early from one that ends there`,
          );
          continue;
        }
        const onDisk = BigInt(type.count);
        if (onDisk !== type.declared) {
          failures.push(
            `${type.name} declares ${type.declared} vertices and the tiles hold ${onDisk}`,
          );
        }
        const tiles = tilesOf(type.declared, type.chunkSize);
        notes.push(
          `${type.name}: ${type.declared} declared, ${tiles ?? "?"} tile(s), ` +
            `the last holding ${tailRows(type.declared, type.chunkSize) ?? "?"}`,
        );
      }

      for (const edge of corpus.edges) {
        if (edge.declared === null) {
          failures.push(`${edge.rel} declares no edge_count`);
          continue;
        }
        // Each orientation on its own, because they are one relation stored
        // twice and a tail lost from one of them is a hop that answers short in
        // one direction. `one-relation-twice` compares them to each other and
        // would pass on both being truncated the same way.
        for (const side of [edge.bySource, edge.byTarget]) {
          if (side.tiles.length === 0) continue;
          const onDisk = rowsIn(side.tiles);
          if (onDisk !== edge.declared) {
            failures.push(
              `${edge.rel} ${side.name} declares ${edge.declared} edges and its tiles hold ${onDisk}`,
            );
          }
        }
        notes.push(`${edge.rel}: ${edge.declared} declared`);
      }
      return result(failures, notes);
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
      "`hilbert-order`, which is the guard that makes the numbering an address.",
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
      "form the corpus uses: against the number in the filename when a tile is a file, and against " +
      "the row-group ordinal when a tile is a row group of a FIXED-STRIDE set — the vertex payload " +
      "and the identity index, whose tiles are exactly `chunk_size` rows. An adjacency is not " +
      "fixed-stride, so what is checked there is the property a reader actually needs: the " +
      "row-group boxes on the key column ascend and do not overlap, so a tile is a contiguous run " +
      "of them. An edge is checked against the tile of the endpoint its file is ordered by — the " +
      "source for `by_source`, the destination for `by_target` — which is what CSR and CSC mean.",
    cannotProve:
      "That the shift is applied the same way elsewhere. This asks the corpus a question; the " +
      "published border vectors are what ask the *reader* one, and they are the next guard. And " +
      "in the row-group container it cannot ask an adjacency for its ordinal: a tile whose " +
      "vertices have no edges contributes no rows, so it has no row group to be numbered, and " +
      "there is nothing weaker to check than ascending boxes.",
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

      /**
       * The rule an adjacency satisfies in the row-group container, and the ordinal is not it.
       *
       * A tile's rows are contiguous when the boxes on the key column ascend and do not overlap,
       * which is what lets the footer turn a `dense_id` range into a run of row groups. Equal
       * endpoints are allowed: one vertex's edges can straddle a group boundary.
       */
      const checkAscending = (files, column, label, chunkSize) => {
        for (const [file, groups] of rowGroups(files)) {
          let previous = null;
          for (const [id, group] of [...groups].sort((a, b) => a[0] - b[0])) {
            const stats = group.stats.get(column);
            if (!stats || stats.min === null) {
              failures.push(`${label}: row group ${id} of ${file} carries no ${column} statistics`);
              continue;
            }
            const lo = BigInt(stats.min);
            if (previous !== null && lo < previous) {
              failures.push(
                `${label}: row group ${id} starts at ${column} ${lo} and the one before it ends at ${previous}`,
              );
            }
            previous = BigInt(stats.max);
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
            checkAscending(side.tiles, side.column, label, chunkSize);
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
      "`tile_of`, `tile_url`, `hilbert2`, the quantisation and the count-to-tiles arithmetic answer " +
      "what `guards/vectors.json` says they answer, at every border where a re-implementation " +
      "diverges: 2³¹ for a shift taken as signed, 2⁵³ for an id, a count or a tile number that " +
      "went through a JavaScript `Number`, bit 31 and the 3·2³⁰ step of a Hilbert index kept in a " +
      "signed accumulator, the reflection a port of the curve drops, a count that exactly fills a tile for the ceiling that writes an " +
      "empty one after it, and one path for every tile of a set for a port that carried the " +
      "file-per-tile composition into the other container. The vectors are the deliverable — they " +
      "are what gets copied.",
    cannotProve:
      "Anything about the corpus. This is a statement about a function, and it is here so that a " +
      "checker run against somebody else's corpus also checks the checker.",
    run() {
      return checkVectors(vectors());
    },
  },

  {
    id: "hilbert-order",
    title: "`dense_id` ascends with the Hilbert code of the position",
    proves:
      "The order is the index. Recomputing the code from the corpus's own `x` and `y` — quantised " +
      "over that vertex type's own extent, in binary32, the width the conventions specify — the " +
      "code never decreases as `dense_id` increases. This is what makes a rectangle in space a few " +
      "hundred contiguous runs of `dense_id` instead of a scan: a rectangle over a Hilbert order " +
      "breaks into O(√n) segments, and that is the whole mechanism.",
    cannotProve:
      "That the *layout* is any good — a Hilbert order over positions that mean nothing is still a " +
      "Hilbert order. And it is exact only because the width is specified: a writer that quantised " +
      "in binary64 can report inversions here that are ties in its own arithmetic, which is why " +
      "the conventions name the width rather than leaving it to be inferred.",
    run(corpus) {
      const failures = [];
      for (const type of corpus.types) {
        if (type.files.length === 0 || !type.columns.has("x") || !type.columns.has("y")) continue;
        const bad = scalar(
          `${hilbertSql(type.files)}
           SELECT count(*) FROM (SELECT hilbert, lag(hilbert) OVER (ORDER BY dense_id) AS prev FROM z)
            WHERE prev IS NOT NULL AND hilbert < prev`,
        );
        failures.push(...violations(bad, `${type.name}: dense_id ascends where the Hilbert code falls`));
      }
      return result(failures);
    },
  },

  {
    id: "spatial-tiles",
    title: "A tile is compact in space",
    proves:
      "What the order exists to produce, measured on the footer a reader actually has: the average " +
      "tile's bounding box covers at most half the corpus extent. A Hilbert-ordered corpus measures " +
      "far below that (0.080 on the fixture); the same vertices renumbered at random measure 0.99, " +
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
      "every id in range; only `hilbert-order` and an identity join catch that.",
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
      "A vertex payload set holds each `dense_id` once — across every tile of the type, so a row " +
      "copied from one tile into another is a failure and not a rounding. It also asserts that the " +
      "staged single-file vertex Parquet a layout pass consumes is *gone*: left behind it is a " +
      "second, stale copy of every vertex, the kind a reader picks up by globbing and never " +
      "questions.\n\n" +
      "**Two numbers, reported on every run and zero included.** How many vertex types this " +
      "corpus declares, and how many payload sets were actually read — because a corpus with no " +
      "type to read violates nothing and passes here, which is exactly how a corpus whose payload " +
      "the inspector failed to find passes too. An absent note and a zero note read identically to " +
      "whoever is reading the output, and only one of them is a measurement.",
    cannotProve:
      "That the rows are the right rows. Splitting a file is where rows are silently dropped, and " +
      "this catches a row written twice; a file that was split correctly from wrong content " +
      "passes.\n\n" +
      "**Nothing at all about the edge tiles.** This guard once diffed each orientation's tiles " +
      "against the uncut `by_source.parquet` they were cut from, which is the only way to assert " +
      "that a cut preserved a MULTISET. No writer publishes that file — a corpus carries an " +
      "orientation one way, as tiles — so the comparison had no second document and had never once " +
      "run. What covers the ground it claimed is `declared-count` per orientation and " +
      "`one-relation-twice` between them; what neither covers is recorded in " +
      "`/docs/design/discarded`.",
    run(corpus) {
      const failures = [];
      const notes = [];
      // What this run looked at, counted where it was looked at. A type whose payload the
      // inspector found no tiles for is skipped below, so `corpus.types.length` is not it.
      let read = 0;
      for (const type of corpus.types) {
        if (existsSync(type.staged)) {
          failures.push(`${type.name}: the staged ${type.prefix}.parquet is a second copy of every vertex`);
        }
        if (type.files.length === 0) {
          notes.push(`${type.name}: no payload tiles, so there is no row to count twice`);
          continue;
        }
        read += 1;
        const counted = query(
          `SELECT count(*) AS rows, count(DISTINCT dense_id) AS distinct_ids
             FROM read_parquet(${fileList(type.files)})`,
        )[0];
        const bad = Number(counted.rows) - Number(counted.distinct_ids);
        failures.push(...violations(bad, `${type.name}: a dense_id appears in more than one payload file`));
        notes.push(
          `${type.name}: ${counted.rows} row(s) over ${type.files.length} payload file(s), ` +
            `${counted.distinct_ids} distinct dense_id(s)` +
            (existsSync(type.staged) ? "" : ` — and no staged ${type.prefix}.parquet beside them`),
        );
      }
      // First, and present on every run. A guard that says nothing when it examined nothing is
      // indistinguishable from a guard that examined something and found it well-formed.
      return result(failures, [
        `${corpus.types.length} vertex type(s), ${read} payload set(s) read`,
        ...notes,
      ]);
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
      "without knowing the Hilbert range, and the boxes are what say it. One request buys the " +
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
      "not fail on it. It also never opens the manifest: a projection's `properties` carry an " +
      "`is_primary` per property, and a corpus that marks `dense_id` with it passes here " +
      "unremarked. That is how `crates/fossil-df` came to mark the address for months while this " +
      "sentence said it could not be one — the flag is nested a level deeper than this scanner " +
      "addresses, so checking it is a change to `manifest.mjs` and not to this run().",
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
  {
    id: "index-agrees-with-the-payload",
    title: "An identity index names exactly the rows the payload has",
    proves:
      "Where a vertex type declares an `index:`, it is the SAME rows a second time — every " +
      "`(subject, dense_id)` pair in the payload is in the index and nothing else is — its tiles " +
      "are sorted by the column it declares, and their ranges are disjoint. Those three together " +
      "are what let a reader binary-search the footers instead of scanning: a lookup lands in one " +
      "tile, and the row it finds there is the row the payload has. **An index that disagrees " +
      "with the payload is worse than no index at all**, because a scan finds nothing while a " +
      "wrong index returns a vertex that is plausible.",
    cannotProve:
      "That a reader USES it. Once this passes, the index and a scan return the same row by " +
      "construction, so a reader that ignores the index is correct and slow and no property of " +
      "the artefact tells the two apart. It also says nothing about a type that declares no " +
      "index: that is a legal corpus, and every one written before the field existed is one.",
    run(corpus) {
      const failures = [];
      const notes = [];
      for (const type of corpus.types) {
        if (type.index === null) {
          notes.push(`${type.name}: no index declared — a lookup by identity scans, which is legal`);
          continue;
        }
        const idx = type.index;
        if (idx.files.length === 0) {
          failures.push(`${type.name}: declares an index at ${idx.prefix}/ and no tile is there`);
          continue;
        }
        const key = idx.orderedBy;
        if (!type.columns.has(key)) {
          failures.push(`${type.name}: indexes ${key}, which the payload has no column for`);
          continue;
        }

        // The same rows, both ways round. `EXCEPT` one way catches a short index
        // and the other a stale one, and a corpus can be both at once.
        const pair = `${key}, dense_id`;
        const row = query(
          `WITH pay AS (SELECT ${pair} FROM read_parquet(${fileList(type.files)})),
                idx AS (SELECT ${pair} FROM read_parquet(${fileList(idx.files)}))
           SELECT (SELECT count(*) FROM (SELECT * FROM pay EXCEPT SELECT * FROM idx)) AS missing,
                  (SELECT count(*) FROM (SELECT * FROM idx EXCEPT SELECT * FROM pay)) AS stale`,
        )[0];
        failures.push(...violations(row.missing, `${type.name}: a payload row the index does not name`));
        failures.push(...violations(row.stale, `${type.name}: an index row the payload does not have`));

        // Disjoint BETWEEN tiles, which is what makes a footer search possible.
        // Sorted tiles with overlapping ranges cannot be searched at all.
        const bounds = query(
          `SELECT filename, min(${key}) AS lo, max(${key}) AS hi, count(*) AS rows,
                  count(*) FILTER (${key} IS NULL) AS nulls
             FROM read_parquet(${fileList(idx.files)}, filename = true)
            GROUP BY filename ORDER BY lo`,
        );
        failures.push(
          ...violations(
            bounds.reduce((n, b) => n + Number(b.nulls), 0),
            `${type.name}: an index row has no ${key}`,
          ),
        );
        for (let i = 1; i < bounds.length; i += 1) {
          if (String(bounds[i - 1].hi) >= String(bounds[i].lo)) {
            failures.push(
              `${type.name}: index tiles overlap — one ends at ${bounds[i - 1].hi} and the next ` +
                `starts at ${bounds[i].lo}, so no footer names the tile a lookup is in`,
            );
          }
        }
        notes.push(
          `${type.name}: ${idx.files.length} index tile(s) over ${key}, ` +
            `${bounds.reduce((n, b) => n + Number(b.rows), 0)} rows, ranges disjoint`,
        );
      }
      return result(failures, notes);
    },
  },
  {
    id: "declared-channels",
    title: "A declared channel names a column the payload has, on the domain it publishes",
    proves:
      "That every entry of a vertex type's `channels:` is drawable from the bytes: the `column` " +
      "is a column the payload actually carries, the `scale` is one of the two a reader " +
      "dispatches on, and — for a categorical — the declared `domain` is the number of distinct " +
      "values in that column, **recounted over the whole payload set** rather than over a tile. " +
      "The domain is the field the block exists for, because it is the one a reader cannot " +
      "recover: a quantitative channel's range is min/max in the footers, and a count of distinct " +
      "values is in no footer and costs a scan to find. A reader compares that number against " +
      "what it has to draw with *before* it draws, so a stale one is worse than an absent one — " +
      "absent, the reader knows it is guessing.\n\n" +
      "A quantitative entry is required to declare NO domain, for the same reason the categorical " +
      "must: declaring a range would be a second statement of what the footers carry, and the " +
      "second statement is the one that goes stale when a tile is rewritten.\n\n" +
      "Silence is not a violation and neither is an empty list, and the two are reported apart. " +
      "No `channels:` is a corpus whose writer said nothing — every corpus written before the " +
      "field is one — and an empty list is a writer declaring that this type carries none.",
    cannotProve:
      "That the domain FITS. A palette has a capacity, and a capacity is not a corpus fact: it " +
      "belongs to whatever is drawing, and the same corpus is well drawn by one reader and " +
      "unreadable in another. What the declaration changes is *when* the mismatch is knowable, " +
      "not whether it happens — this guard proves the number is true, and a reader is what " +
      "compares it against eight.\n\n" +
      "That `derived_by` is true. It says what computed the column, and nothing on disk records " +
      "what computed anything: a corpus claiming `louvain-cut` over a column somebody pasted in " +
      "passes here, and so does a derived column that claims nothing.\n\n" +
      "That the SCALE is the right reading of the column. An integer column can be read as a " +
      "measure or as a group and the bytes do not decide which — a `birth_year` declared " +
      "categorical with a truthful domain of 40 satisfies every line of this guard and draws a " +
      "picture nobody wants.\n\n" +
      "That the declaration is COMPLETE. A column a reader would happily colour by and no channel " +
      "names is invisible here, for the reason it is invisible to a reader: silence is legal, and " +
      "which columns are worth drawing is not a question the artefact answers.",
    run(corpus) {
      const SCALES = new Set(["categorical", "quantitative"]);
      const failures = [];
      const notes = [];
      for (const type of corpus.types) {
        if (type.channels === null) {
          notes.push(
            `${type.name}: no \`channels:\` — the writer says nothing, which is a corpus and not a claim`,
          );
          continue;
        }
        if (type.channels.length === 0) {
          notes.push(`${type.name}: declares no channel, which is not the same as saying nothing`);
          continue;
        }
        if (type.files.length === 0) {
          failures.push(
            `${type.name}: declares ${type.channels.length} channel(s) over a payload with no tiles`,
          );
          continue;
        }

        // ONE relation over every tile of the set, which is the line the domain
        // turns on: a distinct count per tile and summed is a different number
        // and a larger one, and it is the easiest wrong answer available here.
        const relation = `read_parquet(${fileList(type.files)})`;
        const named = new Set();
        let recounted = 0;
        for (const channel of type.channels) {
          const where = `${type.name}.${channel.name || "«unnamed»"}`;
          if (channel.name === "") {
            failures.push(`${type.name}: a channel with no name, which a reader cannot ask for`);
          } else if (named.has(channel.name)) {
            failures.push(`${where}: two channels of one name, so asking for it by name is ambiguous`);
          }
          named.add(channel.name);

          if (!SCALES.has(channel.scale)) {
            failures.push(
              `${where}: a scale of \`${channel.scale}\`, which is neither \`categorical\` nor ` +
                "`quantitative` — a reader dispatches on it rather than displaying it",
            );
            continue;
          }
          // The failure a declaration introduces that a derivation could not
          // have: a derived column is the column it was derived from, and a
          // named one is a name that can be wrong.
          if (!type.columns.has(channel.column)) {
            failures.push(
              `${where}: names column \`${channel.column}\`, which the payload has no column for`,
            );
            continue;
          }
          if (channel.scale === "quantitative") {
            if (channel.declaresDomain) {
              failures.push(
                `${where}: is quantitative and declares a domain — its range is min/max in the ` +
                  "footers, and a second statement of what the bytes carry is the one that goes stale",
              );
            }
            notes.push(`${where}: quantitative over ${channel.column}, range read from the footers`);
            continue;
          }
          if (!channel.declaresDomain) {
            failures.push(
              `${where}: is categorical and declares no domain, which is the state the block ` +
                "exists to remove — no footer holds a distinct count",
            );
            continue;
          }
          if (channel.domain === null) {
            failures.push(`${where}: declares a domain that is not a count of distinct values`);
            continue;
          }

          const quoted = `"${channel.column.replace(/"/g, '""')}"`;
          const row = query(
            `SELECT count(DISTINCT ${quoted}) AS distinct_values,
                    count(*) FILTER (${quoted} IS NULL) AS absent
               FROM ${relation}`,
          )[0];
          recounted += 1;
          const distinct = BigInt(row.distinct_values);
          const absent = BigInt(row.absent);
          if (distinct !== channel.domain) {
            failures.push(
              `${where}: declares a domain of ${channel.domain} and \`${channel.column}\` holds ` +
                `${distinct} distinct value(s) — that number is what a reader weighs against what ` +
                "it has to draw with, before it draws",
            );
          }
          notes.push(
            `${where}: categorical over ${channel.column}, ${distinct} distinct value(s)` +
              // A null is not a value of the domain — SQL counts it out, and a
              // reader drawing the column has rows it has no slot for. Reported
              // rather than counted in: which of the two a writer meant is not
              // something the bytes say.
              (absent > 0n ? `, ${absent} row(s) carrying none` : "") +
              (channel.derivedBy === null ? "" : ` · derived by ${channel.derivedBy}`),
          );
        }
        notes.push(
          `${type.name}: ${type.channels.length} channel(s) declared, ${recounted} domain(s) ` +
            "recounted over the whole payload",
        );
      }
      return result(failures, notes);
    },
  },
  {
    id: "mode-names-a-channel",
    title: "A cell tree's `mode` names a channel the type declares",
    proves:
      "That the reference a cell tree makes RESOLVES: the channel it names its rungs' `mode` " +
      "after is one of the entries in that same vertex type's `channels:`, and that entry is " +
      "categorical. A cell row's `mode` is a bare `uint32` and the column says nothing about what " +
      "it is the mode OF — which palette the values belong to, or how many of them there are — so " +
      "the tree names the channel and a reader follows one hop to the entry that measured the " +
      "domain. That is the same question `declared-channels` answers one level down, asked of the " +
      "summary instead of the payload.\n\n" +
      "**A name and not a copy, which is why this guard exists at all.** The domain stays written " +
      "once, on the channel, so the number `declared-channels` recounts off the Parquet is the " +
      "number a reader arrives at through this reference; a tree carrying its own copy would need " +
      "both checked and would go stale in exactly one of them. What a name buys in one place it " +
      "costs in another: a dangling reference is a well-formed document that answers neither " +
      "question, and nothing in either block can see the other.\n\n" +
      "Two states and not three, which is where this departs from `channels:` and `coordinates:`. " +
      "No `mode_channel` is a tree whose writer did not say — every tree written before the field " +
      "is one — and it is reported rather than failed. There is no empty state to tell apart: a " +
      "rung's `mode` always holds the mode of something, so «declares none» is not an answer a " +
      "reference can give.\n\n" +
      "**Two numbers, reported on every run and zero included.** How many trees this corpus " +
      "declares, and how many references were followed into a channel — because a corpus " +
      "declaring no tree violates nothing and passes here, which is exactly how a corpus that " +
      "SHOULD carry one and lost it passes too. `cells:` is optional on the way in and no " +
      "manifest struct refuses a key it does not know, so a writer and a reader spelling that key " +
      "differently produce a document with the tree ABSENT, and nothing in it distinguishes that " +
      "from a document whose writer never had a pyramid to declare. An absent note and a zero " +
      "note read identically to whoever is reading the output, and only one of them is a " +
      "measurement — so the count is stated rather than implied by the notes that happen to be " +
      "there. What makes a zero loud is not this guard: it is `self-test.mjs`, which requires the " +
      "corpus `fixture.mjs` writes to make both numbers non-zero.",
    cannotProve:
      "That the rungs' `mode` values ARE that column's mode. This corpus publishes no rungs, and " +
      "the claim is about bytes rather than about a document: it is evaluated where the rungs are " +
      "written, by `crates/fossil-layout/tests/cells.rs`, which recounts the majority and its " +
      "share per cell against the payload — over the column this same reference resolves to.\n\n" +
      "That the channel named is the RIGHT one. A type declaring two categoricals can name either " +
      "and both resolve; which of them a summary should be of is a writer's decision, and the " +
      "artefact records it rather than justifying it.\n\n" +
      "That a tree which says nothing should have said something. Silence is legal and every tree " +
      "written before the field is silent — a reader colouring a rung against a palette it " +
      "guessed is guessing, and this guard is what leaves it knowing that.",
    run(corpus) {
      const failures = [];
      const notes = [];
      // What this run looked at, counted where it was looked at and never derived from
      // `notes.length` — a note is prose and several of them say a reference was NOT followed.
      let trees = 0;
      let followed = 0;
      for (const type of corpus.types) {
        if (type.cells === null) {
          notes.push(`${type.name}: declares no cell tree, so there is no reference to resolve`);
          continue;
        }
        trees += 1;
        const named = type.cells.modeChannel;
        if (named === null) {
          notes.push(
            `${type.name}: a tree whose \`mode\` names no channel — the writer did not say, which is a corpus and not a claim`,
          );
          continue;
        }
        if (type.channels === null || type.channels.length === 0) {
          failures.push(
            `${type.name}: the tree's \`mode\` names \`${named}\` and the type declares ` +
              `${type.channels === null ? "no channels at all" : "an empty list of channels"}, ` +
              "so the name resolves against nothing",
          );
          continue;
        }
        const resolved = type.channels.find((channel) => channel.name === named);
        if (resolved === undefined) {
          failures.push(
            `${type.name}: the tree's \`mode\` names \`${named}\`, which is none of the ` +
              `${type.channels.length} channel(s) this type declares ` +
              `(${type.channels.map((c) => c.name || "«unnamed»").join(", ")}) — a reader ` +
              "following it gets neither a column nor a domain",
          );
          continue;
        }
        if (resolved.scale !== "categorical") {
          failures.push(
            `${type.name}: the tree's \`mode\` names \`${named}\`, declared \`${resolved.scale}\` — ` +
              "a mode is the majority value of a category, and a measure has no majority to take",
          );
          continue;
        }
        followed += 1;
        notes.push(
          `${type.name}: \`mode\` is \`${named}\` over ${resolved.column}` +
            (resolved.domain === null ? "" : `, ${resolved.domain} value(s) wide`) +
            " — resolved, not derived",
        );
      }
      // First, and present on every run. A guard that says nothing when it examined nothing is
      // indistinguishable from a guard that examined something and found it well-formed.
      return result(failures, [
        `${corpus.types.length} vertex type(s), ${trees} cell tree(s) declared, ` +
          `${followed} reference(s) resolved`,
        ...notes,
      ]);
    },
  },
  {
    id: "tile-manifest",
    title: "The tile manifest says what the footers say",
    proves:
      "Where a vertex type names a tile manifest, the file is there and it lists exactly the tiles " +
      "on disk — the payload at the highest `z` and each rung below it, rung `k` of `R` at " +
      "`z = R − k` — and for every one of them the row count, every column's null count and every " +
      "numeric column's lower and upper bound equal the footer's, with no bound published for a " +
      "string. A reader plans a scan off this file and never opens a footer to do it, so a wrong " +
      "bound is a tile pruned that held the match, and nothing downstream can notice.\n\n" +
      "**The edges a tile addresses are held the same way.** Every adjacency orientation cut on the " +
      "type's tiles, at the payload's `z`, and every rung's quotient below it: each entry numbered by " +
      "the tile its rows are cut on — the row group's lower bound on the aligned column, shifted — " +
      "and folded over the row groups that tile is made of. A tile of sources with no edges has no " +
      "entry and no row group, and one side missing a tile the other lists is a failure.\n\n" +
      "**A float is compared as the `f32` it is.** The footer prints `0.1` and the manifest prints " +
      "the widened value; both are cast to the column's physical type before they are compared, so " +
      "the check is exact rather than approximate and a manifest that printed the short decimal " +
      "would still pass — it names the same `f32`.\n\n" +
      "A type naming no tile manifest is reported, not failed: every corpus written before the " +
      "field is one, and its reader falls back to the footers.",
    cannotProve:
      "That the footers are true. This holds the manifest to the footers and the footers to " +
      "nothing: `footer-is-the-index` has the same limit, and only re-reading every page would " +
      "lift it.\n\n" +
      "That a rung is where this looks. The manifest scanner reads one level of nesting and the " +
      "rungs are deeper, so a rung is found at `<cells.prefix>/r{k}/` — the naming fossil's writer " +
      "uses — and its quotient at `quotient/` under it, not at whatever `path` the tree declares. A " +
      "writer that names them otherwise is reported as a manifest listing zooms with no bytes " +
      "under them.\n\n" +
      "That an orientation left out had to be. An adjacency whose row groups straddle two tiles — " +
      "DuckDB cuts a row-group container by row count, not on tile boundaries — has no per-tile " +
      "statistics to publish, so its absence is reported; an orientation left out whose row groups " +
      "do fall on tiles is a failure, and this cannot tell a writer that forgot from one that chose.",
    run(corpus) {
      const failures = [];
      const notes = [];
      let checked = 0;
      for (const type of corpus.types) {
        if (type.tileManifest === null) {
          notes.push(`${type.name}: names no tile manifest, so a reader reads the footers`);
          continue;
        }
        if (!existsSync(type.tileManifest)) {
          failures.push(`${type.name}: names a tile manifest at ${type.tileManifest} and there is none`);
          continue;
        }
        const manifest = lit(type.tileManifest);
        const top = Number(
          scalar(
            `SELECT coalesce(max((m->>'z')::INTEGER), -1)
               FROM (SELECT unnest(json_extract(json, '$.matrices[*]')) AS m
                       FROM read_json_objects('${manifest}'))`,
          ),
        );
        if (top < 0) {
          failures.push(`${type.name}: the tile manifest lists no zoom at all`);
          continue;
        }
        const cellRoot = join(corpus.root, type.prefix, type.cells?.prefix ?? "cell");
        const onDisk = existsSync(cellRoot)
          ? readdirSync(cellRoot).filter((name) => /^r\d+$/.test(name)).length
          : 0;
        if (onDisk !== top) {
          failures.push(
            `${type.name}: the tile manifest's payload is at z = ${top}, which says ${top} rung(s), ` +
              `and ${onDisk} are on disk`,
          );
        }
        const shift = type.shift === null ? null : Number(type.shift);
        const zooms = type.files.map((f) => ({ z: top, path: f.path, tile: f.tile }));
        for (let k = 1; k <= top; k += 1) {
          for (const f of payload(join(cellRoot, `r${k}`))) {
            zooms.push({ z: top - k, path: f.path, tile: f.tile });
          }
          for (const f of payload(join(cellRoot, `r${k}`, "quotient"))) {
            zooms.push({ z: top - k, part: "quotient", path: f.path, tile: f.tile, key: "src_cell", shift });
          }
        }
        // Every orientation cut on this type's tiles. One whose row groups straddle a tile boundary
        // has no entry to hold, and the manifest leaving it out is the answer rather than a gap.
        const published = new Set(
          query(
            `SELECT DISTINCT a->>'edge_type' AS edge_type, a->>'src_type' AS src_type,
                    a->>'dst_type' AS dst_type, a->>'aligned_by' AS aligned_by
               FROM (SELECT unnest(json_extract(m, '$.adjacencies[*]')) AS a
                       FROM (SELECT unnest(json_extract(json, '$.matrices[*]')) AS m
                               FROM read_json_objects('${manifest}')))`,
          ).map(partOf),
        );
        for (const edge of corpus.edges) {
          for (const side of [edge.bySource, edge.byTarget]) {
            const aligned = side.alignedBy === "src" ? edge.srcType : edge.dstType;
            if (aligned !== type.name || side.tiles.length === 0) continue;
            const part = partOf({
              edge_type: edge.edgeType,
              src_type: edge.srcType,
              dst_type: edge.dstType,
              aligned_by: side.alignedBy,
            });
            const files = side.tiles.map((f) => ({ z: top, part, path: f.path, tile: f.tile, key: side.column, shift }));
            if (!published.has(part)) {
              const straddles = query(`SELECT bool_or(straddles) AS s FROM (${tileSql(files)})`)[0]?.s === true;
              (straddles ? notes : failures).push(
                straddles
                  ? `${type.name}: ${edge.edgeType} ${side.name} has row groups across tile boundaries, so it publishes no entries`
                  : `${type.name}: ${edge.edgeType} ${side.name} is cut on this type's tiles and the manifest lists none of them`,
              );
              continue;
            }
            zooms.push(...files);
          }
        }
        const mismatches = query(`
          WITH m AS (SELECT unnest(json_extract(json, '$.matrices[*]')) AS m
                       FROM read_json_objects('${manifest}')),
               a AS (SELECT (m->>'z')::INTEGER AS z, unnest(json_extract(m, '$.adjacencies[*]')) AS a FROM m),
               t AS (SELECT (m->>'z')::INTEGER AS z, '' AS part, unnest(json_extract(m, '$.tiles[*]')) AS t FROM m
                     UNION ALL
                     SELECT (m->>'z')::INTEGER, 'quotient', unnest(json_extract(m, '$.quotient[*]')) FROM m
                     UNION ALL
                     SELECT z, concat_ws(':', a->>'edge_type', a->>'src_type', a->>'dst_type', a->>'aligned_by'),
                            unnest(json_extract(a, '$.tiles[*]')) FROM a),
               published AS (
                 SELECT z, part, (t->>'tile')::BIGINT AS tile, (t->>'record_count')::BIGINT AS rows,
                        k.key AS col, k.value::BIGINT AS nulls,
                        t->'lower_bounds'->>k.key AS lo, t->'upper_bounds'->>k.key AS hi
                   FROM t, json_each(t->'null_value_counts') k),
               footer AS (${tileSql(zooms)})
          SELECT coalesce(p.z, f.z) AS z, coalesce(p.part, f.part) AS part,
                 coalesce(p.tile, f.tile) AS tile, coalesce(p.col, f.col) AS col,
                 p.rows AS p_rows, f.rows AS f_rows, p.nulls AS p_nulls, f.nulls AS f_nulls,
                 p.lo AS p_lo, f.lo AS f_lo, p.hi AS p_hi, f.hi AS f_hi, f.straddles
            FROM published p FULL JOIN footer f USING (z, part, tile, col)
           WHERE f.straddles OR p.rows IS DISTINCT FROM f.rows OR p.nulls IS DISTINCT FROM f.nulls
              OR CASE WHEN f.type = 'FLOAT'
                        THEN p.lo::FLOAT IS DISTINCT FROM f.lo::FLOAT OR p.hi::FLOAT IS DISTINCT FROM f.hi::FLOAT
                      WHEN f.type = 'DOUBLE'
                        THEN p.lo::DOUBLE IS DISTINCT FROM f.lo::DOUBLE OR p.hi::DOUBLE IS DISTINCT FROM f.hi::DOUBLE
                      ELSE p.lo::HUGEINT IS DISTINCT FROM f.lo::HUGEINT OR p.hi::HUGEINT IS DISTINCT FROM f.hi::HUGEINT END
           ORDER BY 1, 2, 3, 4`);
        for (const row of mismatches.slice(0, 5)) {
          const where = `z ${row.z}${row.part === "" ? "" : ` ${row.part}`} tile ${row.tile}`;
          failures.push(
            row.straddles === true
              ? `${type.name}: ${where} shares a row group with the next tile, so no entry describes it`
              : `${type.name}: ${where} \`${row.col}\` — the manifest says ` +
                  `rows ${row.p_rows}, nulls ${row.p_nulls}, [${row.p_lo}, ${row.p_hi}] and the footer ` +
                  `says rows ${row.f_rows}, nulls ${row.f_nulls}, [${row.f_lo}, ${row.f_hi}]`,
          );
        }
        if (mismatches.length > 5) {
          failures.push(`${type.name}: … and ${mismatches.length - 5} more disagreement(s)`);
        }
        const counted = query(
          `SELECT count(DISTINCT (z, tile)) FILTER (WHERE part = '') AS tiles,
                  count(DISTINCT (z, part, tile)) FILTER (WHERE part <> '') AS edges
             FROM (${tileSql(zooms)})`,
        )[0];
        checked += Number(counted.tiles) + Number(counted.edges);
        notes.push(
          `${type.name}: ${counted.tiles} tile(s) over ${top + 1} zoom(s) and ${counted.edges} edge ` +
            `tile(s) held against their footers`,
        );
      }
      return result(failures, [`${checked} tile(s) checked`, ...notes]);
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

