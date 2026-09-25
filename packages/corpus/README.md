# @fossil-lang/corpus

The query layer for whatever draws the graph. **Fossil ships no viewer.**

**One door, and it is one function.** `open(url, { query })`
returns discovery, the camera (`extent`, `frame`, `rows`, `node`, `neighbours`)
and the five bounded verbs (`read`, `expand`, `path`, `aggregate`, `schema`) on
one object. No tiles, no `dense_id`, no Morton, no `by_source`, no prefixes, no
footers.

**Three depths on that one name, and the depth is what the caller brings.** The
engine-free route was a second function, `resolveCorpus`, and is now the shallow
end of the same call:

```ts
await open(url,  { query })          // Corpus — the whole door
await open(url,  { readText })       // CorpusAddressing — manifests only
await open(base, { manifestFiles })  // CorpusAddressing — no request at all
```

`query` is the engine; `readText` is `(url) => text` and buys only the
manifests; `manifestFiles` is for a caller that already holds them, where `base`
is what every address is prepended with. Exactly one is required, and giving
none is a `TypeError` that names the three. `corpus.addressing` is none of the
three — it is what the first rung already resolved, for a caller who paid for
it.

The capability was always the real difference between the two functions, and a
capability is an argument. The call is now always asynchronous; the synchronous
form `resolveCorpus` had once the module was up had no production consumer.

Everything else that was on the barrel is reachable through the door or not at
all:

- `createGraphClient` — the in-process transport the verbs dispatch through.
  `open` already holds the two things it took.
- `GRAPH_INFO_PATH` — the index's file name is the door's business. `open`
  takes a corpus URL and reads whatever is under it. **That was applied to the
  engine-free route too, and it should not have been**: a caller with no engine
  cannot read anything for itself, so it was left hand-writing a scan of the
  index's `vertices:`/`edges:` lists — three copies of it, one in another
  repository. The file name stays off the surface and the *sequence* is
  published instead, as `readText`: lend a text reader and the package reads the
  index, the manifests it names, and nothing else.
- `initFossilGraphWasm` — the boot, awaited inside `open`. A consumer
  should not have to know there is a wasm module, let alone sequence two calls
  against it. Nor does it say where the module is: the `.wasm` ships in this
  package and the host's bundler emits it as an asset (`new URL(…, import.meta.url)`
  in the glue). `wasm` is left on the options for a host with no bundler — Node,
  which hands the bytes. (Vite's dev server: see `@fossil-lang/wasm`'s README for
  the one `optimizeDeps` line.)
- `export type *` — an unbounded star publishes whatever the codegen makes, now
  and later, with nobody deciding. The twelve params/results the members name
  are re-exported by name instead.

`Corpus.levels()` is the free function `levelsOf(addressing, type?)`, because
every line of it is addressing rather than a member of a door.

`./corpus` was a subpath whose one justification was that its closure reached no
WASM; the door reaches the verbs now, so it does, and the subpath went with the
justification.

### The addressing costs a WASM module now, and that is the price

`@fossil-lang/corpus/address` was a **fourth** entry, and it is deleted. It
published the addressing on a closure that reached no wasm-bindgen output:
synchronous arithmetic over a parsed manifest, importable by a notebook, a CLI
or a server with a Parquet reader of its own and no WebAssembly.

Keeping that promise meant keeping a second implementation of the addressing.
`crates/fossil-graph/src/plan.rs` is the first — `fossil-mcp` is a native server
with no JS runtime, so it cannot be the one that goes — and 1,058 lines of
TypeScript beside it composed the same URLs from the same manifest, agreeing
with it because people kept making them agree. Nothing compared the two.

So the TypeScript reader is gone and the addressing asks the Rust one, through
`fossil-graph-wasm`. What that costs:

- **The module has to be up before anything resolves**, exactly as for a verb.
  Composing a URL was arithmetic over bytes the host already held and is now a
  call into an instantiated module. `open` awaits the boot itself, on
  every rung.
- **There is no WASM-free path, and there will not be one.** A second
  implementation is what a WASM-free path is.
- `tests/address-standalone.test.ts` proved the subpath's closure loaded from a
  package directory with no `pkg/` in it. It went with the subpath.

The barrel — every part of it — static-imports the wasm-bindgen output.

```
@fossil-lang/corpus (this package)
  ├─ pkg/            fossil-graph-wasm, wasm-bindgen --target web (built, gitignored)
  ├─ src/generated.ts   verb Params/Result types — codegen'd from schemars JSON Schema
  ├─ src/load.ts        the memoised wasm boot (internal; open awaits it)
  ├─ src/client.ts      the verb transport, dispatched through by the door (not exported)
  ├─ src/query.ts       QueryFn + ReadTextFn — what a host lends, and how deep each one reaches
  ├─ src/manifest.ts    graph.graph.yml, scanned for the paths to fetch next
  ├─ src/address.ts     addressManifests + levelsOf — the binding, not the reader
  └─ src/corpus.ts      open(url, { query | readText | manifestFiles }) — THE DOOR
```

## Usage

```ts
import { open } from '@fossil-lang/corpus';

const corpus = await open('https://data.example/graph', {
  // Adapt the host's DuckDB-WASM to row objects. In keasy this wraps the Mosaic
  // coordinator; the binding stays free of an Arrow/Mosaic dependency.
  query: async (sql) => {
    const table = await coordinator.query(sql, { type: 'arrow' });
    return table.toArray().map((r) => r.toJSON());
  },
  // No wasm URL: `open` boots the module (memoised) and the bundler already
  // emitted its `.wasm`. The addressing needs it too — it asks
  // `fossil_graph::plan` rather than re-deriving anything.
});

corpus.types;                              // vertex types with counts and columns, edge types
const box = await corpus.extent();         // the coordinates a rectangle is expressed in
const here = await corpus.rows({ x: box.minX, y: box.minY, w: 100, h: 100 });

// The camera. Two questions, two names: `rows` is what is here, entire, and
// `frame` is what to draw at this resolution. The level comes from the canvas.
const rect = { x: box.minX, y: box.minY, w: 1000, h: 1000 };
const frame = await corpus.frame({ ...rect, pixels: { w: 1200, h: 800 } });
frame.level;         // which level the canvas could show — `level:` names one by hand
frame.marks;         // a prefix length — everything past it is an anchor
frame.cost.requests; // and `.bytes`: what it spent, in the terms a network tab has

const one = await corpus.node('https://example.org/person/15');
const hood = await corpus.neighbours([one.id], { depth: 2 });

const { vertices } = await corpus.schema();
const hist = await corpus.aggregate({
  vertex_type: 'Person',
  group_by: 'age',
  agg: 'count',
  bins: 20,
});
```

**One argument is the corpus and the other is the engine.** The host brings a
`query` callback — DuckDB-WASM in a browser, a native `duckdb::Connection` on a
server — and this package keeps its zero runtime dependencies. Three of the four
members have to decode Parquet, and an engine also does the footer pruning a
rectangle read would otherwise re-derive by hand. `read_text`, `read_parquet` and
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
whether one is a file (`chunk{k}.parquet`) or a row group inside a single
`tiles.parquet` is a second question — the one `graph.graph.yml`'s `container`
answers, because a reader over HTTP has no directory to list and cannot work it
out. **Both are read.** `apps/corpus/integration/containers.test.ts` writes
the same graph in each and asserts the answers back are identical — extent,
window, `node`, `neighbours` — and that the row-group container names strictly
fewer files for the same window (over HTTP, 5.6 requests per window against 22.3
at five million vertices). Reading only the file-per-tile container `fossil run`
writes and refusing the other by name is what this paragraph used to claim, and
it is the one thing the reader must not do: the row-group container is the
measured winner and the one fossil does not write yet, so refusing it would
refuse the corpus this package exists to read. Neither is globbed, because a
glob picks up the staged single-file copy beside the tiles and counts every row
twice.

## The verbs

`read` · `expand` · `path` · `aggregate` · `schema`, and `executeSql` when the
host asks for it.

Verb→SQL runs in WASM (`fossil-graph-wasm`, single-source with the native
runtime and with `fossil-mcp`'s server-side surface); SQL **execution** goes
through the same `query` callback everything else does. The host's DuckDB
streams Parquet over httpfs, so the binding never materialises rows in JS —
that is what lets a host scale past RAM.

**The verbs name tables and a corpus is files**, so the door registers the
views they expect on the first verb call, over paths the manifest already gave
it — `CREATE OR REPLACE TEMP VIEW "Person"`, and `TEMP` because a host's own
`Person` table is not unlikely and a temp view shadows it rather than replacing
it. `crates/fossil-mcp` does the identical thing for the identical reason.

**A verb reads the manifest's vocabulary; the camera reads the bytes.** A verb
composes SQL before it has seen a byte, so its column list is the payload
projection's declared `properties`;
`open` had a round trip to spend and spent it on a `DESCRIBE`. On the
conformance corpus that is **three** declared properties against **seven**
columns on disk, so `read` answers with `subject`, `birth_year` and `postcode`
while `corpus.types` reports all seven — the four the manifest never names are
`dense_id`, `x`, `y` and `cluster_id`, which the writer puts there and the
vocabulary does not. Neither is wrong and they are not the same question.

- **`read`** — rows of one vertex type under a `where` predicate, an order and
  a limit. `where` is SQL and carries the same authority as `executeSql`, so it
  is governed by the same option and refused with it.
- **`expand`** — the neighbourhood of a set of vertices. `all` walks outward up
  to `depth`; `into` keeps only the edges whose both ends are in the set.
- **`path`** — the shortest route between two vertices.
- **`aggregate`** — one grouping, over values or, with `bins`, over equal-width
  ranges. Binning is grouping, so there is no histogram verb.
- **`schema`** — the vertex and edge types with their counts. Name a
  `vertex_type` for its per-field statistics, and a `field` for its samples;
  a bare call runs no per-field query.
- **`executeSql`** — the escape hatch, for the question the other five cannot
  shape. **Withheld unless the host asks**: `open(url, { query })` returns
  a `Corpus` with no `executeSql` member and a `read` that refuses a `where`;
  `open(url, { query, sql: 'allowed' })` returns a `SqlCorpus` with both.

  One option, two consequences, no second knob — `crates/fossil-graph`'s
  `Verb::reaches_raw_sql()` is `["read", "execute_sql"]`, so deleting the hatch
  would have left the other door open. This is `crates/fossil-mcp`'s `SqlPolicy`
  ported: closed by default because the five bounded verbs cost a function of the
  answer and the hatch costs a function of whatever was typed. It is not a
  sanitiser and not a security boundary — the engine and the files are the
  host's; see [`/docs/design/privacy`]. What it holds is the coupling, and the
  fact that a host has to write the word down.

**None of them draws.** The camera is addressed, not queried — the
LOD is not a filter but a different relation, and a `WHERE` cannot change which
table it reads. A filter that must change the picture answers with ids, and the
canvas masks its resident tiles with them.

## Addressing

What gets drawn comes from tiles, and a tile's URL is arithmetic over four
manifest fields. That arithmetic is `fossil_graph::plan`, in Rust, and this
package is how JavaScript asks it — one reader, reached from two languages,
rather than one contract implemented in each.

**It is not a second import.** `corpus.addressing` is the resolved plan the door
already built while it was opening, so a drawing path that fetches its own tiles
reaches it through the corpus it already has:

```ts
const { addressing } = await open(url, { query });

addressing.vertexType().tileUrl(10);
// '/bench/1000000/vertex/Person/chunk10.parquet'

const { vertexUrls, edgeUrls, complete, gaps } = addressing.tilesFor({
  tiles: [10, 11],          // from the host's own footer read — see below
  directions: ['src'],      // out-edges only; see `drawing` for what a picture may draw
});
// complete: false
// gaps: [{ edgeType: 'knows', direction: 'dst', reason: 'not-requested' }]
```

**And it is not a second function either.** A caller with no engine asks the
same name for the same object, by lending less:

```ts
// The package reads the index and the manifests it names. You supply the reader.
const addressing = await open(base, {
  readText: async (url) => (await fetch(url)).text(),
});

// Or, with the manifests already in hand, nothing is read at all.
const same = await open(base, { manifestFiles });
```

`container` on every address says which number a footer hands you is the tile:
under `rowgroups` the row group **is** the tile, under `files` the file is. That
rule used to live only in `/docs/format`, so a footer reader had the
discriminant and not the rule.

**It never composes an address the corpus does not publish.** An edge type with
no projection at `scale: 1` for a direction, or one declaring it with no `path`,
has tiles nobody can address: `adjacency('dst')` is `null` and the direction is
absent from `directions`, rather than a string that 404s in a browser with no
type error. A corpus whose `src_chunk_size` disagrees with the vertex type
addressing it is refused when it is opened, not at the first request.

**A partial answer says which part.** CSR alone is complete for *drawing* —
every drawable edge has its source on screen — and incomplete for *incidence*.
`complete` is about incidence, and `gaps` separates the caller not asking from
the corpus not publishing.

**No `fetch`, and no boxes.** The module is instantiated once and then answers
without touching the network; which tiles a rectangle touches comes from the
per-tile `x`/`y` statistics in the Parquet footers; reading a footer needs a
Parquet reader, the host has one, and the tile numbers come back here. Tile
cache, debounce, supersede-cancellation and sampling stay in the reader too.

The contract is executable: `apps/corpus/conformance/` holds a corpus, the
manifest cases a corpus cannot hold, and `expected.json` — every address that
must compose and every one that must be refused. `tests/conformance.test.ts`
runs it here, through the wasm32 build that actually ships;
`crates/fossil-graph/tests/conformance.rs` runs it natively, where `usize` is 64
bits rather than 32; and `apps/corpus/conformance/verify.mjs` runs it in plain
Node with no npm at all, which is the position a third-party reader is in and
the only one of the three that is a separate implementation.

## Build

```sh
pnpm --filter @fossil-lang/corpus build   # build:wasm → gen:types → tsc
```

`build:wasm` needs `wasm-bindgen` 0.2.120 (`cargo install --version 0.2.120
wasm-bindgen-cli`); `gen:types` runs `cargo` to dump the schemas. Both are
mirrors of the [`@fossil-lang/wasm`](../wasm) build (`--target web`).
