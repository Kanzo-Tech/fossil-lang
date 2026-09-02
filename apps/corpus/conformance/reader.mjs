/**
 * The addressing, derived from the manifest and from nothing else.
 *
 * This is one of the two implementations the conformance corpus exists to hold. The other is
 * `resolveCorpus` in `@fossil-lang/corpus`, published and typed; this one is plain Node with no npm
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

import { GRAPH_INFO_PATH, load } from "../guards/manifest.mjs";
import { shiftFor, tileOf, tileUrl } from "../guards/arithmetic.mjs";

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
 * Which container the corpus carries its tiles in — `graph.graph.yml`'s own `container`.
 *
 * Absent is `files`, and that is the one default in this file rather than a `need()`: a corpus
 * written before the field existed is the file-per-tile container, so absence is a statement and
 * not a gap. Anything else is refused, because a third spelling would compose a URL.
 */
function containerOf(index) {
  const declared = index.container;
  if (declared === undefined || declared === "") return "files";
  if (declared !== "files" && declared !== "rowgroups") {
    throw new Error(
      `${GRAPH_INFO_PATH} declares container ${declared}; a tile is a file or a row group`,
    );
  }
  return declared;
}

/**
 * Where the tiles of one payload set are, bound to that set.
 *
 * One binding for the three sets that had three copies of the composition — the vertex payload, the
 * identity index and each adjacency orientation — because the only thing that ever differed between
 * them is the stem of the filename. `tileUrl` itself is in `guards/arithmetic.mjs`, with the rest
 * of the arithmetic the published vectors execute.
 */
const tileUrlFor = (prefix, stem, container) => (k) => tileUrl(prefix, stem, container, k);

/** Distinct, in order. In the row-group container every tile of a set names the same file. */
const distinct = (urls) => [...new Set(urls)];

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
  const container = containerOf(manifest.index);

  const types = manifest.vertices.map((info) => {
    const chunkSize = needRows(info, "chunk_size");
    const shift = shiftFor(BigInt(chunkSize));
    if (shift === null) {
      throw new Error(`${info.rel} declares a tile of ${chunkSize} rows, which no shift addresses`);
    }
    const typePrefix = withSlash(rel(prefix, need(info, "prefix")));
    return {
      type: need(info, "type"),
      prefix: typePrefix,
      container,
      chunkSize,
      shift: Number(shift),
      tileOf: (denseId) => tileOf(denseId, shift),
      tileUrl: tileUrlFor(typePrefix, "chunk", container),
      // The identity index, or `null`. A half-declared one is refused rather than
      // ignored: ignoring it reads exactly like a corpus that declares none, and
      // guessing the sort of files whose `ordered_by` is missing returns a
      // plausible stranger instead of nothing.
      /**
       * The WRITTEN levels, or `null`. A level is the predicate `dense_id % 2^k == 0` over the
       * payload whatever this says, so `null` is a corpus and not a gap — what a written level
       * changes is which bytes answer, never which rows.
       *
       * A block that names a prefix and no level list is refused on the index's argument: which
       * levels a writer spent bytes on is a POLICY, a reader cannot re-derive it from
       * `vertex_count` and `chunk_size` without reimplementing the writer's plan, and a
       * half-declared pyramid read as none is the difference between opening `l6/` and striding a
       * million rows.
       */
      levels: (() => {
        const declared = info.levels;
        if (declared === undefined || declared === null || Array.isArray(declared)) return null;
        const stem = declared.prefix;
        if (typeof stem !== "string" || stem === "") {
          throw new Error(`${info.rel} declares levels and no prefix, so the tiles they name cannot be composed`);
        }
        const listed = Array.isArray(declared.levels) ? declared.levels : [];
        if (listed.length === 0) {
          throw new Error(
            `${info.rel} declares levels and no level list, so which of them is written is not ` +
              `derivable — and it is a policy, not arithmetic a reader can redo`,
          );
        }
        const written = listed.map((raw) => {
          const level = Number(raw);
          if (!Number.isInteger(level) || level < 0) {
            throw new Error(`${info.rel} declares level ${raw}, which is not a level`);
          }
          return level;
        });
        const levelChunk = Number(declared.chunk_size);
        const levelShift = Number.isInteger(levelChunk) && levelChunk > 0 ? shiftFor(BigInt(levelChunk)) : null;
        if (levelShift === null) {
          throw new Error(
            `${info.rel} declares a level tile of ${declared.chunk_size} rows, which no shift addresses`,
          );
        }
        const count = needRows(info, "vertex_count");
        const levelPrefix = (level) => withSlash(rel(typePrefix, `${stem}${level}`));
        const rows = (level) => Math.ceil(count / 2 ** level);
        return {
          levels: written,
          chunkSize: levelChunk,
          shift: Number(levelShift),
          container,
          has: (level) => written.includes(level),
          prefix: levelPrefix,
          // `k` more bits of `dense_id` fall off than the payload's own address drops: level `k`
          // holds one row in `2^k`, so a tile of it spans that many times the ids.
          tileOf: (level, denseId) => tileOf(denseId, levelShift + BigInt(level)),
          tileUrl: (level, tile) => tileUrlFor(levelPrefix(level), "chunk", container)(tile),
          rows,
          tiles: (level) => Math.ceil(rows(level) / levelChunk),
          files: (level) => {
            if (!written.includes(level)) {
              throw new Error(
                `${info.rel} writes levels ${written.join(", ")} and not ${level}, so its files are ` +
                  `not addressable — the predicate over the payload is what answers that level`,
              );
            }
            const urls = [];
            for (let k = 0; k < Math.ceil(rows(level) / levelChunk); k += 1) {
              urls.push(tileUrlFor(levelPrefix(level), "chunk", container)(k));
            }
            return distinct(urls);
          },
        };
      })(),
      index: (() => {
        const declared = info.index;
        if (declared === undefined || declared === null || Array.isArray(declared)) return null;
        const indexPrefix = withSlash(rel(typePrefix, need(declared, "prefix")));
        const orderedBy = need(declared, "ordered_by");
        const indexChunk = needRows(declared, "chunk_size");
        return {
          prefix: indexPrefix,
          container,
          orderedBy,
          chunkSize: indexChunk,
          tiles: Math.ceil(needRows(info, "vertex_count") / indexChunk),
          tileUrl: tileUrlFor(indexPrefix, "tile", container),
        };
      })(),
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
        container,
        column: direction === "src" ? "src_dense" : "dst_dense",
        chunkSize: vertex.chunkSize,
        shift: vertex.shift,
        tileUrl: tileUrlFor(tilePrefix, "tile", container),
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
   *
   * The URLs are **distinct**, which is a no-op in the file-per-tile container and the whole answer
   * in the other: fifteen tiles of one type are one file, and a list that named it fifteen times
   * would be fifteen scans of it.
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
      vertex_urls: distinct(tiles.map((k) => vertex.tileUrl(k))),
      edge_urls: distinct(urls),
      complete: gaps.length === 0,
      gaps,
    };
  };

  /**
   * The same URLs a window addresses, each with the column it is READ BY.
   *
   * A sibling of `window` rather than a field of its answer, because `edge_urls` would then be
   * `edge_reads.map(u => u.url)` — one datum with two spellings in one object, which is the
   * duplication this tree deletes on sight. `window`'s shape is also what `expected.json` pins and
   * two implementations execute, and widening it to carry a derivable field would change that
   * table for no question it answers.
   *
   * The pairing is not derivable from a URL and it is not the same for the two orientations:
   * `by_source/tile{k}` holds the edges whose `src_dense` is in tile k, so filtering that file by
   * `dst_dense` answers a question nobody asked and drags in edges whose source is outside the
   * window. That was written here first, and it read **153** where a full scan says **152**.
   *
   * One read per distinct URL, not per tile: in the row-group container every tile of an
   * orientation is the same file, and a caller that ran one query per tile would read the whole
   * relation once per tile and count every edge that many times.
   */
  const edgeReads = ({ type, tiles, directions = ["src"] }) => {
    const wanted = new Set(directions);
    const vertex = vertexType(type);
    const reads = new Map();
    for (const edge of edges) {
      const applicable = [];
      if (edge.srcType === vertex.type) applicable.push("src");
      if (edge.dstType === vertex.type) applicable.push("dst");
      for (const direction of applicable) {
        const adjacency = edge.adjacency(direction);
        if (adjacency === null || !wanted.has(direction)) continue;
        for (const k of tiles) {
          const url = adjacency.tileUrl(k);
          const key = `${edge.edgeType} ${direction} ${url}`;
          if (!reads.has(key)) {
            reads.set(key, { url, column: adjacency.column, direction, edge_type: edge.edgeType });
          }
        }
      }
    }
    return [...reads.values()];
  };

  return { container, types, edges, vertexType, window, edgeReads };
}
