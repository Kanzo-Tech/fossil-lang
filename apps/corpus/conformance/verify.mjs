/**
 * The conformance corpus, executed.
 *
 * `expected.json` is a table of addresses: what a reader must compose from a manifest, and what it
 * must refuse to compose. This file is one of the two implementations that execute it. The other is
 * `packages/graph/tests/conformance.test.ts`, which runs `resolveCorpus` — the published module —
 * against the same table. Neither wrote it, and a change to either that moves an address moves it
 * away from the other.
 *
 * **What is shared and what is not.** The YAML scan comes from `../guards/manifest.mjs`, because
 * there is one of those and it has its own self-test; a fourth copy would be a fourth thing to keep
 * right. The *addressing* — which prefix, which shift, which orientation applies to which window —
 * is written here from the conventions and from nothing else. That is the half this corpus exists
 * to hold two implementations of.
 *
 *   node conformance/verify.mjs
 *
 * Exit `0` when every address in the table reproduces, `1` when one does not.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join as pathJoin } from "node:path";
import { fileURLToPath } from "node:url";
import { load } from "../guards/manifest.mjs";
import { shiftFor, tileOf } from "../guards/arithmetic.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

/** Join dataset-relative segments the way the manifest writes them: forward slashes, always. */
function rel(...parts) {
  return parts
    .filter((p) => p !== undefined && p !== "")
    .map((p, i) => (i === 0 ? p.replace(/\/+$/, "") : p.replace(/^\/+|\/+$/g, "")))
    .filter((p) => p !== "")
    .join("/");
}

const withSlash = (p) => `${p.replace(/\/+$/, "")}/`;

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
 * The addressing, derived from the manifest and nothing else.
 *
 * Throws on a manifest that cannot address itself: a `chunk_size` no shift addresses, an endpoint
 * type the index does not declare, or an edge whose declared tile size disagrees with the vertex
 * type that addresses it. It does *not* throw for an orientation the corpus does not publish — that
 * is a legitimate corpus, and it comes back as an address that does not exist.
 */
function resolve(root, base = "") {
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

// ---------------------------------------------------------------------------

const failures = [];
const notes = [];
const fail = (message) => failures.push(message);

function same(what, got, want) {
  const a = JSON.stringify(got);
  const b = JSON.stringify(want);
  if (a !== b) fail(`${what}: ${a} is not ${b}`);
}

const table = JSON.parse(readFileSync(pathJoin(HERE, "expected.json"), "utf8"));

for (const expected of table.cases) {
  const root = pathJoin(HERE, expected.root);
  const label = expected.name;

  if (expected.resolve_throws) {
    let threw = null;
    try {
      resolve(root);
    } catch (error) {
      threw = error.message;
    }
    if (threw === null) fail(`${label}: resolved a manifest that addresses nothing`);
    else if (!threw.includes(expected.resolve_throws)) {
      fail(`${label}: threw "${threw}", which does not say "${expected.resolve_throws}"`);
    } else notes.push(`${label}: refused — ${threw}`);
    continue;
  }

  const corpus = resolve(root);

  same(
    `${label}: vertex types`,
    corpus.types.map((t) => ({
      type: t.type,
      prefix: t.prefix,
      chunk_size: t.chunkSize,
      shift: t.shift,
    })),
    expected.types.map((t) => ({
      type: t.type,
      prefix: t.prefix,
      chunk_size: t.chunk_size,
      shift: t.shift,
    })),
  );

  same(
    `${label}: edge types`,
    corpus.edges.map((e) => ({
      edge_type: e.edgeType,
      src_type: e.srcType,
      dst_type: e.dstType,
      prefix: e.prefix,
      directions: e.directions,
      adjacencies: e.directions.map((d) => {
        const a = e.adjacency(d);
        return { direction: d, prefix: a.prefix, column: a.column, chunk_size: a.chunkSize, shift: a.shift };
      }),
    })),
    expected.edges.map((e) => ({
      edge_type: e.edge_type,
      src_type: e.src_type,
      dst_type: e.dst_type,
      prefix: e.prefix,
      directions: e.directions,
      adjacencies: e.adjacencies,
    })),
  );

  for (const v of expected.tile_of ?? []) {
    const got = corpus.vertexType(v.type).tileOf(BigInt(v.dense_id));
    if (got !== BigInt(v.tile)) fail(`${label}: tile_of(${v.dense_id}) in ${v.type} = ${got}, not ${v.tile}`);
  }

  for (const address of expected.addresses ?? []) {
    const got =
      address.kind === "vertex"
        ? corpus.vertexType(address.type).tileUrl(address.tile)
        : corpus.edges
            .find((e) => e.edgeType === address.edge_type)
            ?.adjacency(address.direction)
            ?.tileUrl(address.tile);
    if (got !== address.path) fail(`${label}: composed ${got}, not ${address.path}`);
    // The whole point, on the one case that has bytes: a composed URL names a file that is there.
    if (expected.on_disk && got !== undefined && !existsSync(pathJoin(root, got))) {
      fail(`${label}: ${got} composes and is not on disk`);
    }
  }

  for (const refused of expected.refused ?? []) {
    const got = corpus.edges.find((e) => e.edgeType === refused.edge_type)?.adjacency(refused.direction);
    if (got !== null) {
      fail(`${label}: ${refused.edge_type} handed back an address for ${refused.direction}, which it does not publish`);
    }
  }

  for (const [index, expectation] of (expected.windows ?? []).entries()) {
    const got = corpus.window({
      type: expectation.type,
      tiles: expectation.tiles,
      directions: expectation.directions,
    });
    same(`${label}: window ${index}`, got, {
      vertex_urls: expectation.vertex_urls,
      edge_urls: expectation.edge_urls,
      complete: expectation.complete,
      gaps: expectation.gaps,
    });
  }

  for (const expectation of expected.throws ?? []) {
    let threw = null;
    try {
      corpus.vertexType(expectation.vertex_type);
    } catch (error) {
      threw = error.message;
    }
    if (threw === null || !threw.includes(expectation.message)) {
      fail(`${label}: naming vertex type ${expectation.vertex_type} did not say "${expectation.message}"`);
    }
  }

  notes.push(
    `${label}: ${corpus.types.length} type(s), ${corpus.edges.length} edge type(s), ` +
      `${(expected.addresses ?? []).length} address(es)${expected.on_disk ? " checked on disk" : ""}`,
  );
}

// The base is prepended and nothing else happens to it.
{
  const { base, path, url, case: name } = table.base_join;
  const target = table.cases.find((c) => c.name === name);
  const corpus = resolve(pathJoin(HERE, target.root), base);
  const got = corpus.types
    .flatMap((t) => [t.tileUrl(3)])
    .find((u) => u.endsWith(path.slice(path.lastIndexOf("/") + 1)));
  if (got !== url) fail(`base_join: composed ${got}, not ${url}`);
}

for (const note of notes) console.log(`  ${note}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} address(es) do not reproduce:\n`);
  for (const failure of failures) console.error(`  ✗ ${failure}`);
  process.exit(1);
}
console.log(`\n${table.cases.length}/${table.cases.length} conformance cases reproduce`);
