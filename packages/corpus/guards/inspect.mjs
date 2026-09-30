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
export function payload(dir) {
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

/**
 * The columns of a payload set: the names, and the type under each name.
 *
 * One `DESCRIBE` answers both, and the second half is not decoration. Every convention here is
 * addressed by NAME — `dense_id`, `src_dense`, `x` — and a name says nothing about what a shift
 * will do to the value under it. `plain-parquet` proves the names are present and says in its own
 * `cannotProve` that it cannot reach the types; `addressing-is-unsigned` is where they are reached.
 */
function columnsOf(files) {
  if (files.length === 0) return { names: new Set(), types: new Map() };
  const rows = query(`DESCRIBE SELECT * FROM read_parquet(${fileList(files)})`);
  return {
    names: new Set(rows.map((r) => String(r.column_name))),
    types: new Map(rows.map((r) => [String(r.column_name), String(r.column_type)])),
  };
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
 * The channels a vertex type declares, or `null` when it declares none of them and `[]` when it
 * declares that it has none.
 *
 * **Three states, and the first two are different findings.** An absent `channels:` is a writer that said nothing, which is every corpus
 * written before the field existed; an empty list is a writer saying this type carries no channel.
 * A reader that collapsed the two would be reporting the age of a writer as a property of the data.
 *
 * `domain` is kept twice on purpose: `declaresDomain` is whether the key is *there*, and `domain`
 * is the count under it. A quantitative entry owes the first to be false, and a categorical one
 * owes a number — and `domain: many` is a third thing, which a single nullable field would report
 * as the second.
 */
function channelsOf(info) {
  const declared = info.channels;
  if (declared === undefined) return null;
  // `channels: []` scans as the two-character string, the way `projections: []` does: the flow form
  // of an empty collection is the one flow form this scanner reads, and it carries no entries
  // either way.
  if (!Array.isArray(declared)) return [];
  return declared.map((entry) => {
    const domain = String(entry?.domain ?? "").trim();
    return {
      name: String(entry?.name ?? ""),
      column: String(entry?.column ?? ""),
      scale: String(entry?.scale ?? ""),
      /** Whether the key is written at all, which is what the scale owes an answer about. */
      declaresDomain: entry?.domain !== undefined,
      /** The count under it, or `null` for a key that is there and is not a count. */
      domain: /^\d+$/.test(domain) ? BigInt(domain) : null,
      derivedBy: entry?.derived_by === undefined ? null : String(entry.derived_by),
    };
  });
}

/**
 * The cell tree a vertex type declares, or `null` when it declares none.
 *
 * **Two fields, and the second is the one a guard reads.** `declared` is whether the block is there
 * at all; `modeChannel` is the name the tree gives the channel its rungs' `mode` summarises, or
 * `null` where the tree does not say. A tree written before that field said nothing and draws
 * exactly as it drew, so absence is a corpus and not a claim — `channels:`'s rule, one artefact
 * along, minus the empty state a reference cannot have: a rung's `mode` is always the mode of
 * something.
 *
 * The rungs are deliberately not read here. `manifest.mjs` scans one level of mapping and skips
 * anything deeper, so a rung — a mapping inside a sequence inside a mapping — is out of its grammar
 * and would have to be guessed at. Nothing in this directory opens a rung either: what is checked
 * is the reference, and the bytes under it are `crates/fossil-layout/tests/cells.rs`'s to evaluate.
 */
function cellsOf(info) {
  const declared = info.cells;
  if (declared === undefined) return null;
  // A key with an empty value and no `k: v` child scans as `[]`, which carries nothing; a tree
  // written that way has declared no field this reads.
  const tree = Array.isArray(declared) ? {} : declared;
  const named = tree.mode_channel === undefined ? null : String(tree.mode_channel).trim();
  const prefix = String(tree.prefix ?? "").replace(/\/+$/, "");
  return { modeChannel: named === "" ? null : named, prefix: prefix === "" ? null : prefix };
}

/**
 * Every row group of a set of zooms, one row per column, with what its footer says: the SQL a
 * tile manifest is written from and checked against.
 *
 * `zooms` is `[{ z, path, tile, part?, key?, shift? }]`. `tile` is the tile a file-per-tile file
 * holds, or `null` where the file is a row-group container. There the tile is the row group's
 * ordinal on a vertex zoom, and on an edge set — an adjacency orientation or a quotient, named by
 * `part` — it is the row group's lower bound on `key` shifted by `shift`, because a tile of sources
 * with no edges writes no row group and the ordinals stop being tiles. `tile_hi` is the same shift
 * of the upper bound, so a row group that straddles two tiles is visible rather than filed under
 * the first. A bound is kept only for a column of an integer or floating-point type —
 * `INT32`/`INT64` with no converted type or an integer one, `FLOAT`, `DOUBLE` — which is the set
 * `crates/fossil-sinks/src/tiles.rs` publishes; a string, a date or a boolean is counted for nulls
 * and not bounded.
 */
export function footerSql(zooms) {
  const text = (value) => (value === undefined || value === null ? "NULL" : `'${lit(String(value))}'`);
  const values = zooms.map(
    (z) =>
      `(${z.z}, ${text(z.part ?? "")}, '${lit(z.path)}', ${z.tile === null ? "NULL" : z.tile}, ` +
      `${text(z.key)}, ${z.shift ?? "NULL"})`,
  );
  const list = `[${zooms.map((z) => `'${lit(z.path)}'`).join(", ")}]`;
  const bounded =
    "(m.type IN ('FLOAT', 'DOUBLE') OR (m.type IN ('INT32', 'INT64') AND " +
    "(s.converted_type IS NULL OR regexp_matches(s.converted_type, '^U?INT_(8|16|32|64)$'))))";
  return `SELECT v.z::INTEGER AS z, v.part,
                 coalesce(v.t, k.stats_min_value::UBIGINT >> v.sh, m.row_group_id)::BIGINT AS tile,
                 coalesce(v.t, k.stats_max_value::UBIGINT >> v.sh, m.row_group_id)::BIGINT AS tile_hi,
                 m.row_group_id, m.row_group_num_rows AS rows, m.path_in_schema AS col, m.type,
                 m.stats_null_count AS nulls,
                 CASE WHEN ${bounded} THEN m.stats_min_value END AS lo,
                 CASE WHEN ${bounded} THEN m.stats_max_value END AS hi
            FROM (VALUES ${values.join(", ")}) v(z, part, f, t, key, sh)
            JOIN parquet_metadata(${list}) m ON m.file_name = v.f
            JOIN parquet_schema(${list}) s ON s.file_name = m.file_name AND s.name = m.path_in_schema
            LEFT JOIN parquet_metadata(${list}) k
              ON k.file_name = m.file_name AND k.row_group_id = m.row_group_id AND k.path_in_schema = v.key`;
}

/**
 * {@link footerSql} per TILE rather than per row group: what a tile manifest entry says.
 *
 * A tile is one row group wherever fossil's writer cut it, and a run of them where a writer that
 * sizes row groups by rows did — a file-per-tile adjacency holding more edges than one group, or
 * DuckDB's own `ROW_GROUP_SIZE`. The entry is then the run's statistics folded the way Iceberg folds
 * a file's: rows and nulls summed, the lower bound the least and the upper the greatest, compared
 * in the column's own domain. `straddles` is whether any row group of the tile reaches into the
 * next one — a tile no entry can describe, because its rows share a footer with another tile's.
 */
export function tileSql(zooms) {
  const typed = (fn, column) =>
    `CASE WHEN any_value(type) IN ('FLOAT', 'DOUBLE') THEN ${fn}(${column}::DOUBLE)::VARCHAR ` +
    `ELSE ${fn}(${column}::HUGEINT)::VARCHAR END`;
  return `SELECT z, part, tile, col, any_value(type) AS type, sum(rows)::BIGINT AS rows,
                 sum(nulls)::BIGINT AS nulls, ${typed("min", "lo")} AS lo, ${typed("max", "hi")} AS hi,
                 bool_or(tile_hi <> tile) AS straddles
            FROM (${footerSql(zooms)}) GROUP BY z, part, tile, col`;
}

/**
 * The name one edge set goes by in a tile manifest check: an adjacency orientation as its relation
 * and the endpoint it is cut on, a quotient as the word. Both sides of the check spell it here.
 */
export const partOf = (entry) =>
  `${entry.edge_type}:${entry.src_type}:${entry.dst_type}:${entry.aligned_by}`;

/**
 * The projections a manifest declares, resolved against the type's own prefix.
 *
 * **One function for a vertex type and a relation**, because there is one list. A corpus is an
 * order, a cut and some projections of that sequence: the payload is the entry at `scale: 1`, and
 * an adjacency is a `scale: 1` entry with an `aligned_by`. `GraphAr`'s two vocabularies for them —
 * `property_groups` and `adj_lists` — were one rule written twice.
 *
 * **The scale is read and never derived.** A reader shifts by its trailing zeros, and no checker
 * here spells an exponent.
 *
 * An entry that names no `path` is not a projection — it declares tiles nobody can address, which
 * `declared-tiling` reports. An entry whose files are not on disk comes back with an empty `files`.
 * Neither throws: an inspector that did would decide which violation a reader hears about first.
 */
function projectionsOf(info, root, prefix) {
  const declared = Array.isArray(info.projections) ? info.projections : [];
  return declared.map((entry) => {
    const path = String(entry.path ?? "").replace(/\/+$/, "");
    const raw = String(entry.scale ?? "").trim();
    const at = join(root, prefix, path);
    return {
      path,
      /** `null` when the entry declares no scale a shift addresses — a projection with no address. */
      scale: /^\d+$/.test(raw) && shiftFor(BigInt(raw)) !== null ? BigInt(raw) : null,
      alignedBy: entry.aligned_by === undefined ? null : String(entry.aligned_by),
      prefix: path === "" ? prefix : `${prefix}/${path}`,
      files: existsSync(at) ? payload(at) : [],
    };
  });
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
    const columns = columnsOf(files);
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
      columns: columns.names,
      /** The type under each of those names, which is what a shift depends on and a name cannot say. */
      columnTypes: columns.types,
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
          // The index carries `dense_id` too — it is the half of the pair that gets shifted, so it
          // is addressing and answers to the same rule as the payload's.
          columnTypes: columnsOf(tiles).types,
        };
      })(),
      /**
       * What this type says a reader can draw it with, or `null` for a type that says nothing —
       * see {@link channelsOf}. The declaration `declared-channels` holds against the payload.
       */
      channels: channelsOf(info),
      /**
       * The cell tree this type declares, or `null` for a type with none — see {@link cellsOf}.
       * Its `modeChannel` is a reference into `channels` above, and `mode-names-a-channel` is what
       * holds the two together: nothing in either block can see the other.
       */
      cells: cellsOf(info),
      /**
       * The tile manifest this type names, as a path under the corpus root, or `null` for a type
       * that names none — a reader then reads the footers, which is every corpus written before
       * the field. `tile-manifest` is what holds the file against those footers.
       */
      tileManifest:
        info.tile_manifest === undefined || String(info.tile_manifest).trim() === ""
          ? null
          : join(root, prefix, String(info.tile_manifest).trim()),
      /**
       * Every projection this type declares, the payload included — see {@link projectionsOf}.
       *
       */
      projections: projectionsOf(info, root, prefix),
    };
  });

  const edges = manifest.edges.map((info) => {
    const prefix = edgePrefix(info);
    const projections = projectionsOf(info, root, prefix);
    // Where an orientation's tiles are comes from the projection that declares
    // it, never from the convention that names them. `aligned_by` says which
    // endpoint column addresses the tiles and `path` says where they are, and
    // between them a reader turns a `dense_id` into a URL with nothing agreed out
    // of band. An orientation the manifest declares without a path has tiles
    // nobody can address, which `declared-tiling` reports; here it is simply an
    // orientation with none.
    const orientation = (alignedBy, name, column) => {
      const declared =
        projections.find((p) => p.scale === 1n && p.alignedBy === alignedBy) ?? null;
      const tilePrefix = declared === null ? "" : declared.path;
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
      /**
       * Every projection this relation declares — see {@link projectionsOf}. The two adjacencies
       * are its `scale: 1` entries.
       */
      projections,
      bySource: orientation("src", "by_source", "src_dense"),
      byTarget: orientation("dst", "by_target", "dst_dense"),
    };
  });

  for (const edge of edges) {
    for (const side of [edge.bySource, edge.byTarget]) {
      const columns = columnsOf([...side.relation, ...side.tiles]);
      side.columns = columns.names;
      side.columnTypes = columns.types;
    }
  }

  return {
    root,
    manifest,
    /** What `graph.graph.yml` says the container is. Absent is the file-per-tile one. */
    container: manifest.index.container === undefined ? "files" : String(manifest.index.container),
    types,
    edges,
    rel,
  };
}
