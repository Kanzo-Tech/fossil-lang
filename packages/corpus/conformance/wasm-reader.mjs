/**
 * The third reader, adapted to the shape the other two are executed in.
 *
 * `reader.mjs` is plain Node written from the conventions. The third leg is the published
 * `@fossil-lang/corpus`, whose `open` is a binding over exactly the module this file loads —
 * so the table is executed by three legs over **two** implementations, and this one is the entry
 * the other two share a language boundary with rather than a reader of its own.
 *
 * It was two legs and both were JavaScript, and a mistake they shared — a shift taken as signed, a
 * count that went through a `Number` — was invisible to a diff of the two. This one is
 * `fossil_graph::plan`, compiled to wasm32 and reached through `fossil-graph-wasm`'s `Corpus`: a
 * different language from `reader.mjs`, a different integer width from the native leg in
 * `crates/fossil-graph/tests/conformance.rs`, and the build that actually ships to a browser.
 *
 * # This is the one thing here that is not `node` plus a `duckdb` binary
 *
 * Everything else under `conformance/` and `guards/` runs in a checkout where nothing has been installed, which
 * is the position the third party this format is for is in — `guards/README.md` says take this
 * directory, it is meant to be copied, and `guards/` keeps that claim intact. This file does not:
 * it imports `packages/corpus/pkg/`, which `pnpm --filter @fossil-lang/corpus build:wasm` writes and
 * which needs a Rust toolchain and `wasm-bindgen-cli`.
 *
 * So the leg is **required by default and refused explicitly**, never skipped quietly. `verify.mjs`
 * without `--without-wasm` fails when the build output is absent; `corpus.yml`, which installs
 * `node` and `duckdb` and nothing else, passes the flag and says why. `pnpm-ci.yml` has already
 * built the wasm by the time it runs the same file with all three legs. A leg that skips itself
 * when its dependency is missing is how a conformance suite comes to pass over nothing, which is
 * the failure `expected.json` exists to prevent, so the default is the strict one.
 *
 * # What it does not do, and it is not an omission
 *
 * It opens no byte. `answers.mjs` is the half that reads Parquet and it stays on `reader.mjs`,
 * because what `expected.json`'s `cases` block asks is which URL a tile has — a question with no
 * engine in it. That is also what makes this leg indifferent to which engine a host puts behind a
 * query: there is none here to swap.
 */

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

/** Where `packages/corpus/scripts/build-wasm.sh` writes its output. */
export const PKG_DIR = join(HERE, "..", "pkg");

const SHIM = join(PKG_DIR, "fossil_graph_wasm.js");
const BINARY = join(PKG_DIR, "fossil_graph_wasm_bg.wasm");

/** How to get the leg back, printed wherever it is missing. */
export const BUILD_COMMAND = "pnpm --filter @fossil-lang/corpus run build:wasm";

/**
 * Every manifest under a case root, keyed the way a host that fetched them would key them.
 *
 * The wasm reader takes the bytes rather than a path, because the browser it is built for has no
 * filesystem — which is the same reason `ManifestSource` exists on the Rust side. Collecting them
 * here is the host's job in every binding.
 */
export function manifestFiles(root) {
  const files = {};
  for (const entry of readdirSync(root, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith(".yml")) continue;
    const path = join(entry.parentPath, entry.name);
    files[relative(root, path).split("\\").join("/")] = readFileSync(path, "utf8");
  }
  return files;
}

/**
 * Load the wasm module, or return why it could not be loaded.
 *
 * `--target web` output, initialised from bytes read off disk rather than fetched: the shim's
 * default export takes `{ module_or_path }`, and handing it a `Buffer` is what a Node host does in
 * place of a network round trip.
 */
export async function load() {
  const { default: init, Corpus } = await import(SHIM);
  await init({ module_or_path: readFileSync(BINARY) });
  return Corpus;
}

/** Whether the build output is there at all, checked before the import so the error can say what. */
export function built() {
  try {
    readFileSync(BINARY);
    return true;
  } catch {
    return false;
  }
}

/**
 * A tile SIZE, or a scale, as the `Number` the other two readers hold it in — refused rather than
 * rounded when it does not fit.
 *
 * `snapshot()` serialises with `serialize_large_number_types_as_bigints`, so every `u64` in the
 * plan arrives as a `BigInt`, whatever it measures. That is right for a count and a tile number,
 * which are `dense_id` width, and it is the shape of the field rather than a claim about it for a
 * tile size or a scale: the contract — `expected.json`, `reader.mjs` and the published
 * `ProjectionAddress` alike — holds those as `Number`, and the table compares them as one. So the
 * conversion is made here, once per field, and made **exact**: a size past 2^53 is not a corpus any
 * reader of this table can agree about, and a `Number()` that rounded it would be this file
 * agreeing with the table about a value the module never returned.
 */
function exact(field, value) {
  const n = Number(value);
  if (!Number.isSafeInteger(n) || BigInt(n) !== BigInt(value)) {
    throw new RangeError(`${field} ${value} does not fit a Number, and the contract holds it as one`);
  }
  return n;
}

/**
 * `resolve(root, base)` over the wasm reader, in the shape `reader.mjs` returns.
 *
 * The adaptation is naming and width, and nothing else. A `dense_id` and a tile number cross as a
 * `BigInt` in both directions, because they carry more bits than a `Number` holds; a tile size, a
 * shift and a scale come back as a `Number` through {@link exact}; a row count and a tile count
 * stay the `BigInt` the snapshot carries, and `verify.mjs` compares those in decimal, as the table
 * writes them. An adjacency is the projection at `scale: 1` in its orientation, which is what the
 * binding reports it as. Anything computed here rather than asked of the module would be this file
 * agreeing with itself.
 */
export function reader(Corpus) {
  return (root, base = "") => {
    const corpus = new Corpus(manifestFiles(root), base);
    const snapshot = corpus.snapshot();

    const projection = (p, tileOf, tileUrl, files) => {
      const scale = BigInt(p.scale);
      return {
        prefix: p.prefix,
        scale: exact("scale", p.scale),
        direction: p.direction ?? null,
        column: p.column,
        chunkSize: exact("chunk_size", p.chunk_size),
        shift: p.shift,
        rows: p.rows,
        tiles: p.tiles,
        container: p.container,
        tileOf: (denseId) => tileOf(scale, BigInt(denseId)),
        tileUrl: (k) => tileUrl(scale, BigInt(k)),
        files: () => files(scale),
      };
    };

    const types = snapshot.types.map((t) => {
      const projections = t.projections.map((p) =>
        projection(
          p,
          (scale, id) => corpus.projectionTileOf(t.type, scale, id),
          (scale, k) => corpus.projectionTileUrl(t.type, scale, k),
          (scale) => corpus.projectionFiles(t.type, scale),
        ),
      );
      return {
        type: t.type,
        prefix: t.prefix,
        container: t.container,
        chunkSize: exact("chunk_size", t.chunk_size),
        shift: t.shift,
        tileOf: (denseId) => corpus.tilesOf(t.type, BigUint64Array.of(BigInt(denseId)))[0],
        tileUrl: (k) => corpus.vertexTileUrl(t.type, BigInt(k)),
        index:
          t.index === null || t.index === undefined
            ? null
            : {
                prefix: t.index.prefix,
                container: t.index.container,
                orderedBy: t.index.ordered_by,
                chunkSize: exact("index chunk_size", t.index.chunk_size),
                tiles: t.index.tiles,
              },
        projections,
        projection: (scale) => projections.find((p) => p.scale === scale) ?? null,
        projectionFiles: (scale) => corpus.projectionFiles(t.type, BigInt(scale)),
      };
    });

    const edges = snapshot.edges.map((e) => {
      const projections = e.projections.map((p) =>
        projection(
          p,
          (scale, id) => corpus.edgeProjectionTileOf(e.edge_type, p.direction, scale, id),
          (scale, k) => corpus.edgeProjectionTileUrl(e.edge_type, p.direction, scale, k),
          (scale) => corpus.edgeProjectionFiles(e.edge_type, p.direction, scale),
        ),
      );
      const at = (scale, direction) =>
        projections.find((p) => p.scale === scale && p.direction === direction) ?? null;
      return {
        edgeType: e.edge_type,
        srcType: e.src_type,
        dstType: e.dst_type,
        prefix: e.prefix,
        directions: e.directions,
        projections,
        projection: at,
        adjacency: (direction) => at(1, direction),
        projectionFiles: (scale, direction) =>
          corpus.edgeProjectionFiles(e.edge_type, direction, BigInt(scale)),
      };
    });

    // The module's own refusal, and it carries the message `expected.json` pins. Asked for rather
    // than reproduced here: a message this file wrote would be this file agreeing with the table
    // about a sentence the reader never says.
    const vertexType = (name) => {
      const resolved = corpus.vertexTypeName(name);
      return types.find((t) => t.type === resolved);
    };

    return {
      container: snapshot.container,
      types,
      edges,
      vertexType,
      window: ({ type, tiles, directions = ["src"] }) => {
        const got = corpus.window(type, BigUint64Array.from(tiles.map(BigInt)), directions);
        return {
          vertex_urls: got.vertex_urls,
          edge_urls: got.edge_urls,
          complete: got.complete,
          gaps: got.gaps,
        };
      },
    };
  };
}
