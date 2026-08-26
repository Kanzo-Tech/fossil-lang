/**
 * What is on disk, before any guard has an opinion about it.
 *
 * The guards ask questions; this file answers *what there is to ask about*. Keeping the two apart
 * is what lets a guard be a single sentence of SQL with a name — and it is where the one genuinely
 * open convention lives, so it is stated here rather than buried in a check.
 *
 * **A tile is a fixed 4,096-row range of `dense_id`. Which container carries it is a second
 * question, and the corpus answers it by construction rather than by declaring it.** Two containers
 * are addressed by the same arithmetic and measured against each other:
 *
 *   - **one file per tile** — `chunk{k}.parquet`, the address in the name;
 *   - **one file, one row group per tile** — the address is the row-group ordinal.
 *
 * They cost differently and the difference is measured, not argued: 22.3 requests per window
 * against 5.6, and 1.15 MB of footer against 496 kB, at five million vertices. A file boundary is
 * the only thing that stops contiguous tiles from being fetched in one range. Both are readable,
 * so this module classifies rather than judges, and the guard that cares says which it found.
 *
 * **What is on disk is one answer and what the manifest declares is another.** A checker can list a
 * directory; a reader over HTTP cannot, which is why `graph.graph.yml` carries `container` and why
 * `declared-tiling` compares the two. This module reports both and reconciles neither.
 */

import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { lit, query, scalar } from "./duck.mjs";
import { load, rel, vertexPrefix, edgePrefix } from "./manifest.mjs";
import { shiftFor } from "./arithmetic.mjs";

/**
 * A count a manifest declares, as a `BigInt`, or `null` when the field is not there.
 *
 * `null` and not `0`, because the two are different findings and only one of them is a corpus. A
 * declared `0` is an empty type and legal; an absent count is a manifest that cannot be checked for
 * truncation at all, which `declared-count` reports as the broken convention it is. Anything that is
 * not a non-negative integer is also `null` — a scanner that guessed would be deciding what the
 * writer meant.
 */
function declaredCount(value) {
  if (value === undefined || !/^\d+$/.test(String(value).trim())) return null;
  return BigInt(String(value).trim());
}

/**
 * Parquet files directly inside `dir`, **in tile order**, with the tile number their name claims.
 *
 * By the number and not by the name: `tile10.parquet` sorts before `tile2.parquet` as a string, and
 * a guard that reads the list as one relation then sees the rows in an order no writer produced.
 * That was invisible for as long as the uncut relation was published beside the tiles, because
 * every ordering guard preferred the single file and never read the set — `csr-and-csc` reports one
 * disorder per orientation the moment it does. A file whose name claims no tile keeps its place by
 * name, since there is no number to sort it by.
 */
function payload(dir) {
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".parquet"))
    .map((name) => {
      const named = /^(?:chunk|tile)(\d+)\.parquet$/.exec(name);
      return { name, path: join(dir, name), tile: named ? BigInt(named[1]) : null };
    })
    .sort((a, b) => {
      if (a.tile === null || b.tile === null) return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
      return a.tile < b.tile ? -1 : a.tile > b.tile ? 1 : 0;
    });
}

/**
 * How a set of payload files carries its tiles.
 *
 * `files` — every file names the tile it holds. `rowgroups` — one file, and the row-group ordinal
 * is the address. `mixed` is a violation and not a third layout: two ways to address the same rows
 * is one way too many, and a reader that globs finds both.
 */
function layoutOf(files) {
  if (files.length === 0) return "empty";
  const named = files.filter((f) => f.tile !== null).length;
  if (named === files.length) return "files";
  if (named === 0 && files.length === 1) return "rowgroups";
  return "mixed";
}

/** A DuckDB list literal over a set of paths, so one query spans a whole payload set. */
export function fileList(files) {
  return `[${files.map((f) => `'${lit(f.path)}'`).join(", ")}]`;
}

/** The column names of a payload set. */
function columnsOf(files) {
  if (files.length === 0) return new Set();
  const rows = query(`DESCRIBE SELECT * FROM read_parquet(${fileList(files)})`);
  return new Set(rows.map((r) => String(r.column_name)));
}

/**
 * Per-row-group footer statistics, keyed by file, for one payload set.
 *
 * `min_value`/`max_value` first, `min`/`max` only as a fallback. Parquet's original statistics
 * fields were defined by *signed* byte comparison, which is meaningless for an unsigned column, so
 * a writer that gets this right leaves them empty and fills `min_value`/`max_value` instead —
 * DuckDB does, and `dense_id` is unsigned. A reader that only knows the deprecated pair concludes
 * the footer carries no box for exactly the column the whole address is built on.
 */
export function rowGroups(files) {
  const rows = query(
    `SELECT file_name, row_group_id, row_group_num_rows, path_in_schema,
            coalesce(stats_min_value, stats_min) AS stats_min,
            coalesce(stats_max_value, stats_max) AS stats_max
       FROM parquet_metadata(${fileList(files)})`,
  );
  const byFile = new Map();
  for (const row of rows) {
    const file = String(row.file_name);
    if (!byFile.has(file)) byFile.set(file, new Map());
    const groups = byFile.get(file);
    const id = Number(row.row_group_id);
    if (!groups.has(id)) groups.set(id, { rows: Number(row.row_group_num_rows), stats: new Map() });
    groups.get(id).stats.set(String(row.path_in_schema), {
      min: row.stats_min === null ? null : String(row.stats_min),
      max: row.stats_max === null ? null : String(row.stats_max),
    });
  }
  return byFile;
}

/**
 * Read a corpus into the shape the guards ask questions of.
 *
 * Nothing here fails on a violation — a missing file becomes an absent entry and a guard reports
 * it. An inspector that threw would decide which violation a reader hears about first.
 */
export function inspect(root) {
  const manifest = load(root);

  const types = manifest.vertices.map((info) => {
    const prefix = vertexPrefix(info);
    const files = payload(join(root, prefix));
    const chunkSize = BigInt(info.chunk_size ?? 0);
    return {
      name: String(info.type ?? ""),
      rel: info.rel,
      prefix,
      /** What the manifest says the row count is, against which the disk is checked. */
      declared: declaredCount(info.vertex_count),
      chunkSize,
      shift: shiftFor(chunkSize),
      files,
      layout: layoutOf(files),
      columns: columnsOf(files),
      count: files.length === 0 ? 0 : Number(scalar(`SELECT count(*) FROM read_parquet(${fileList(files)})`)),
      /** The staged single file the layout pass consumes. A reader that globs picks it up. */
      staged: join(root, `${prefix}.parquet`),
      /**
       * The identity index, when the manifest declares one, or `null`.
       *
       * `null` is a legal corpus rather than an incomplete one: a lookup by
       * subject answers without it, by scanning, so its absence is a cost and
       * not a gap. Every corpus written before the field existed reads this way.
       */
      index: (() => {
        const declared = info.index;
        if (declared === undefined || declared === null) return null;
        const indexPrefix = String(declared.prefix ?? "").replace(/\/+$/, "");
        if (indexPrefix === "") return null;
        const tiles = payload(join(root, prefix, indexPrefix));
        return {
          prefix: indexPrefix,
          orderedBy: String(declared.ordered_by ?? ""),
          chunkSize: BigInt(declared.chunk_size ?? 0),
          files: tiles,
          layout: layoutOf(tiles),
        };
      })(),
    };
  });

  const edges = manifest.edges.map((info) => {
    const prefix = edgePrefix(info);
    const adjLists = Array.isArray(info.adj_lists) ? info.adj_lists : [];
    // Where an orientation's tiles are comes from the `adj_list` that declares
    // it, never from the convention that names them. `aligned_by` says which
    // endpoint column addresses the tiles and `prefix` says where they are, and
    // between them a reader turns a `dense_id` into a URL with nothing agreed out
    // of band. An orientation the manifest declares without a prefix has tiles
    // nobody can address, which `declared-tiling` reports; here it is simply an
    // orientation with none.
    const orientation = (alignedBy, name, column) => {
      const declared = adjLists.find((a) => String(a.aligned_by ?? "") === alignedBy) ?? null;
      const tilePrefix = String(declared?.prefix ?? "").replace(/\/+$/, "");
      const file = join(root, prefix, `${name}.parquet`);
      const tiles = tilePrefix === "" ? [] : payload(join(root, prefix, tilePrefix));
      return {
        name,
        alignedBy,
        column,
        declared,
        tilePrefix,
        relation: existsSync(file) ? [{ name: `${name}.parquet`, path: file, tile: null }] : [],
        tiles,
        layout: layoutOf(tiles),
      };
    };
    return {
      rel: info.rel,
      prefix,
      srcType: String(info.src_type ?? ""),
      dstType: String(info.dst_type ?? ""),
      edgeType: String(info.edge_type ?? ""),
      /** One number for both orientations: they are one relation stored twice. */
      declared: declaredCount(info.edge_count),
      chunkSize: BigInt(info.chunk_size ?? 0),
      srcChunkSize: BigInt(info.src_chunk_size ?? 0),
      dstChunkSize: BigInt(info.dst_chunk_size ?? 0),
      adjLists,
      bySource: orientation("src", "by_source", "src_dense"),
      byTarget: orientation("dst", "by_target", "dst_dense"),
    };
  });

  for (const edge of edges) {
    for (const side of [edge.bySource, edge.byTarget]) {
      side.columns = columnsOf([...side.relation, ...side.tiles]);
    }
  }

  return {
    root,
    manifest,
    /** What `graph.graph.yml` says the container is. Absent is the file-per-tile one. */
    container: manifest.index.container === undefined ? "files" : String(manifest.index.container),
    /**
     * What `graph.graph.yml` says the bytes guarantee, or `null`.
     *
     * **`null` and `{bound: "undeclared"}` are different findings and neither is
     * "public".** An absent key is a corpus written before the field existed; a
     * declared `undeclared` is a producer that knows about the field and is
     * saying this release carries no bound. A reader that collapsed the two
     * would be reporting the age of a writer as a property of the data.
     *
     * It scans as a flat mapping of scalars because that is the only shape this
     * scanner reads — see `manifest.mjs`. A producer emitting the set as a YAML
     * sequence would find it silently absent here, which is why the Rust that
     * writes it has a test asserting the shape rather than a comment asking for
     * it.
     */
    privacy: manifest.index.privacy === undefined ? null : manifest.index.privacy,
    types,
    edges,
    rel,
  };
}
