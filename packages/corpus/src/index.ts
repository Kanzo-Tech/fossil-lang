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
 * addresses come from `resolveCorpus` over the same `manifestFiles` — the same reader, over the
 * same WASM module the verbs run in:
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


// The addressing half. It shares the manifest with the verb half and now shares the READER too:
// `resolveCorpus` resolves through `fossil-graph-wasm`, so `initFossilGraphWasm` has to have been
// awaited before it, exactly as for a verb.
//
// **It had a subpath, `@fossil-lang/corpus/address`, and it is gone.** That entry existed so a
// notebook, a CLI or a server could address a corpus with no WASM and a Parquet reader of its own —
// and the only way to keep that promise was a second implementation of the addressing in
// TypeScript, agreeing with `crates/fossil-graph/src/plan.rs` because people kept making it agree.
// A wasm-free path is that copy, so the copy went and the subpath went with it.
export { resolveCorpus, CorpusManifestError, GRAPH_INFO_PATH } from './address.js';
export type {
  Direction,
  EdgeAddress,
  EdgeTiles,
  Gap,
  IndexAddress,
  ProjectionAddress,
  GapReason,
  ResolveCorpusOptions,
  CorpusAddressing,
  VertexAddress,
  AddressedTiles,
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
  Frame,
  FrameCost,
  FrameParams,
  LevelInfo,
  Neighbourhood,
  NeighboursParams,
  NodeParams,
  OpenCorpusOptions,
  Pixels,
  RowsAnswer,
  RowsParams,
} from './corpus.js';

// The writer's column table, by ROLE — `corpus.bnf` through `cargo xtask corpus`, and the same
// table `crates/fossil-sinks/src/generated.rs` carries on the Rust side.
//
// Exported because the alternative is a consumer spelling the names again. `cluster_id` was
// written down in `corpus.bnf`, in this generated file, twice in this package, and three times in
// `apps/playground` — six statements of one fact, of which the only machine-readable one had no
// consumer. A reader asks for the ROLE it means and gets whatever the writer calls it.
export {
  PAYLOAD_ADDRESS,
  PAYLOAD_CATEGORICAL,
  PAYLOAD_COORDINATES,
  PAYLOAD_IDENTITY,
} from './vocabulary.generated.js';
export type { ColumnRole, WriterColumn } from './vocabulary.generated.js';

// The codegen'd wire types (params + results + the Operation envelope). Re-exported
// so consumers type their calls without reaching into the generated module.
export type * from './generated.js';
