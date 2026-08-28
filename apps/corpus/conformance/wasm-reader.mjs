/**
 * The third reader, adapted to the shape the other two are executed in.
 *
 * `reader.mjs` is plain Node written from the conventions. `resolveCorpus` in
 * `@fossil-lang/corpus/address` is the published TypeScript. Both are JavaScript, and a mistake they
 * share — a shift taken as signed, a count that went through a `Number` — is invisible to a diff of
 * the two. This one is `fossil_graph::address`, compiled to wasm32 and reached through
 * `fossil-graph-wasm`'s `Corpus`: a different language, a different integer width, and the build
 * that actually ships to a browser.
 *
 * # This is the one thing here that is not `node` plus a `duckdb` binary
 *
 * Everything else under `apps/corpus/` runs in a checkout where nothing has been installed, which
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
export const PKG_DIR = join(HERE, "..", "..", "..", "packages", "corpus", "pkg");

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
 * `resolve(root, base)` over the wasm reader, in the shape `reader.mjs` returns.
 *
 * The adaptation is naming and nothing else — `tileOf` in and out as a `BigInt` because a
 * `dense_id` carries more bits than a `Number` holds, `shift` as a `Number` because it is at most
 * 63. Anything computed here rather than asked of the module would be this file agreeing with
 * itself.
 */
export function reader(Corpus) {
  return (root, base = "") => {
    const corpus = new Corpus(manifestFiles(root), base);
    const snapshot = corpus.snapshot();

    const types = snapshot.types.map((t) => ({
      type: t.type,
      prefix: t.prefix,
      container: t.container,
      chunkSize: t.chunk_size,
      shift: t.shift,
      tileOf: (denseId) => BigInt(corpus.tileOf(t.type, String(denseId))),
      tileUrl: (k) => corpus.vertexTileUrl(t.type, Number(k)),
      index:
        t.index === null || t.index === undefined
          ? null
          : {
              prefix: t.index.prefix,
              container: t.index.container,
              orderedBy: t.index.ordered_by,
              chunkSize: t.index.chunk_size,
              tiles: t.index.tiles,
            },
    }));

    const edges = snapshot.edges.map((e) => ({
      edgeType: e.edge_type,
      srcType: e.src_type,
      dstType: e.dst_type,
      prefix: e.prefix,
      directions: e.directions,
      adjacency: (direction) => {
        const declared = e.adjacencies.find((a) => a.direction === direction);
        if (declared === undefined) return null;
        return {
          direction,
          prefix: declared.prefix,
          container: declared.container,
          column: declared.column,
          chunkSize: declared.chunk_size,
          shift: declared.shift,
          tileOf: (denseId) => BigInt(denseId) >> BigInt(declared.shift),
          tileUrl: (k) => corpus.adjacencyTileUrl(e.edge_type, direction, Number(k)),
        };
      },
    }));

    const vertexType = (name) => {
      const found = name === undefined ? types[0] : types.find((t) => t.type === name);
      if (found === undefined) {
        // The module's own refusal, and it carries the message `expected.json` pins. Asked for
        // rather than reproduced here: a message this file wrote would be this file agreeing with
        // the table about a sentence the reader never says.
        corpus.vertexTileUrl(name, 0);
        throw new Error(`no vertex type ${name}`);
      }
      return found;
    };

    return {
      container: snapshot.container,
      types,
      edges,
      vertexType,
      window: ({ type, tiles, directions = ["src"] }) => {
        const got = corpus.window(type, Uint32Array.from(tiles.map(Number)), directions);
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
