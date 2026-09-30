/**
 * What is on disk, before any guard has an opinion about it.
 *
 * The guards ask questions; this file answers *what there is to ask about*: the manifest as parsed,
 * one entry per table it declares with the columns and rows its Parquet file holds, and every
 * Parquet file under the root whether the manifest names it or not. It reconciles nothing — a path
 * that does not resolve is recorded as such, and deciding that is a violation is a guard's job.
 */

import { existsSync, readdirSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { lit, query, scalar } from "./duck.mjs";
import { load } from "./manifest.mjs";

/** `read_parquet` over one file, as SQL. */
export function parquet(file) {
  return `read_parquet('${lit(file)}')`;
}

/** Every `.parquet` under `root`, corpus-relative with forward slashes, sorted. */
function parquetFiles(root) {
  if (!existsSync(root)) return [];
  return readdirSync(root, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".parquet"))
    .map((entry) => relative(root, join(entry.parentPath, entry.name)).split(sep).join("/"))
    .sort();
}

/**
 * One declared table and what its file holds. `columns` maps a column name to DuckDB's spelling of
 * its type; `rows` is a `BigInt`, or `null` when the file did not open, and `error` says why.
 */
function table(root, kind, entry) {
  const path = typeof entry?.path === "string" ? entry.path : null;
  const file = path === null ? null : join(root, path);
  const out = {
    kind,
    entry: entry ?? {},
    name: typeof entry?.name === "string" ? entry.name : "(unnamed)",
    path,
    file,
    exists: file !== null && existsSync(file),
    columns: new Map(),
    rows: null,
    error: null,
  };
  if (!out.exists) return out;
  try {
    for (const row of query(`DESCRIBE SELECT * FROM ${parquet(file)}`)) {
      out.columns.set(row.column_name, row.column_type);
    }
    out.rows = BigInt(scalar(`SELECT count(*) FROM ${parquet(file)}`));
  } catch (error) {
    out.error = error.message;
  }
  return out;
}

/**
 * The corpus at `root`.
 *
 * @param {string} root
 */
export function inspect(root) {
  const manifest = load(root);
  const json = manifest.json ?? {};
  const list = (key) => (Array.isArray(json[key]) ? json[key] : []);
  const vertices = list("vertex_tables").map((entry) => table(root, "vertex", entry));
  const edges = list("edge_tables").map((entry) => table(root, "edge", entry));
  return {
    root,
    manifest,
    vertices,
    edges,
    /** Every Parquet file under the root, named or not. */
    onDisk: parquetFiles(root),
    /** A vertex table by name, or `undefined`. */
    vertex: (name) => vertices.find((t) => t.name === name),
  };
}
