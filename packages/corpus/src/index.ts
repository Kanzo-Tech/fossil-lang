/**
 * @fossil-lang/corpus — in-process TS binding for the fossil-graph verb surface.
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
 *   import { initFossilGraphWasm, createGraphClient } from '@fossil-lang/corpus';
 *   import wasmUrl from '@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm?url'; // Vite
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
 *   const { vertexUrls, edgeUrls, complete } = corpus.tilesFor({ tiles, directions: ['src'] });
 */

export { initFossilGraphWasm } from './load.js';
export type { InitFossilGraphWasmOpts } from './load.js';

// The one capability a host supplies, for every member of the door.
export type { QueryFn, QueryRow } from './query.js';

// `createGraphClient` is NOT exported, and its absence is the point of this barrel.
//
// It took `{ query, manifestFiles }` and returned the six verbs; `openCorpus` takes `(url,
// { query })` and returns those same six plus the camera. Two entry points over one manifest with
// no rule for choosing between them is how `node()` and `read({ where: "subject = …" })` came to
// be two answers to one question, in two languages, over two SQL generators. There is one door
// now, and `client.ts` is the transport behind it — still a module, still tested directly by
// `tests/client.test.ts`, and public in the wasm-bindgen sense only because it is generated.


// The addressing half, and it deliberately shares nothing with the verb half but the manifest:
// no WASM to initialise, no `query` callback, no promise. A notebook, a CLI or a server opens the
// same corpus with this and a Parquet reader of its own choosing.
//
// Reached that way it is `@fossil-lang/corpus/address`, NOT this barrel. Everything above this line
// static-imports `../pkg/fossil_graph_wasm.js`, so the barrel cannot load without the wasm-bindgen
// output — which is why the subpath exists and why `tests/address-standalone.test.ts` imports it
// from a package directory with no `pkg/` in it. The re-export below is for consumers who are
// already using the verbs and want both halves from one specifier.
export {
  resolveCorpus,
  parseTileCodes,
  shiftFor,
  tailRows,
  tileOf,
  tilesOf,
  TILE_SHIFT,
  CorpusManifestError,
  GRAPH_INFO_PATH,
} from './address.js';
export type {
  AdjacencyAddress,
  Direction,
  EdgeAddress,
  EdgeTiles,
  Gap,
  IndexAddress,
  GapReason,
  ResolveCorpusOptions,
  CorpusAddressing,
  VertexAddress,
  AddressedTiles,
  TileCodes,
  TileCodesDocument,
} from './address.js';

// The door. It had a subpath of its own — `@fossil-lang/corpus/corpus` — whose one justification
// was that its closure reached no WASM, and it reaches the verbs now, so the justification is gone
// and so is the subpath. The objection that answered was never large: `openCorpus` runs a `DESCRIBE`
// per vertex type before it returns, so a caller holding one is already in "I have an engine"
// territory. `./address` is the half that genuinely stands alone, and it keeps its subpath.
export { CorpusReadError, openCorpus } from './corpus.js';
export type {
  Answer,
  Box,
  Corpus,
  PlacedEdge,
  CorpusEdgeType,
  CorpusField,
  CorpusTypes,
  PlacedVertex,
  CorpusVertexType,
  Extent,
  Neighbourhood,
  NeighboursParams,
  NodeParams,
  OpenCorpusOptions,
  WindowAnswer,
  WindowParams,
} from './corpus.js';

// The codegen'd wire types (params + results + the Operation envelope). Re-exported
// so consumers type their calls without reaching into the generated module.
export type * from './generated.js';
