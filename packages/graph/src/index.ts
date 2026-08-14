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

// The codegen'd wire types (params + results + the Operation envelope). Re-exported
// so consumers type their calls without reaching into the generated module.
export type * from './generated.js';
