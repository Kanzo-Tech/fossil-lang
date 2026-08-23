/**
 * The addressing, derived from the manifest and from nothing else.
 *
 * This is one of the two implementations the conformance corpus exists to hold. The other is
 * `resolveCorpus` in `@fossil-lang/graph`, published and typed; this one is plain Node with no npm
 * and no build, and it shares no line with it. The YAML scan comes from `../guards/manifest.mjs`,
 * because there is one of those and it has its own self-test; the *addressing* — which prefix, which
 * shift, which orientation applies to which window — is written here from the conventions.
 *
 * It has two harnesses, and they ask different questions of it:
 *
 *   - `verify.mjs` runs it against `expected.json`, a table neither implementation wrote. That
 *     catches **drift between two readers**.
 *   - `writer.mjs` runs it against a corpus `fossil run` has just written. That catches **a reader
 *     agreeing with a reader while both disagree with the writer**, which no table can, because a
 *     table is written by whoever read the conventions last.
 *
 * The split into a module is what makes the second harness worth anything. A `writer.mjs` carrying
 * its own copy of the arithmetic would be a third reader, and three readers that agree still say
 * nothing about the bytes on disk.
 */

import { load } from "../guards/manifest.mjs";
import { shiftFor, tileOf } from "../guards/arithmetic.mjs";

/** Join dataset-relative segments the way the manifest writes them: forward slashes, always. */
export function rel(...parts) {
  return parts
    .filter((p) => p !== undefined && p !== "")
    .map((p, i) => (i === 0 ? p.replace(/\/+$/, "") : p.replace(/^\/+|\/+$/g, "")))
    .filter((p) => p !== "")
    .join("/");
}

export const withSlash = (p) => `${p.replace(/\/+$/, "")}/`;

/** A manifest field the address depends on. Absent is an error, because a default composes a URL. */
function need(info, key) {
  const value = info[key];
  if (typeof value !== "string" || value === "") {
    throw new Error(`${info.rel} declares no ${key}, so it addresses nothing`);
  }
  return value;
}

function needRows(info, key) {
  const value = Number(need(info, key));
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`${info.rel} declares ${key}: ${info[key]}, which is not a row count`);
  }
  return value;
}

/**
 * Resolve a corpus root into the addresses a reader composes URLs from.
 *
 * Throws on a manifest that cannot address itself: a `chunk_size` no shift addresses, an endpoint
 * type the index does not declare, or an edge whose declared tile size disagrees with the vertex
 * type that addresses it. It does *not* throw for an orientation the corpus does not publish — that
 * is a legitimate corpus, and it comes back as an address that does not exist.
 */
export function resolve(root, base = "") {
  const manifest = load(root);
  const prefix = rel(base, typeof manifest.index.prefix === "string" ? manifest.index.prefix : "");

  const types = manifest.vertices.map((info) => {
    const chunkSize = needRows(info, "chunk_size");
    const shift = shiftFor(BigInt(chunkSize));
    if (shift === null) {
      throw new Error(`${info.rel} declares a tile of ${chunkSize} rows, which no shift addresses`);
    }
    return {
      type: need(info, "type"),
      prefix: withSlash(rel(prefix, need(info, "prefix"))),
      chunkSize,
      shift: Number(shift),
      tileOf: (denseId) => tileOf(denseId, shift),
      tileUrl: (k) => `${withSlash(rel(prefix, need(info, "prefix")))}chunk${BigInt(k)}.parquet`,
    };
  });

  const edges = manifest.edges.map((info) => {
    const endpoint = (name, role) => {
      const found = types.find((t) => t.type === name);
      if (!found) throw new Error(`${info.rel} names ${role} type ${name}, which the index does not declare`);
      return found;
    };
    const src = endpoint(need(info, "src_type"), "source");
    const dst = endpoint(need(info, "dst_type"), "destination");
    for (const [key, vertex] of [["src_chunk_size", src], ["dst_chunk_size", dst]]) {
      const declared = needRows(info, key);
      if (declared !== vertex.chunkSize) {
        throw new Error(
          `${info.rel} declares ${key} ${declared} against ${vertex.type}'s chunk_size ${vertex.chunkSize}, ` +
            `so its tiles address nothing`,
        );
      }
    }
    const edgePrefix = withSlash(rel(prefix, need(info, "prefix")));
    const declared = new Map();
    for (const entry of Array.isArray(info.adj_lists) ? info.adj_lists : []) {
      const direction = entry?.aligned_by;
      if (direction !== "src" && direction !== "dst") continue;
      const adjPrefix = String(entry.prefix ?? "").replace(/\/+$/, "");
      if (adjPrefix === "") continue;
      const vertex = direction === "src" ? src : dst;
      const tilePrefix = withSlash(rel(edgePrefix, adjPrefix));
      declared.set(direction, {
        direction,
        prefix: tilePrefix,
        column: direction === "src" ? "src_dense" : "dst_dense",
        chunkSize: vertex.chunkSize,
        shift: vertex.shift,
        tileUrl: (k) => `${tilePrefix}tile${BigInt(k)}.parquet`,
      });
    }
    return {
      edgeType: need(info, "edge_type"),
      srcType: src.type,
      dstType: dst.type,
      prefix: edgePrefix,
      directions: ["src", "dst"].filter((d) => declared.has(d)),
      adjacency: (d) => declared.get(d) ?? null,
    };
  });

  const vertexType = (name) => {
    const found = name === undefined ? types[0] : types.find((t) => t.type === name);
    if (!found) throw new Error(`no vertex type ${name}; the index names ${types.map((t) => t.type).join(", ")}`);
    return found;
  };

  /**
   * The URLs a set of vertex tiles addresses, and what that set is complete for.
   *
   * Only the orientations whose own `dense_id` space is this window's: `by_target` tile `k` of a
   * cross-type edge addresses tile `k` of the *destination* type, which is a different set of
   * vertices. A window over the source type that read it would answer a question nobody asked.
   */
  const window = ({ type, tiles, directions = ["src"] }) => {
    const vertex = vertexType(type);
    const wanted = new Set(directions);
    const urls = [];
    const gaps = [];
    for (const edge of edges) {
      const applicable = [];
      if (edge.srcType === vertex.type) applicable.push("src");
      if (edge.dstType === vertex.type) applicable.push("dst");
      for (const direction of applicable) {
        const adjacency = edge.adjacency(direction);
        if (adjacency === null) {
          gaps.push({ edge_type: edge.edgeType, direction, reason: "not-declared" });
        } else if (!wanted.has(direction)) {
          gaps.push({ edge_type: edge.edgeType, direction, reason: "not-requested" });
        } else {
          urls.push(...tiles.map((k) => adjacency.tileUrl(k)));
        }
      }
    }
    return {
      vertex_urls: tiles.map((k) => vertex.tileUrl(k)),
      edge_urls: urls,
      complete: gaps.length === 0,
      gaps,
    };
  };

  return { types, edges, vertexType, window };
}
