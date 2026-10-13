# @fossil-lang/executor

The DataFusion executor that runs fossil mappings **in the browser** — the heavy,
lazy-loaded counterpart to the LSP-only [`@fossil-lang/wasm`](../wasm).

Wraps the `fossil-df-wasm` wasm-bindgen build (`--target web`). The browser reads
every document and source the program names under the credentials the host vends,
runs the mapping on DataFusion-WASM, and writes the `fossil/1` corpus — one
Parquet per vertex type and per relation, and `fossil.json` last — under the job's
prefix. No mapping runtime on the server, and no other writer: this is the one
host that writes a corpus, in the browser and in Node.

## Usage

```ts
import { run } from '@fossil-lang/executor';
import type { Host } from '@fossil-lang/types';

const host: Host = {
  connections: async () => ({ lake: 's3://bucket/raw' }),
  // Iceberg REST StorageCredential[]: `read` per connection, `write` on `{ job }`.
  credentials: async (scope, access) => vend(scope, access),
};

const report = await run(program, { host, into: { job: jobId } });   // { location, dropped }, or throws a FossilError
```

`run` has two independent axes and a limit:

| | |
| --- | --- |
| **reads** | `host` — `read` vended per connection, `@conn` expanded, sources read by range — or `files` + `path`, bytes in hand |
| **writes** | `into: { job }` — the job's storage, through `write` vended on it, answering the report — or `into: 'memory'`, answering `{ report, files }` and asking for no `write` |
| **`limit`** | at most this many rows read from each source scan, before any join |

```ts
await run(program, { host, into: { job } });                          // the job's corpus
const { report, files } = await run(program, { host, into: 'memory', limit: 200 }); // a preview
const { report, files } = await run(program, { files, path });        // Node: bytes in, bytes out
```

With `limit`, an edge whose endpoint fell outside the sample is dropped and counted in
`report.dropped`, like any edge that names no vertex. With `files`, `into` is `'memory'`; `into:
{ job }` is refused — a type error, and `api/invalid-argument` at run time — because there is no host
to vend the write. `/docs/design/three-hosts` argues the shape.

**Breaking:** `run(program, { host, job })` is now `run(program, { host, into: { job } })`. There is
no alias; a host that passes `job` gets a type error, and at run time `api/invalid-argument` for
`into`. And
`run/destination-uncovered` is gone from the error catalogue: a run chooses its destination from
`into`, so no destination can be uncovered.

`run` boots the module itself the first time — the artefact is large and datafusion-heavy, so it is
fetched when the user runs a job and not on page-load; the bundler emits the `.wasm` as an asset.
It reads the documents the program names (every shape, not just the first) with
`resolveDocuments` from `@fossil-lang/storage`, fails if any stays unread, then runs and writes.
Fossil turns each `@conn/path` into a location and the connection it goes through, and every byte
goes through an `object_store` store built from the credential the host vends for it: DataFusion
reads each source through `read` on its connection — by range requests, not whole — and the output
is written at `<prefix><path>` through `write` on `{ job }`, in parts when a file is large — or, into
memory, into an `InMemory` the run answers. The host
never signs a URL.

Pass `profiled` — what the editor's `program.describe()` answered — and the run checks the program
against the same columns the editor did:
`run(program, { host, into: { job }, profiled })`. Without it every source is an open row.

`report` is `{ location, dropped }`: where the corpus went and, per edge table, how many input rows
named a vertex that does not exist. What was written is `<location>fossil.json`.

A run that fails throws a `FossilError`: branch on its `code`, never on its message. Its `problem`
is the failure as plain data, which a host stores as it is and turns back into the error with
`FossilError.from(problem)` from `@fossil-lang/types`. **Recording the outcome is the host's**: what
a job's status is, and how long a refused report is tried again, belong to the host's own job
lifecycle, not to the run.

A run executes under a 2 GiB memory budget. One that needs more rejects with a `FossilError` —
`isFossilError(e, 'run/over-budget')`, and `e.data` says which operator asked, for how much, and
what was already held — while the graph is still executing, so nothing has been written.
`/docs/design/three-hosts` has the budget and its measured ceiling.

A host with no storage — Node, a build script — hands every document and source as bytes, by the
location fossil resolves it to, and gets the corpus back:

```ts
import { initFossilExecutor, run } from '@fossil-lang/executor';

await initFossilExecutor(bytes);   // Node only: its fetch rejects file://
const { report, files } = await run(program, {
  files: { 'https://data.invalid/people.shex': shex, 'https://data.invalid/people.csv': csv },
  path: 'https://data.invalid/people.fossil',   // where the program lives: relatives resolve beside it
});
```

### Bundler notes

`--target web` — the consumer controls the `.wasm` URL (Vite `?url`, Next.js
`public/`, Worker `new URL(..., import.meta.url)`). Same pattern as
`@fossil-lang/wasm`; see its README.

The artefact is large (datafusion + arrow + parquet-rs). `run` fetches it on the
first run; run the build with `wasm-opt -Oz` (install `binaryen`) to shrink it.

## Build

```sh
pnpm run build        # build:wasm (cargo + wasm-bindgen --target web) + tsc
pnpm run test         # vitest — real WASM end-to-end smoke (node)
```

The wasm32 build needs a wasm-capable clang for datafusion's zstd-C
(`brew install llvm`); the build script defaults to `/opt/homebrew/opt/llvm`.
