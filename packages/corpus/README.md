# @fossil-lang/corpus

The query layer for whatever draws the graph. **Fossil ships no viewer.**

**One door, and it is one function.** `open(url, { engine })` returns
discovery, the read path a view is built on (`tileMatrix`, `scan`, `edges`,
`node`), and the verbs (`schema`, `relations`, and `executeSql` when the host
asks) on one object. `/docs/design/backend` is the argument, and the members
that path replaced — `extent`, `frame`, `rows`, `neighbours` — are gone.

**It needs an engine, and the engine is the host's.** `@fossil-lang/types`'
`Engine`: `query(sql, { signal })` answering in Arrow-shaped columns — an
apache-arrow `Table` is one — and interrupting the running statement when the
signal aborts. A job's corpus is opened through it under the credential the host
vends; a corpus at a URL through it alone:

```ts
await open(job, { engine, host })          // a job's corpus
await open(url, { engine })                // a corpus at a URL
await open(url, { engine, manifestFiles }) // with the manifests in hand
```

Giving no `engine` is a `TypeError`. The addressing the open resolves — the
arithmetic over the manifests — is internal: a consumer that needs a URL has
outgrown a surface whose claim is that a corpus is a URL.

Everything else that was on the barrel is reachable through the door or not at
all:

- `createGraphClient` — the in-process transport the verbs dispatch through.
  `open` already holds the two things it took.
- `GRAPH_INFO_PATH` — the index's file name is the door's business. `open`
  takes a corpus URL and reads the index and the manifests it names through the
  engine it was given.
- `initFossilGraphWasm` — the boot, awaited inside `open`. A consumer
  should not have to know there is a wasm module, let alone sequence two calls
  against it. Nor does it say where the module is: the `.wasm` ships in this
  package and the host's bundler emits it as an asset (`new URL(…, import.meta.url)`
  in the glue). `wasm` is left on the options for a host with no bundler — Node,
  which hands the bytes. (Vite's dev server: see `@fossil-lang/wasm`'s README for
  the one `optimizeDeps` line.)
- `export type *` — an unbounded star publishes whatever the codegen makes, now
  and later, with nobody deciding. The params/results the members name
  are re-exported by name instead.

`./corpus` was a subpath whose one justification was that its closure reached no
WASM; the door reaches the verbs now, so it does, and the subpath went with the
justification.

### The addressing costs a WASM module now, and that is the price

`@fossil-lang/corpus/address` was a **fourth** entry, and it is deleted. It
published the addressing on a closure that reached no wasm-bindgen output:
synchronous arithmetic over a parsed manifest, importable by a notebook, a CLI
or a server with a Parquet reader of its own and no WebAssembly.

Keeping that promise meant keeping a second implementation of the addressing.
`crates/fossil-graph/src/plan.rs` is the first, and the conformance suite runs it
natively — and 1,058 lines of
TypeScript beside it composed the same URLs from the same manifest, agreeing
with it because people kept making them agree. Nothing compared the two.

So the TypeScript reader is gone and the addressing asks the Rust one, through
`fossil-graph-wasm`. What that costs:

- **The module has to be up before anything resolves**, exactly as for a verb.
  Composing a URL was arithmetic over bytes the host already held and is now a
  call into an instantiated module. `open` awaits the boot itself.
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
  ├─ src/query.ts       the engine's two faces: rows, and a Batch a read hands back
  ├─ src/manifest.ts    graph.graph.yml, scanned for the paths to fetch next
  ├─ src/address.ts     addressManifests — the binding to fossil_graph::plan, not a reader
  ├─ src/open.ts        open(url, { engine } | { engine, host }) — THE DOOR
  ├─ src/corpus.ts      the Corpus surface's types
  ├─ src/tile-manifest.ts   the published per-tile statistics, read once per type
  ├─ src/tile-matrix.ts OGC's tile matrix set: every zoom, every tile, its rows and box
  ├─ src/expression.ts  Iceberg's Filter: bind, inclusive + strict evaluators, residual SQL
  ├─ src/scan.ts        Table.scan: plan() over every zoom, read() a batch per tile
  ├─ src/edges.ts       edges(): CSR/CSC at Z, the quotient below
  ├─ src/identity.ts    node: the index seek and its key ranges
  ├─ src/verbs.ts       the views the verbs read, schema, relations, executeSql
  └─ src/sql.ts         literals, identifiers, the 64-bit id guard
```

## Usage

The read path a view is built on is three members, and every one of them is
planned from the tile manifest `open` read — Iceberg's scan over OGC's tile
matrix, `/docs/design/backend`:

```ts
import { open } from '@fossil-lang/corpus';

const corpus = await open(url, { engine });

// The zooms: the payload at Z, every rung of the cell pyramid below it, coarsest
// first, every tile with its rows and its box. Synchronous — nothing to fetch.
const set = corpus.tileMatrix('Person');

// A scan is built once per filter, not once per camera move. The filter is data.
const scan = corpus.scan({
  type: 'Person',
  filter: { column: 'cluster_id', op: 'in', values: [3, 7] },
  select: ['dense_id', 'x', 'y', 'cluster_id'],
});
const tasks = scan.plan();                     // every zoom, pruned, each with its residual
const visible = tasks.filter((t) => t.z === z && overlaps(t.bbox, camera));
const batches = await scan.read(visible, { signal }); // a batch per tile, cancellable

// The relations incident to each tile: CSR at `src`, CSC at `dst`, the rung's
// quotient below Z; and what was declined, with fossil's reason. One answer a tile.
const answers = await corpus.edges({ from: visible, direction: 'src', signal });
```

- **`tileMatrix(type)`** — OGC 17-083r4's `TileMatrixSet`: `extent`,
  `coordinates`, `mode` (the channel a cell's `mode` summarises), and
  `tileMatrices[z]` with `kind` (`rows` at `Z`, `cells` below), `count`,
  `shift`, `tileRows` and every tile's `{ tile, rows, bbox }`. Throws for a type
  that publishes no tile manifest.
- **`scan({ type, filter?, select? })`** — Iceberg's `Table.scan`, bound when it
  is built (an unknown column throws here). `plan()` is `plan_files` at every
  zoom, pruned by the inclusive metrics evaluator over the published bounds and
  null counts, each task carrying the residual the strict evaluator could not
  settle; a filter on a payload column has no task below `Z`. `read(addresses)`
  answers a batch per address, reading each run of consecutive tiles as one
  statement with one conjunctive range on `dense_id` (or `cell_id`) and
  splitting it back per tile; an abort rejects with `AbortError`.
- **`edges({ from, direction, relation?, signal })`** — an answer per address in
  `from`, a run of consecutive tiles in one statement: the adjacency's aligned
  half at `Z` (`by_source` or `by_target`), the rung's quotient below it
  (undirected, one row per cell pair, keyed on the lower cell; `dst` is
  declined there), skipping a tile the manifest says has none. A relation to
  another type is declined as `other-space` unless `relation` names it.

- **`node(id)`** — one vertex by identity, with its `dense_id` and position, so
  a pinned or searched vertex is placed without a scan; its tile is `denseId`
  over `tileRows`.

A walk of more than one hop is the caller's loop over `node` and `edges`, and a
rectangle is a filter on `x` and `y`: both are composed in
`tests/corpus.test.ts` against the numbers `conformance/expected.json`'s full
scan published.

**One argument is the corpus and the other is the engine.** The host brings an
`Engine` — DuckDB-WASM in a browser — and this package keeps its zero runtime
engine dependencies: it decodes no Parquet and links no engine. `read_text`,
`read_parquet` and `parquet_metadata` are the whole of what it is asked for.

**`id` is the subject IRI, never the `dense_id`.** Redoing the layout renumbers
every vertex, so an address held outside the corpus names a different vertex
after the next write. `node` refuses a `BigInt` with a `TypeError` that says so.
Where the type declares an `index:` the lookup is a seek; where it does not, it is
a scan of the `subject` column, and `types.vertices[i].indexed` says which;
`src/identity.ts` measures both at the call site.

**An answer says what it is missing.** `edges` answers with the relations it read
and the ones it `declined`, with fossil's `GapReason`: `not-declared` for an
orientation the corpus does not publish, `other-space` for a relation whose far
end is another type.

**What it does not absorb**: the container. A tile is a range of rows, and
whether one is a file (`chunk{k}.parquet`) or a row group inside a single
`tiles.parquet` is a second question — the one `graph.graph.yml`'s `container`
answers, because a reader over HTTP has no directory to list and cannot work it
out. **Both are read.** `packages/corpus/integration/containers.test.ts` writes
the same graph in each and asserts the answers back are identical — the tile
matrix, a window's tiles and their edges, `node` — and that the row-group
container names strictly fewer files for the same window (over HTTP, 5.6
requests per window against 22.3 at five million vertices). Neither is globbed,
because a glob picks up the staged single-file copy beside the tiles and counts
every row twice.

## The verbs

`schema`, and `executeSql` when the host asks for it; `relations` is one bare
`schema` shaped for a host that registers the corpus.

Verb→SQL runs in WASM (`fossil-graph-wasm`, single-source with the Rust verb
structs); SQL **execution** goes
through the same engine everything else does. The host's DuckDB
streams Parquet over httpfs, so the binding never materialises rows in JS —
that is what lets a host scale past RAM.

**The verbs name tables and a corpus is files**, so the door registers the
views they expect on the first verb call, over paths the manifest already gave
it — `CREATE OR REPLACE TEMP VIEW "Person"`, and `TEMP` because a host's own
`Person` table is not unlikely and a temp view shadows it rather than replacing
it.

**A verb reads the manifest's vocabulary; the door reads the bytes.** A verb
composes SQL before it has seen a byte, so its column list is the payload
projection's declared `properties`;
`open` had a round trip to spend and spent it on a `DESCRIBE`. On the
conformance corpus that is **three** declared properties against **seven**
columns on disk, so `schema` lists `subject`, `birth_year` and `postcode`
while `corpus.types` reports all seven — the four the manifest never names are
`dense_id`, `x`, `y` and `cluster_id`, which the writer puts there and the
vocabulary does not. Neither is wrong and they are not the same question.

- **`schema`** — the vertex and edge types with their counts. Name a
  `vertex_type` for its per-field statistics, and a `field` for its samples;
  a bare call runs no per-field query. `{ stats: true }` puts every type's
  statistics on its own summary (`vertices[i].stats`), one batched query per
  type — the call a schema panel makes. Each `FieldStat` carries a `role`
  (a chart-axis guess from name and cardinality) and a `kind` —
  `numeric` · `temporal` · `categorical`, read off the GraphAr spelling — which
  is what decides whether an axis may bin it. A host asks for the kind and
  keeps no table of spellings.
- **`executeSql`** — the escape hatch: SQL over the views `relations` names.
  **Withheld unless the host asks**: `open(url, { engine })` returns a `Corpus`
  with no `executeSql` member; `open(url, { engine, sql: 'allowed' })` returns a
  `SqlCorpus` with it. Closed by default because every other member costs a
  function of the answer and the hatch costs a function of whatever was typed.
  It is not a sanitiser and not a security boundary — the engine and the files
  are the host's, and a corpus is files the host already holds. What it holds
  is the fact that a host has to write the word down.

**None of them draws.** The camera is addressed, not queried — the
LOD is not a filter but a different relation, and a `WHERE` cannot change which
table it reads. A filter that must change the picture answers with ids, and the
canvas masks its resident tiles with them.

## Handing a corpus to a host

A host that keeps a corpus per job gives the door its engine and its `Host`,
and names the job:

```ts
const corpus = await open(jobId, { engine, host });
for (const r of await corpus.relations()) {
  // { kind: 'vertex', name, rows, files, columns } | { kind: 'edge', name, rows, files, … }
  record(r);
}
```

The corpus is the one prefix `host.credentials({ job }, 'read')` vends, and
every file of it — the manifests too — is read through the engine under that
credential: a scoped secret for S3, renewed before it expires for as long as the
corpus is open, and a lease per file for Azure (`@fossil-lang/storage`). The
host signs nothing, composes no path and derives no name. `close()` gives the
credential back.

What an Azure host lends file by file is `fossil_graph::plan::ReadPlan::files`:
every vertex type's payload and index, then every relation's adjacency. A
projection whose count is not declared cannot be enumerated and is left out, not
guessed. `relations()` is one bare `schema()` — the name is the corpus's
`table_name`, the rows its `count(*)` — joined to the addressing for the files.

## Addressing

What gets read comes from tiles, and a tile's address is arithmetic over four
manifest fields. That arithmetic is `fossil_graph::plan`, in Rust, and this
package asks it through `fossil-graph-wasm` — one reader, reached from two
languages, rather than one contract implemented in each. It is internal: the door
resolves it while it opens, and `tileMatrix`, `scan` and `edges` are what it is
for.

**It never composes an address the corpus does not publish.** An edge type with
no projection at `scale: 1` for a direction, or one declaring it with no `path`,
has tiles nobody can address, and `edges` declines that orientation as
`not-declared` rather than asking for a string that 404s. A corpus whose
`src_chunk_size` disagrees with the vertex type addressing it is refused when it
is opened, not at the first request.

The contract is executable: `packages/corpus/conformance/` holds a corpus, the
manifest cases a corpus cannot hold, and `expected.json` — every address that
must compose and every one that must be refused. `tests/conformance.test.ts`
runs what the published reader asks of it through the wasm32 build that actually
ships; `crates/fossil-graph/tests/conformance.rs` runs all of it natively, where
`usize` is 64 bits rather than 32; and `packages/corpus/conformance/verify.mjs`
runs it in plain Node with no npm at all, which is the position a third-party
reader is in and the only one of the three that is a separate implementation.

## Build

```sh
pnpm --filter @fossil-lang/corpus build   # build:wasm → gen:types → tsc
```

`build:wasm` needs `wasm-bindgen` 0.2.120 (`cargo install --version 0.2.120
wasm-bindgen-cli`); `gen:types` runs `cargo` to dump the schemas. Both are
mirrors of the [`@fossil-lang/wasm`](../wasm) build (`--target web`).
