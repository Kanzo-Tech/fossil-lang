# @fossil-lang/graph

In-process TypeScript binding for the **fossil-graph** verb surface — the
larger-than-RAM query layer for [`@fossil-lang/viewer`](../viewer).

Verb→SQL logic runs in WASM (`fossil-graph-wasm`, single-source with the native
runtime); SQL **execution** is delegated to a host-provided DuckDB-WASM `query`
callback. The host's DuckDB streams GraphAr Parquet over httpfs, so the binding
never materialises rows in JS — that's what lets the viewer scale past RAM.

```
@fossil-lang/graph (this package)
  ├─ pkg/            fossil-graph-wasm, wasm-bindgen --target web (built, gitignored)
  ├─ src/generated.ts   verb Params/Result types — codegen'd from schemars JSON Schema
  ├─ src/load.ts        initFossilGraphWasm({ wasmUrl })
  └─ src/client.ts      createGraphClient({ query, manifestFiles })
```

## Usage

```ts
import { initFossilGraphWasm, createGraphClient } from '@fossil-lang/graph';
import wasmUrl from '@fossil-lang/graph/pkg/fossil_graph_wasm_bg.wasm?url'; // Vite

await initFossilGraphWasm({ wasmUrl });

const graph = createGraphClient({
  // Adapt the host's DuckDB-WASM to row objects. In keasy this wraps the Mosaic
  // coordinator; the binding stays free of an Arrow/Mosaic dependency.
  query: async (sql) => {
    const table = await coordinator.query(sql, { type: 'arrow' });
    return table.toArray().map((r) => r.toJSON());
  },
  // The GraphAr manifest YAMLs, pre-fetched by the host (small: one index +
  // per-type files). The binding keeps them by value to stay sync in WASM.
  manifestFiles,
});

const { types } = await graph.listVertexTypes();
const hist = await graph.histogram({ vertex_type: 'Person', field: 'age', bins: 20 });
```

## Verbs

Schema (`listVertexTypes`, `listEdgeTypes`, `describeField`), discovery
(`searchByLabel`, `findNeighbors`, `findPath`), aggregation (`aggregate`,
`histogram`, `topK`), GraphRAG (`summarizeCluster`, `answerWithCommunities`),
viewport (`viewport`, `setSelection`), and the `executeSql` escape hatch. The
GraphRAG/search verbs require writer enrichment (embeddings + cluster summaries,
fossil-sinks W3); `viewport` returns real positions once the writer emits
`x`/`y`.

## Build

```sh
pnpm --filter @fossil-lang/graph build   # build:wasm → gen:types → tsc
```

`build:wasm` needs `wasm-bindgen` 0.2.120 (`cargo install --version 0.2.120
wasm-bindgen-cli`); `gen:types` runs `cargo` to dump the schemas. Both are
mirrors of the [`@fossil-lang/wasm`](../wasm) build (ADR-0030 / `--target web`).
