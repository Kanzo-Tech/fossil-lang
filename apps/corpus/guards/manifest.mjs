/**
 * The manifest, read the way a stranger reads it.
 *
 * A corpus cannot be discovered by listing a directory — over HTTP there is no listing — so it has
 * one entry point, `graph.graph.yml`, and everything else is reached from there. This module scans
 * that file and the per-type files it names.
 *
 * **It is a line scanner and not a YAML parser, deliberately.** Deserialising the manifest through
 * the struct that wrote it proves the struct round-trips and nothing about the artefact; pulling in
 * a YAML library would make this checker installable rather than copyable. What the manifest
 * actually uses is a flat mapping of scalars, one sequence of paths, and one sequence of small
 * mappings, and that is the grammar below.
 *
 * **What it refuses rather than guesses:** anchors and aliases, flow style (`[a, b]`), multi-line
 * scalars (`|`, `>`), and any nesting deeper than one sequence of mappings. A manifest using them
 * fails to scan with a message naming the line, which is the honest outcome — a scanner that
 * silently reads half a document is worse than one that stops.
 */

import { existsSync, readFileSync } from "node:fs";
import { join, posix } from "node:path";

/** Dataset-relative location of the aggregate index. The one path a reader is told. */
export const GRAPH_INFO_PATH = "graph.graph.yml";

/** Strip the quoting serde emits around a string that needs it. */
function unquote(value) {
  const v = value.trim();
  if (v.length >= 2 && ((v[0] === "'" && v.at(-1) === "'") || (v[0] === '"' && v.at(-1) === '"'))) {
    return v.slice(1, -1).replace(/''/g, "'");
  }
  return v;
}

const REFUSED = [
  [/^\s*[-\w]+:\s*[|>][-+\d]*\s*$/, "a multi-line scalar"],
  // `[]` and `{}` are how an empty collection is written and carry nothing to misread; a *populated*
  // flow collection is a second grammar for a sequence and this scanner has one.
  [/^\s*[-\w]+:\s*[[{]\s*[^\s\]}]/, "flow style"],
  [/^\s*[-\w]*:?\s*[&*]\w/, "an anchor or alias"],
];

/**
 * Scan one manifest file into `{ key: scalar | string[] | Array<Record<string, string>> }`.
 *
 * @param {string} path
 * @returns {Record<string, unknown>}
 */
export function scan(path) {
  const text = readFileSync(path, "utf8");
  const out = {};
  let sequence = null;
  let item = null;

  const lines = text.split("\n");
  for (const [index, raw] of lines.entries()) {
    const line = raw.replace(/\s+$/, "");
    if (line === "" || line.trimStart().startsWith("#")) continue;
    for (const [pattern, what] of REFUSED) {
      if (pattern.test(line)) {
        throw new Error(`${path}:${index + 1} uses ${what}, which this scanner refuses to guess at`);
      }
    }

    // A continuation of the mapping currently being built inside a sequence.
    const continuation = /^ {2}([\w]+):\s*(.*)$/.exec(line);
    if (continuation && item) {
      item[continuation[1]] = unquote(continuation[2]);
      continue;
    }
    // Anything else indented belongs to a nested collection — a property list inside a property
    // group — and no guard reads one. Skipped rather than refused: it is legal, it is just not
    // addressed by any convention here, and refusing it would make the checker fail on a corpus it
    // can read perfectly well.
    if (/^ {2,}/.test(line)) continue;

    const element = /^- (.*)$/.exec(line);
    if (element && sequence) {
      const pair = /^([\w]+):\s*(.*)$/.exec(element[1]);
      if (pair) {
        item = { [pair[1]]: unquote(pair[2]) };
        sequence.push(item);
      } else {
        item = null;
        sequence.push(unquote(element[1]));
      }
      continue;
    }

    const entry = /^([\w]+):\s*(.*)$/.exec(line);
    if (!entry) throw new Error(`${path}:${index + 1} is not a key, an item or a continuation`);
    item = null;
    if (entry[2] === "") {
      sequence = [];
      out[entry[1]] = sequence;
    } else {
      sequence = null;
      out[entry[1]] = unquote(entry[2]);
    }
  }

  return out;
}

/**
 * Load a corpus's manifest set from its root directory: the index, every vertex type it names and
 * every edge type it names, each carrying the dataset-relative path it was read from.
 *
 * @param {string} root
 */
export function load(root) {
  const index = scan(join(root, GRAPH_INFO_PATH));
  const relPaths = (key) => (Array.isArray(index[key]) ? index[key] : []).map(String);

  // A path the index names and cannot be read is a finding, not a crash. Throwing here would decide
  // which broken convention a reader hears about first, and it would hide every other one behind it.
  const resolve = (rel) => {
    const path = join(root, rel);
    if (!existsSync(path)) return { rel, missing: "is not on disk" };
    try {
      return { rel, ...scan(path) };
    } catch (error) {
      return { rel, missing: error.message };
    }
  };

  return {
    root,
    index,
    vertices: relPaths("vertices").map(resolve).filter((v) => !v.missing),
    edges: relPaths("edges").map(resolve).filter((e) => !e.missing),
    unreadable: [...relPaths("vertices"), ...relPaths("edges")].map(resolve).filter((m) => m.missing),
    declared: [GRAPH_INFO_PATH, ...relPaths("vertices"), ...relPaths("edges")],
  };
}

/**
 * The directory a vertex type's tiles live under, dataset-relative and without a trailing slash.
 * `prefix` is the manifest's own word for it, and the manifest is what a reader has.
 */
export function vertexPrefix(vertex) {
  return String(vertex.prefix ?? "").replace(/\/+$/, "");
}

/** The directory an edge type's two orientations live under, dataset-relative. */
export function edgePrefix(edge) {
  return String(edge.prefix ?? "").replace(/\/+$/, "");
}

/** Join dataset-relative segments the way the manifest writes them: forward slashes, always. */
export function rel(...parts) {
  return posix.join(...parts.filter((p) => p !== ""));
}
