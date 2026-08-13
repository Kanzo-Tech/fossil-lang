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
 */

import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { lit, query, scalar } from "./duck.mjs";
import { load, rel, vertexPrefix, edgePrefix } from "./manifest.mjs";
import { shiftFor } from "./arithmetic.mjs";

/** Parquet files directly inside `dir`, sorted, with the tile number their name claims. */
function payload(dir) {
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".parquet"))
    .sort()
    .map((name) => {
      const named = /^(?:chunk|tile)(\d+)\.parquet$/.exec(name);
      return { name, path: join(dir, name), tile: named ? BigInt(named[1]) : null };
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
      chunkSize,
      shift: shiftFor(chunkSize),
      files,
      layout: layoutOf(files),
      columns: columnsOf(files),
      count: files.length === 0 ? 0 : Number(scalar(`SELECT count(*) FROM read_parquet(${fileList(files)})`)),
      /** The staged single file the layout pass consumes. A reader that globs picks it up. */
      staged: join(root, `${prefix}.parquet`),
    };
  });

  const edges = manifest.edges.map((info) => {
    const prefix = edgePrefix(info);
    const orientation = (name) => {
      const file = join(root, prefix, `${name}.parquet`);
      const tiles = payload(join(root, prefix, name));
      return {
        name,
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
      chunkSize: BigInt(info.chunk_size ?? 0),
      srcChunkSize: BigInt(info.src_chunk_size ?? 0),
      dstChunkSize: BigInt(info.dst_chunk_size ?? 0),
      adjLists: Array.isArray(info.adj_lists) ? info.adj_lists : [],
      bySource: orientation("by_source"),
      byTarget: orientation("by_target"),
    };
  });

  for (const edge of edges) {
    for (const side of [edge.bySource, edge.byTarget]) {
      side.columns = columnsOf([...side.relation, ...side.tiles]);
    }
  }

  return { root, manifest, types, edges, rel };
}
