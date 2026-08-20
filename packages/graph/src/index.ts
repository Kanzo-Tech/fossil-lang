/**
 * @fossil-lang/graph — in-process TS binding for the fossil-graph verb surface.
 *
 * The query layer for whatever draws the graph — fossil ships no viewer.
 * Verb→SQL runs in WASM (`fossil-graph-wasm`, single-source with
 * the native runtime), and SQL execution is delegated to a host-provided
 * DuckDB-WASM `query` callback (e.g. keasy's Mosaic coordinator). This is what
 * makes a viewer larger-than-RAM: the host's DuckDB streams Parquet over
 * httpfs; the binding
 * never materialises rows in JS.
 *
 * Consumer pattern:
 *
 *   import { initFossilGraphWasm, createGraphClient } from '@fossil-lang/graph';
 *   import wasmUrl from '@fossil-lang/graph/pkg/fossil_graph_wasm_bg.wasm?url'; // Vite
 *
 *   await initFossilGraphWasm({ wasmUrl });
 *   const graph = createGraphClient({ query, manifestFiles });
 *   const { types } = await graph.listVertexTypes();
 *
 * The verbs answer questions and return ids; what gets *drawn* comes from tiles, and the tile
 * addresses come from `resolveCorpus` over the same `manifestFiles` — synchronous, WASM-free, and
 * the only published copy of the arithmetic a reader would otherwise re-derive:
 *
 *   const corpus = resolveCorpus({ manifestFiles, base: '/bench/1000000' });
 *   const { vertexUrls, edgeUrls, complete } = corpus.window({ tiles, directions: ['src'] });
 */

export { initFossilGraphWasm } from './load.js';
export type { InitFossilGraphWasmOpts } from './load.js';

export { createGraphClient } from './client.js';
export type {
  GraphClient,
  CreateGraphClientOpts,
  QueryFn,
  QueryRow,
} from './client.js';

// The addressing half, and it deliberately shares nothing with the verb half but the manifest:
// no WASM to initialise, no `query` callback, no promise. A notebook, a CLI or a server opens the
// same corpus with this and a Parquet reader of its own choosing.
//
// Reached that way it is `@fossil-lang/graph/address`, NOT this barrel. Everything above this line
// static-imports `../pkg/fossil_graph_wasm.js`, so the barrel cannot load without the wasm-bindgen
// output — which is why the subpath exists and why `tests/address-standalone.test.ts` imports it
// from a package directory with no `pkg/` in it. The re-export below is for consumers who are
// already using the verbs and want both halves from one specifier.
export { resolveCorpus, shiftFor, tileOf, TILE_SHIFT, CorpusManifestError, GRAPH_INFO_PATH } from './address.js';
export type {
  AdjacencyAddress,
  Direction,
  EdgeAddress,
  EdgeTiles,
  Gap,
  GapReason,
  ResolveCorpusOptions,
  ResolvedCorpus,
  VertexAddress,
  Window,
} from './address.js';

// The codegen'd wire types (params + results + the Operation envelope). Re-exported
// so consumers type their calls without reaching into the generated module.
export type * from './generated.js';
