# @fossil-lang/graph

The query layer for whatever draws the graph. **Fossil ships no viewer.**

Three entry points over one manifest, and each is a subpath because the other
two are not what a given reader wants:

- **`@fossil-lang/graph/corpus`** — the reference API. `openCorpus(url, { query })`
  and four members: `types`, `window`, `node`, `neighbours`. **Start here.** No
  tiles, no `dense_id`, no Morton, no `by_source`, no prefixes, no footers.
- **the verbs** (`@fossil-lang/graph`) — `read`, `expand`, `path`, `aggregate`,
  `schema`, `executeSql`. Verb→SQL runs in WASM (`fossil-graph-wasm`,
  single-source with the native runtime); SQL **execution** is delegated to a
  host-provided DuckDB-WASM `query` callback. The host's DuckDB streams Parquet
  over httpfs, so the binding never materialises rows in JS — that is what lets
  a host scale past RAM.
- **the addressing** (`@fossil-lang/graph/address`) — `resolveCorpus`, which
  turns the manifest into the tile URLs a camera reads. Synchronous, no WASM, no
  `fetch`, no promise. It is what `openCorpus` is built **on**, and it stays
  published for a drawing path that wants the URLs and does its own fetching.

The root barrel reaches `client.ts` and `load.ts`, and both static-import the
wasm-bindgen output, so a reader without it cannot load the barrel at all —
which is why the other two have subpaths. That is not a sentence:
`tests/address-standalone.test.ts` compiles each closure into a package
directory with no `pkg/` and no `node_modules`, and imports the subpath from a
child Node process.

```
@fossil-lang/graph (this package)
  ├─ pkg/            fossil-graph-wasm, wasm-bindgen --target web (built, gitignored)
  ├─ src/generated.ts   verb Params/Result types — codegen'd from schemars JSON Schema
  ├─ src/load.ts        initFossilGraphWasm({ wasmUrl })
  ├─ src/client.ts      createGraphClient({ query, manifestFiles })
  ├─ src/query.ts       QueryFn — the one capability a host supplies, for both surfaces
  ├─ src/manifest.ts    the GraphAr manifest, scanned without a YAML dependency
  ├─ src/address.ts     resolveCorpus({ manifestFiles, base })
  └─ src/corpus.ts      openCorpus(url, { query })
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

## The corpus API

```ts
import { openCorpus } from '@fossil-lang/graph/corpus';

const corpus = await openCorpus('https://data.example/graph', { query });

corpus.types;                              // vertex types with counts and columns, edge types
const box = await corpus.extent();         // the coordinates a window is expressed in
const view = await corpus.window({ x: box.minX, y: box.minY, w: 100, h: 100 });
const one = await corpus.node('https://example.org/person/15');
const hood = await corpus.neighbours([one.id], { depth: 2 });
```

**One argument is the corpus and the other is the engine.** The host brings a
`query` callback — DuckDB-WASM in a browser, a native `duckdb::Connection` on a
server — and this package keeps its zero runtime dependencies. Three of the four
members have to decode Parquet, and an engine also does the footer pruning a
window would otherwise re-derive by hand. `read_text`, `read_parquet` and
`parquet_metadata` are the whole of what it is asked for.

**`id` is the subject IRI, never the `dense_id`.** Redoing the layout renumbers
every vertex, so an address held outside the corpus names a different vertex
after the next write. `node` refuses a `BigInt` with a `TypeError` that says so.
The price is a scan of the `subject` column, because the corpus carries no index
from a name to an address; `src/corpus.ts` measures it at the call site.

**An answer says what it is missing.** `complete` is `true` only when every edge
incident to the answer's vertices is in it. `gaps` names an orientation that was
not read — the caller not asking and the corpus not publishing are different
reasons and are reported as different reasons — and `neighbours` also returns
the `frontier` its depth bound stopped at.

**What it does not absorb**: the container. A tile is a range of rows, and
whether one is a file or a row group inside a single file is not a question any
manifest field answers. This reads the file-per-tile container that `fossil run`
writes; handed the other it refuses by name, and it never globs, because a glob
picks up the staged single-file copy beside the tiles and counts every row twice.

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

**None of them draws.** The camera is addressed, not queried — the
LOD is not a filter but a different relation, and a `WHERE` cannot change which
table it reads. A filter that must change the picture answers with ids, and the
canvas masks its resident tiles with them.

## Addressing

What gets drawn comes from tiles, and a tile's URL is arithmetic over four
manifest fields. `resolveCorpus` is that arithmetic, published once instead of
copied into every reader:

```ts
import { resolveCorpus } from '@fossil-lang/graph/address';

const corpus = resolveCorpus({ manifestFiles, base: '/bench/1000000' });

corpus.vertexType().tileUrl(10);
// '/bench/1000000/vertex/Person/chunk10.parquet'

const { vertexUrls, edgeUrls, complete, gaps } = corpus.tilesFor({
  tiles: [10, 11],          // from the host's own footer read — see below
  directions: ['src'],      // the drawing read
});
// complete: false
// gaps: [{ edgeType: 'knows', direction: 'dst', reason: 'not-requested' }]
```

**It never composes an address the corpus does not publish.** An edge type with
no `adj_lists` entry for a direction, or one declaring it with no `prefix`, has
tiles nobody can address: `adjacency('dst')` is `null` and the direction is
absent from `directions`, rather than a string that 404s in a browser with no
type error. A corpus whose `src_chunk_size` disagrees with the vertex type
addressing it is refused when it is opened, not at the first request.

**A partial answer says which part.** CSR alone is complete for *drawing* —
every drawable edge has its source on screen — and incomplete for *incidence*.
`complete` is about incidence, and `gaps` separates the caller not asking from
the corpus not publishing.

**No `fetch`, and no boxes.** Which tiles a rectangle touches comes from the
per-tile `x`/`y` statistics in the Parquet footers; reading a footer needs a
Parquet reader, the host has one, and the tile numbers come back here. Tile
cache, debounce, supersede-cancellation and sampling stay in the reader too.

The contract is executable: `apps/corpus/conformance/` holds a corpus, the
manifest cases a corpus cannot hold, and `expected.json` — every address that
must compose and every one that must be refused. `tests/conformance.test.ts`
runs it here; `apps/corpus/conformance/verify.mjs` runs it in plain Node with no
npm at all, which is the position a third-party reader is in.

## Build

```sh
pnpm --filter @fossil-lang/graph build   # build:wasm → gen:types → tsc
```

`build:wasm` needs `wasm-bindgen` 0.2.120 (`cargo install --version 0.2.120
wasm-bindgen-cli`); `gen:types` runs `cargo` to dump the schemas. Both are
mirrors of the [`@fossil-lang/wasm`](../wasm) build (`--target web`).
