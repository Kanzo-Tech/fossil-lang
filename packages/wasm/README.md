# @fossil-lang/wasm

JS/TS wrapper around the `fossil-wasm` Rust crate's wasm-bindgen artefacts. Provides:

- `initFossilWasm({ wasmUrl })` — consumer-controlled `.wasm` URL loader (memoised).
- `tokenize(text)` — calls the Rust lexer, returns `TokenRow[]`. The Rust lexer
  is the only lexer: no host reimplements one and drifts from the grammar.
- `semanticLegend()` — returns the LSP semantic-tokens legend.
- `FossilPlayground` — the Workspace API class for LSP + compile. `fossil-lsp`
  itself is native-only (stdio over crossbeam), so the browser gets this
  equivalent dispatch surface over the same `fossil-ide` functions.

## Documents and sources: fossil resolves, the host reads

A program names shape documents (`io.shex("@vocab/person.shex")`) and data
sources (`io.csv("@lake/users.csv")`). The checker reads no file and no
network: it reports what it is missing, and the host hands back text through a
`SourceHost` (`@fossil-lang/types`) — the connection map, and `sign(locators)`.

```typescript
import { resolveDocuments } from '@fossil-lang/types';

const pg = new FossilPlayground();
pg.setConnections(await host.connections()); // name → base; re-checks nothing
const h = pg.openFile('prog.fossil', text);   // edited buffers only
const { unread } = await resolveDocuments(pg.workspace(h), host);
const rows = pg.check();
```

- `missingDocuments(h)` — `{ key, locator }` rows. The key is what the program
  wrote, so repointing a connection invalidates nothing; the locator is that
  key through the map, and it is what `sign` receives.
- `registerDocument(key, text)` — what `resolveDocuments` calls for each
  fetched document, until nothing new is missing (a document can name another).
  It is the one loop; a host does not write a second.
- `openFile` is for buffers the user edits. An open `.shex` is the document
  every program naming it reads; opening a program registers nothing it names.
- `sources(h)` — the `ProgramSource[]` the program reads: binding, key,
  locator, catalogue row, reader option. Introspection DESCRIBEs these and
  registers each descriptor under `key` with `registerInferredDescriptor`.

## Why explicit `init({ wasmUrl })` and not auto-load?

We use `wasm-bindgen --target web` (NOT `--target bundler`). This means consumers
control the `.wasm` URL resolution — works in Vite, Next.js, Webpack, Rspack, or
plain `new URL(...)` in a Web Worker context. `--target bundler` was rejected
because its output assumes the consumer's bundler resolves `.wasm` ESM imports,
which a republished library cannot assume of a host's Vite/Next/Webpack config.

## Consumer patterns

### Vite host

```typescript
import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

await initFossilWasm({ wasmUrl });
const tokens = tokenize('User := io.csv("data/people.csv")');
```

### Next.js host (in a Client Component)

```typescript
'use client';
import { initFossilWasm, tokenize } from '@fossil-lang/wasm';

// Put fossil_wasm_bg.wasm in public/wasm/ at build time (next.config.mjs copies it)
useEffect(() => {
  void initFossilWasm({ wasmUrl: '/wasm/fossil_wasm_bg.wasm' });
}, []);
```

### Web Worker

```typescript
// inside a Worker module:
import { initFossilWasm, FossilPlayground } from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

await initFossilWasm({ wasmUrl });
const pg = new FossilPlayground();
```

## Build

```bash
# From repo root
pnpm --filter @fossil-lang/wasm build:wasm   # → packages/wasm/pkg/
pnpm --filter @fossil-lang/wasm build         # = build:wasm + tsc
```

Requires: Rust 1.90 toolchain (per `rust-toolchain.toml` at repo root) +
`wasm-bindgen-cli` 0.2.120 (`cargo install --version 0.2.120 wasm-bindgen-cli`).
Optionally `wasm-opt` (binaryen) for size reduction.

## Source of truth

- Rust crate: `crates/fossil-wasm/` — everything here is a wrapper over its
  wasm-bindgen exports, and nothing in this package reimplements it.
- The grammar the tokenizer follows: `grammar.bnf` at the repo root.
