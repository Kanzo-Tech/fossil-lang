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
import { initFossilExecutor, run } from '@fossil-lang/executor';
import type { Host } from '@fossil-lang/types';

// Lazy — only when the user runs a job (the artefact is large, datafusion-heavy).
// The .wasm ships in this package and the bundler emits it as an asset: nothing
// to copy, no URL to pass. Node passes the bytes: initFossilExecutor(bytes).
// Vite dev: see @fossil-lang/wasm's README for the one optimizeDeps line.
await initFossilExecutor();

const host: Host = {
  connections: async () => ({ lake: 's3://bucket/raw' }),
  // Iceberg REST StorageCredential[]: `read` per connection, `write` on `{ job }`.
  credentials: async (scope, access) => vend(scope, access),
};

const report = await run(program, { host, job: jobId });   // { dest, dropped }, or throws a FossilError
```

`run` reads the documents the program names (every shape, not just the first)
with `resolveDocuments` from `@fossil-lang/storage`, fails if any stays unread,
then runs and writes. Fossil turns each `@conn/path` into a locator and the
connection it goes through, and every byte goes through an `object_store` store
built from the credential the host vends for it: DataFusion reads each source
through `read` on its connection — by range requests, not whole — and the output
is written at `<prefix><path>` through `write` on `{ job }`, in parts when a file
is large. The host never signs a URL.

A run that fails throws a `FossilError`: branch on its `code`, never on its
message. Its `problem` is the failure as plain data, which a host stores as it is
and turns back into the error with `FossilError.from(problem)` from
`@fossil-lang/types`. **Recording the outcome is the host's**: what a job's
status is, and how long a refused report is tried again, belong to the host's
own job lifecycle, not to the run.

Step by step, the same thing is:

```ts
import { resolveDocuments } from '@fossil-lang/storage';

const exec = new FossilExecutor(program);
const { unread } = await resolveDocuments(exec, host); // sets connections, registers documents
const report = await exec.run(host, jobId);            // reads, runs, writes under the job's prefix
exec.free();
```

`report` is `{ dest, dropped }`: where the corpus went and, per edge table, how many
input rows named a vertex that does not exist. What was written is `<dest>fossil.json`.

A run executes under a 2 GiB memory budget. One that needs more rejects with a
`FossilError` — `isFossilError(e, 'run/over-budget')`, and `e.data` says which
operator asked, for how much, and what was already held — while the graph is
still executing, so nothing has been written.
`/docs/design/three-hosts` has the budget and its measured ceiling.

A host with no storage runs over files it holds, and gets them back:

```ts
const { files, report } = await exec.runInMemory({ 'https://data.example.com/users.csv': bytes }, 'memory://corpus');
```

### Bundler notes

`--target web` — the consumer controls the `.wasm` URL (Vite `?url`, Next.js
`public/`, Worker `new URL(..., import.meta.url)`). Same pattern as
`@fossil-lang/wasm`; see its README.

The artefact is large (datafusion + arrow + parquet-rs). **Lazy-load it** (dynamic
`import()` on the run action), and run the build with `wasm-opt -Oz` (install
`binaryen`) to shrink it.

## Build

```sh
pnpm run build        # build:wasm (cargo + wasm-bindgen --target web) + tsc
pnpm run test         # vitest — real WASM end-to-end smoke (node)
```

The wasm32 build needs a wasm-capable clang for datafusion's zstd-C
(`brew install llvm`); the build script defaults to `/opt/homebrew/opt/llvm`.
