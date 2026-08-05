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

const { vertices } = await graph.schema();
const hist = await graph.aggregate({
  vertex_type: 'Person',
  group_by: 'age',
  agg: 'count',
  bins: 20,
});
```

## The six verbs

`read` · `expand` · `path` · `aggregate` · `schema` · `executeSql`.

- **`read`** — rows of one vertex type under a `where` predicate, an order and
  a limit. `where` is SQL and carries the same authority as `executeSql`: gate
  it with the same permission.
- **`expand`** — the neighbourhood of a set of vertices. `all` walks outward up
  to `depth`; `into` keeps only the edges whose both ends are in the set.
- **`path`** — the shortest route between two vertices.
- **`aggregate`** — one grouping, over values or, with `bins`, over equal-width
  ranges. Binning is grouping, so there is no histogram verb.
- **`schema`** — the vertex and edge types with their counts. Name a
  `vertex_type` for its per-field statistics, and a `field` for its samples;
  a bare call runs no per-field query.
- **`executeSql`** — the escape hatch, for the question the other five cannot
  shape.

**None of them draws.** ADR-0042: the camera is addressed, not queried — the
LOD is not a filter but a different relation, and a `WHERE` cannot change which
table it reads. A filter that must change the picture answers with ids, and the
canvas masks its resident tiles with them.

## Build

```sh
pnpm --filter @fossil-lang/graph build   # build:wasm → gen:types → tsc
```

`build:wasm` needs `wasm-bindgen` 0.2.120 (`cargo install --version 0.2.120
wasm-bindgen-cli`); `gen:types` runs `cargo` to dump the schemas. Both are
mirrors of the [`@fossil-lang/wasm`](../wasm) build (ADR-0030 / `--target web`).
