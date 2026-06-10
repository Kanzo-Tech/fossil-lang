# @fossil-lang/executor

The DataFusion executor that runs fossil mappings **in the browser** — the heavy,
lazy-loaded counterpart to the LSP-only [`@fossil-lang/wasm`](../wasm).

Wraps the `fossil-df-wasm` wasm-bindgen build (`--target web`). The browser fetches
each source by signed URL, runs the mapping on DataFusion-WASM, and gets back the
GraphAr output (Parquet + manifests, as bytes) ready to signed-`PUT`. No mapping
runtime on the server (design §E2).

## Usage

```ts
import { initFossilExecutor, FossilExecutor } from '@fossil-lang/executor';
import wasmUrl from '@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm?url'; // Vite

// Lazy — only when the user runs a job (the artefact is large, datafusion-heavy).
await initFossilExecutor({ wasmUrl });

const exec = new FossilExecutor();
const program = `...fossil mapping...`;

// 1. What sources does the program read?
const srcs = exec.sources(program /*, shexText */);

// 2. Fetch each by signed URL.
const sources = await Promise.all(
  srcs.map(async (s) => ({
    ...s,
    bytes: new Uint8Array(await (await fetch(signedUrlFor(s.uri))).arrayBuffer()),
  })),
);

// 3. Run — get the GraphAr files + the run report.
const { files, runStatus } = await exec.run(program, sources, jobDest /*, shexText */);

// 4. signed-PUT each file.path ← file.bytes, then PATCH the job with runStatus.
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
