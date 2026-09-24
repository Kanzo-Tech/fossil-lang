# @fossil-lang/executor

The DataFusion executor that runs fossil mappings **in the browser** — the heavy,
lazy-loaded counterpart to the LSP-only [`@fossil-lang/wasm`](../wasm).

Wraps the `fossil-df-wasm` wasm-bindgen build (`--target web`). The browser reads
every document and source the program names through the host's `SourceHost`, runs
the mapping on DataFusion-WASM, and gets back the GraphAr output (Parquet +
manifests, as bytes) ready to signed-`PUT`. No mapping runtime on the server.

## Usage

```ts
import { initFossilExecutor, runJob } from '@fossil-lang/executor';
import type { SourceHost } from '@fossil-lang/types';
import wasmUrl from '@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm?url'; // Vite

// Lazy — only when the user runs a job (the artefact is large, datafusion-heavy).
await initFossilExecutor({ wasmUrl });

const host: SourceHost = {
  connections: async () => ({ minio: 'http://minio:9000/bucket' }),
  sign: async (locators) => signEach(locators), // { locator: fetchableUrl }
};

const report = await runJob(program, {
  host,
  output: {
    signOutputUrls: async (paths) => signPuts(paths), // { path: putUrl }
    complete: async (outcome) => patchJob(outcome),
  },
});
```

`runJob` reads the documents the program names (every shape, not just the first)
with `resolveDocuments` from `@fossil-lang/types`, fails the job if any stays
unread, then signs and fetches the sources fossil resolved, runs, uploads and
completes. Fossil turns each `@conn/path` into a locator; the host only signs.

Step by step, the same thing is:

```ts
import { resolveDocuments } from '@fossil-lang/types';

const exec = new FossilExecutor(program);
const { unread } = await resolveDocuments(exec, host); // sets connections, registers documents
const wanted = exec.sources();                         // [{ uri: locator, format }]
const signed = await host.sign(wanted.map((s) => s.uri));
const sources = await Promise.all(
  wanted.map(async (s) => ({
    ...s,
    bytes: new Uint8Array(await (await fetch(signed[s.uri])).arrayBuffer()),
  })),
);
const { files, report } = await exec.run(sources, jobDest);
exec.free();
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
